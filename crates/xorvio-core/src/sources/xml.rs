//! What the XMLTV and .nfo readers share: files in any encoding read as
//! UTF-8, and entity references resolved the way hand-written files need.

use std::io::{self, BufRead, Chain, Cursor, Read};
use std::sync::LazyLock;

use encoding_rs::{Decoder, Encoding, UTF_8};
use quick_xml::events::BytesRef;
use regex::bytes::Regex;

/// How much of the start of a file is searched for its XML declaration.
const HEAD: u64 = 1024;

/// `encoding="…"` in an XML declaration at the very start.
static DECLARED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?-u)^\s*<\?xml\s[^>]*?\bencoding\s*=\s*["']([A-Za-z0-9._:-]+)["']"#).expect("built-in pattern")
});

/// A file in the encoding its XML declaration names, read as the UTF-8 that
/// quick-xml needs. Older guides still declare ISO-8859-1. A byte-order mark
/// wins over the declaration, and bytes that aren't valid in the encoding
/// become U+FFFD instead of ending the file there.
pub(crate) struct Utf8<R> {
    inner: Chain<Cursor<Vec<u8>>, R>,
    decoder: Decoder,
    out: Box<[u8]>,
    start: usize,
    end: usize,
    finished: bool,
}

impl<R: BufRead> Utf8<R> {
    pub(crate) fn new(mut inner: R) -> io::Result<Self> {
        let mut head = Vec::new();
        inner.by_ref().take(HEAD).read_to_end(&mut head)?;
        let encoding = declared(&head).unwrap_or(UTF_8);
        Ok(Self {
            inner: Cursor::new(head).chain(inner),
            decoder: encoding.new_decoder(),
            out: vec![0; 1 << 16].into_boxed_slice(),
            start: 0,
            end: 0,
            finished: false,
        })
    }
}

/// The encoding a declaration names, if it's one a declaration could be
/// written in (UTF-16 can't be read without a byte-order mark anyway).
fn declared(head: &[u8]) -> Option<&'static Encoding> {
    let label = DECLARED.captures(head)?.get(1)?.as_bytes();
    Encoding::for_label(label).filter(|encoding| encoding.is_ascii_compatible())
}

impl<R: BufRead> BufRead for Utf8<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        while self.start == self.end && !self.finished {
            let input = self.inner.fill_buf()?;
            let last = input.is_empty();
            let (_, read, written, _) = self.decoder.decode_to_utf8(input, &mut self.out, last);
            self.inner.consume(read);
            (self.start, self.end, self.finished) = (0, written, last);
        }
        Ok(&self.out[self.start..self.end])
    }

    fn consume(&mut self, amount: usize) {
        self.start = (self.start + amount).min(self.end);
    }
}

impl<R: BufRead> Read for Utf8<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let count = available.len().min(buffer.len());
        buffer[..count].copy_from_slice(&available[..count]);
        self.consume(count);
        Ok(count)
    }
}

/// An entity reference in element text, resolved. Files written by hand use
/// HTML entities XML doesn't define; unknown ones are kept as written.
pub(crate) fn entity(reference: &BytesRef<'_>) -> String {
    if let Ok(Some(character)) = reference.resolve_char_ref() {
        return character.to_string();
    }
    let name = reference.xml10_content();
    let text = quick_xml::escape::resolve_predefined_entity(&name).or(match name.as_ref() {
        "nbsp" => Some("\u{a0}"),
        "ndash" => Some("–"),
        "mdash" => Some("—"),
        "hellip" => Some("…"),
        _ => None,
    });
    text.map_or_else(|| format!("&{name};"), str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(bytes: &[u8]) -> String {
        let mut text = String::new();
        Utf8::new(bytes).unwrap().read_to_string(&mut text).unwrap();
        text
    }

    #[test]
    fn files_are_read_in_the_encoding_they_declare() {
        assert_eq!(read(b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><t>Caf\xe9</t>"), "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><t>Café</t>");
        assert_eq!(read(b"<?xml version='1.0' encoding='windows-1252'?><t>\x93hi\x94</t>"), "<?xml version='1.0' encoding='windows-1252'?><t>\u{201c}hi\u{201d}</t>");
        assert_eq!(read("<t>Café</t>".as_bytes()), "<t>Café</t>");
        // A byte-order mark wins, and is dropped.
        assert_eq!(read(b"\xef\xbb\xbf<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><t>\xc3\xa9</t>"), "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><t>é</t>");
        // A declaration further in is not a declaration.
        assert_eq!(read(b"<t/><?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>\xe9"), "<t/><?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>\u{fffd}");
    }

    #[test]
    fn bytes_that_dont_fit_dont_end_the_file() {
        let mut long = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><t>Caf\xe9".to_vec();
        long.extend(std::iter::repeat_n(b'x', 200_000));
        long.extend(b"</t>");
        let text = read(&long);
        assert!(text.contains("Caf\u{fffd}x"));
        assert!(text.ends_with("x</t>"));
        assert_eq!(text.len(), long.len() + 2);
    }
}
