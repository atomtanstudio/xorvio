//! Sealing a backup's logins. A key comes from the person's passphrase
//! through Argon2id, and XChaCha20-Poly1305 seals the logins with it, so a
//! wrong passphrase (or a changed byte) opens nothing. The passphrase is
//! never kept. The same sealing, with a random key instead, carries logins
//! across the restart a restore needs, so they never touch the disk in the
//! clear.
//!
//! A sealed file is a header, then the sealed bytes:
//! `XVSEAL01`, how the key was made (1: passphrase, 2: key), Argon2id's
//! memory in KiB, passes and lanes (each a little-endian u32), a 16-byte
//! salt and a 24-byte nonce. The header is authenticated with the contents.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use unicode_normalization::UnicodeNormalization;

const MAGIC: &[u8; 8] = b"XVSEAL01";
const FROM_PASSPHRASE: u8 = 1;
const FROM_KEY: u8 = 2;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const HEADER_LEN: usize = MAGIC.len() + 1 + 12 + SALT_LEN + NONCE_LEN;
pub(crate) const KEY_LEN: usize = 32;

/// What Argon2id costs: 64 MiB and three passes (RFC 9106's second
/// recommendation, with one lane), about half a second on a recent Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cost {
    memory_kib: u32,
    passes: u32,
    lanes: u32,
}

const FULL_COST: Cost = Cost { memory_kib: 64 * 1024, passes: 3, lanes: 1 };
#[cfg(not(test))]
const COST: Cost = FULL_COST;
/// Tests seal and open many times; the format and checks are the same.
#[cfg(test)]
const COST: Cost = Cost { memory_kib: 1024, passes: 1, lanes: 1 };

/// The most a backup may ask for, so a doctored file can't make opening it
/// take all the memory there is.
const MOST: Cost = Cost { memory_kib: 1024 * 1024, passes: 16, lanes: 16 };

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SealError {
    /// The passphrase (or key) isn't the one it was sealed with, or the
    /// sealed bytes changed.
    WrongKey,
    /// It isn't sealed logins at all, or it's cut short.
    Damaged,
}

/// A random key, from the operating system.
pub(crate) fn random_key() -> anyhow::Result<[u8; KEY_LEN]> {
    random()
}

fn random<const N: usize>() -> anyhow::Result<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|error| anyhow::anyhow!("The system couldn't provide random numbers ({error})."))?;
    Ok(bytes)
}

/// Seals `plain` with a key from `passphrase`.
pub(crate) fn seal_with_passphrase(plain: &[u8], passphrase: &str) -> anyhow::Result<Vec<u8>> {
    let salt: [u8; SALT_LEN] = random()?;
    let key = derive(passphrase, &salt, COST).map_err(|_| anyhow::anyhow!("The backup's logins couldn't be sealed."))?;
    seal(plain, FROM_PASSPHRASE, COST, salt, &key)
}

pub(crate) fn open_with_passphrase(sealed: &[u8], passphrase: &str) -> Result<Vec<u8>, SealError> {
    let header = Header::read(sealed)?;
    if header.kind != FROM_PASSPHRASE {
        return Err(SealError::Damaged);
    }
    let key = derive(passphrase, &header.salt, header.cost)?;
    open(sealed, &key)
}

/// Seals `plain` with a key that's already random.
pub(crate) fn seal_with_key(plain: &[u8], key: &[u8; KEY_LEN]) -> anyhow::Result<Vec<u8>> {
    seal(plain, FROM_KEY, Cost { memory_kib: 0, passes: 0, lanes: 0 }, [0; SALT_LEN], key)
}

pub(crate) fn open_with_key(sealed: &[u8], key: &[u8; KEY_LEN]) -> Result<Vec<u8>, SealError> {
    if Header::read(sealed)?.kind != FROM_KEY {
        return Err(SealError::Damaged);
    }
    open(sealed, key)
}

struct Header {
    kind: u8,
    cost: Cost,
    salt: [u8; SALT_LEN],
    nonce: [u8; NONCE_LEN],
}

impl Header {
    fn bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER_LEN);
        bytes.extend_from_slice(MAGIC);
        bytes.push(self.kind);
        for value in [self.cost.memory_kib, self.cost.passes, self.cost.lanes] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&self.salt);
        bytes.extend_from_slice(&self.nonce);
        bytes
    }

    fn read(sealed: &[u8]) -> Result<Header, SealError> {
        if sealed.len() < HEADER_LEN || &sealed[..MAGIC.len()] != MAGIC {
            return Err(SealError::Damaged);
        }
        let number = |at: usize| u32::from_le_bytes([sealed[at], sealed[at + 1], sealed[at + 2], sealed[at + 3]]);
        let at = MAGIC.len() + 1;
        let cost = Cost { memory_kib: number(at), passes: number(at + 4), lanes: number(at + 8) };
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&sealed[at + 12..at + 12 + SALT_LEN]);
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&sealed[at + 12 + SALT_LEN..HEADER_LEN]);
        Ok(Header { kind: sealed[MAGIC.len()], cost, salt, nonce })
    }
}

