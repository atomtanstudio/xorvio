//! The Cast v2 wire format: each message is a protobuf `CastMessage`
//! (cast_channel.proto in Chromium) behind a four-byte big-endian length.
//! The message has seven fields and only strings, so it's written and read
//! here by hand rather than through a protobuf compiler.
//!
//! ```proto
//! message CastMessage {
//!   required ProtocolVersion protocol_version = 1;  // CASTV2_1_0 = 0
//!   required string source_id = 2;
//!   required string destination_id = 3;
//!   required string namespace = 4;
//!   required PayloadType payload_type = 5;          // STRING = 0, BINARY = 1
//!   optional string payload_utf8 = 6;
//!   optional bytes payload_binary = 7;
//! }
//! ```

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Receivers refuse messages over 64 KiB; anything much bigger coming the
/// other way isn't a Cast message.
pub const MAX_MESSAGE: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CastMessage {
    pub source: String,
    pub destination: String,
    pub namespace: String,
    pub payload: Payload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payload {
    /// JSON, which is every namespace Xorvio uses.
    Text(String),
    /// Device authentication's challenge and reply, which Xorvio doesn't use.
    Binary(Vec<u8>),
}

impl CastMessage {
    pub fn text(source: &str, destination: &str, namespace: &str, json: String) -> CastMessage {
        CastMessage {
            source: source.to_string(),
            destination: destination.to_string(),
            namespace: namespace.to_string(),
            payload: Payload::Text(json),
        }
    }

    /// The protobuf encoding, without the length in front.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.namespace.len());
        // proto2 writes required fields even when they hold the default.
        varint_field(&mut out, 1, 0);
        bytes_field(&mut out, 2, self.source.as_bytes());
        bytes_field(&mut out, 3, self.destination.as_bytes());
        bytes_field(&mut out, 4, self.namespace.as_bytes());
        match &self.payload {
            Payload::Text(text) => {
                varint_field(&mut out, 5, 0);
                bytes_field(&mut out, 6, text.as_bytes());
            }
            Payload::Binary(bytes) => {
                varint_field(&mut out, 5, 1);
                bytes_field(&mut out, 7, bytes);
            }
        }
        out
    }

    /// Reads a message, skipping fields this version doesn't know.
    pub fn decode(mut bytes: &[u8]) -> Result<CastMessage, DecodeError> {
        let mut message = CastMessage {
            source: String::new(),
            destination: String::new(),
            namespace: String::new(),
            payload: Payload::Text(String::new()),
        };
        let mut binary = false;
        let mut text = None;
        let mut data = None;
        while !bytes.is_empty() {
            let key = read_varint(&mut bytes)?;
            let (field, wire) = (key >> 3, key & 7);
            match wire {
                0 => {
                    let value = read_varint(&mut bytes)?;
                    if field == 5 {
                        binary = value == 1;
                    }
                }
                2 => {
                    let len = usize::try_from(read_varint(&mut bytes)?).map_err(|_| DecodeError)?;
                    if len > bytes.len() {
                        return Err(DecodeError);
                    }
                    let (value, rest) = bytes.split_at(len);
                    bytes = rest;
                    let string = || String::from_utf8(value.to_vec()).map_err(|_| DecodeError);
                    match field {
                        2 => message.source = string()?,
                        3 => message.destination = string()?,
                        4 => message.namespace = string()?,
                        6 => text = Some(string()?),
                        7 => data = Some(value.to_vec()),
                        _ => {}
                    }
                }
                1 => bytes = bytes.get(8..).ok_or(DecodeError)?,
                5 => bytes = bytes.get(4..).ok_or(DecodeError)?,
                _ => return Err(DecodeError),
            }
        }
        message.payload = if binary { Payload::Binary(data.unwrap_or_default()) } else { Payload::Text(text.unwrap_or_default()) };
        Ok(message)
    }
}

/// A message that isn't a CastMessage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the device sent a message Xorvio couldn't read")]
pub struct DecodeError;

fn varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn varint_field(out: &mut Vec<u8>, field: u64, value: u64) {
    varint(out, field << 3);
    varint(out, value);
}