fn derive(passphrase: &str, salt: &[u8], cost: Cost) -> Result<[u8; KEY_LEN], SealError> {
    if cost.memory_kib > MOST.memory_kib || cost.passes > MOST.passes || cost.lanes > MOST.lanes {
        return Err(SealError::Damaged);
    }
    let params = Params::new(cost.memory_kib, cost.passes, cost.lanes, Some(KEY_LEN)).map_err(|_| SealError::Damaged)?;
    // The same passphrase typed on another keyboard can arrive composed
    // differently ("é" as one character or two); this makes it one form.
    let passphrase: String = passphrase.nfc().collect();
    let mut key = [0u8; KEY_LEN];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|_| SealError::Damaged)?;
    Ok(key)
}

fn seal(plain: &[u8], kind: u8, cost: Cost, salt: [u8; SALT_LEN], key: &[u8; KEY_LEN]) -> anyhow::Result<Vec<u8>> {
    let header = Header { kind, cost, salt, nonce: random()? };
    let mut sealed = header.bytes();
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| anyhow::anyhow!("The backup's logins couldn't be sealed."))?;
    let body = cipher
        .encrypt(&XNonce::from(header.nonce), Payload { msg: plain, aad: &sealed })
        .map_err(|_| anyhow::anyhow!("The backup's logins couldn't be sealed."))?;
    sealed.extend_from_slice(&body);
    Ok(sealed)
}

fn open(sealed: &[u8], key: &[u8; KEY_LEN]) -> Result<Vec<u8>, SealError> {
    let header = Header::read(sealed)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| SealError::Damaged)?;
    cipher
        .decrypt(&XNonce::from(header.nonce), Payload { msg: &sealed[HEADER_LEN..], aad: &sealed[..HEADER_LEN] })
        .map_err(|_| SealError::WrongKey)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn key_from_hex(text: &str) -> Option<[u8; KEY_LEN]> {
    let text = text.trim();
    if text.len() != KEY_LEN * 2 || !text.is_ascii() {
        return None;
    }
    let mut key = [0u8; KEY_LEN];
    for (index, slot) in key.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_passphrase_opens_what_it_sealed_and_nothing_else() {
        let sealed = seal_with_passphrase(b"logins", "correct horse battery").unwrap();
        assert!(!sealed.windows(6).any(|window| window == b"logins"), "nothing in the clear");
        assert_eq!(open_with_passphrase(&sealed, "correct horse battery").unwrap(), b"logins");
        assert_eq!(open_with_passphrase(&sealed, "Correct horse battery"), Err(SealError::WrongKey));
        // Sealing twice never gives the same bytes (a new salt and nonce).
        assert_ne!(seal_with_passphrase(b"logins", "correct horse battery").unwrap(), sealed);
    }

    #[test]
    fn the_real_cost_is_one_argon2_accepts() {
        let key = derive("passphrase", &[7; SALT_LEN], FULL_COST).unwrap();
        assert_eq!(key, derive("passphrase", &[7; SALT_LEN], FULL_COST).unwrap(), "the same passphrase and salt, the same key");
        assert_ne!(key, derive("passphrase", &[8; SALT_LEN], FULL_COST).unwrap());
    }

    #[test]
    fn composed_and_decomposed_passphrases_are_the_same() {
        let sealed = seal_with_passphrase(b"x", "caf\u{e9} au lait").unwrap();
        assert_eq!(open_with_passphrase(&sealed, "cafe\u{301} au lait").unwrap(), b"x");
    }

    #[test]
    fn a_changed_byte_or_a_doctored_header_opens_nothing() {
        let mut sealed = seal_with_passphrase(b"logins", "passphrase").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 1;
        assert_eq!(open_with_passphrase(&sealed, "passphrase"), Err(SealError::WrongKey));
        let mut greedy = seal_with_passphrase(b"logins", "passphrase").unwrap();
        greedy[9..13].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(open_with_passphrase(&greedy, "passphrase"), Err(SealError::Damaged));
        assert_eq!(open_with_passphrase(b"XVSEAL01", "passphrase"), Err(SealError::Damaged));
        assert_eq!(open_with_passphrase(b"something else entirely, quite long enough to be a header", "p"), Err(SealError::Damaged));
    }

    #[test]
    fn a_random_key_round_trips_and_reads_back_from_hex() {
        let key = random_key().unwrap();
        assert_eq!(key_from_hex(&hex(&key)), Some(key));
        assert_eq!(key_from_hex("not hex"), None);
        let sealed = seal_with_key(b"logins", &key).unwrap();
        assert_eq!(open_with_key(&sealed, &key).unwrap(), b"logins");
        assert_eq!(open_with_key(&sealed, &random_key().unwrap()), Err(SealError::WrongKey));
        assert_eq!(open_with_passphrase(&sealed, ""), Err(SealError::Damaged), "a key-sealed file isn't a passphrase one");
    }
}