fn bytes_field(out: &mut Vec<u8>, field: u64, value: &[u8]) {
    varint(out, (field << 3) | 2);
    varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn read_varint(bytes: &mut &[u8]) -> Result<u64, DecodeError> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = bytes.split_first().ok_or(DecodeError)?;
        *bytes = rest;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(DecodeError)
}

/// Writes one framed message.
pub async fn write_message(writer: &mut (impl AsyncWrite + Unpin), message: &CastMessage) -> std::io::Result<()> {
    let body = message.encode();
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    writer.write_all(&frame).await?;
    writer.flush().await
}

/// Reads one framed message; `None` when the other side closed the connection.
pub async fn read_message(reader: &mut (impl AsyncRead + Unpin)) -> std::io::Result<Option<CastMessage>> {
    let mut length = [0u8; 4];
    match reader.read_exact(&mut length).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_MESSAGE {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "a Cast message far too large"));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await?;
    CastMessage::decode(&body).map(Some).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_as_protobuf_does() {
        let message = CastMessage::text("sender-0", "receiver-0", "urn:x-cast:com.google.cast.tp.connection", r#"{"type":"CONNECT"}"#.into());
        let bytes = message.encode();
        // protocol_version = 0, then source_id "sender-0".
        assert_eq!(&bytes[..4], &[0x08, 0x00, 0x12, 8]);
        assert_eq!(&bytes[4..12], b"sender-0");
        assert_eq!(bytes[12], 0x1a, "destination_id");
        // payload_type STRING, then payload_utf8.
        let tail = [0x28, 0x00, 0x32, 18];
        let at = bytes.windows(4).position(|w| w == tail).expect("payload fields");
        assert_eq!(&bytes[at + 4..], br#"{"type":"CONNECT"}"#);
        assert_eq!(CastMessage::decode(&bytes).unwrap(), message);
    }

    #[test]
    fn long_payloads_and_binary_round_trip() {
        let json = format!(r#"{{"type":"LOAD","x":"{}"}}"#, "a".repeat(300));
        let message = CastMessage::text("sender-0", "web-5", "urn:x-cast:com.google.cast.media", json);
        assert_eq!(CastMessage::decode(&message.encode()).unwrap(), message);
        let binary = CastMessage { payload: Payload::Binary(vec![0, 1, 2, 255]), ..message };
        assert_eq!(CastMessage::decode(&binary.encode()).unwrap(), binary);
    }

    #[test]
    fn unknown_fields_are_skipped_and_garbage_refused() {
        let mut bytes = CastMessage::text("a", "b", "c", "{}".into()).encode();
        // Field 9 as a varint, field 10 as fixed64, field 11 as fixed32.
        bytes.extend_from_slice(&[0x48, 0x96, 0x01]);
        bytes.extend_from_slice(&[0x51, 1, 2, 3, 4, 5, 6, 7, 8]);
        bytes.extend_from_slice(&[0x5d, 1, 2, 3, 4]);
        assert_eq!(CastMessage::decode(&bytes).unwrap().namespace, "c");
        assert_eq!(CastMessage::decode(&[0x12, 50, b'x']), Err(DecodeError), "a string longer than the message");
        assert_eq!(CastMessage::decode(&[0x0b]), Err(DecodeError), "a group, which Cast never sends");
        assert_eq!(CastMessage::decode(&[0x08, 0xff]), Err(DecodeError), "a varint cut short");
    }

    #[tokio::test]
    async fn frames_carry_their_length() {
        let message = CastMessage::text("sender-0", "receiver-0", "ns", r#"{"type":"PING"}"#.into());
        let mut wire = Vec::new();
        write_message(&mut wire, &message).await.unwrap();
        assert_eq!(u32::from_be_bytes(wire[..4].try_into().unwrap()) as usize, wire.len() - 4);
        let mut reader = wire.as_slice();
        assert_eq!(read_message(&mut reader).await.unwrap(), Some(message));
        assert_eq!(read_message(&mut reader).await.unwrap(), None, "closed cleanly");
        let mut huge = &(u32::MAX).to_be_bytes()[..];
        assert!(read_message(&mut huge).await.is_err());
    }
}
