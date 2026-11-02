//! Artwork at plain-http addresses, fetched for the interface. The web view
//! asks for a page's plain-http images over https instead (as browsers now
//! do for secure pages), and many providers' image servers speak only http,
//! so their posters and logos never showed. The interface points such
//! images at an `xorvio-image` address ([`image_url`]), and the app fetches
//! them as given and passes on only what really is an image.

use std::sync::LazyLock;

use tokio::sync::Semaphore;

use crate::error::SourceError;
use crate::http::ApiCall;
use crate::{Core, Result};

/// The scheme of plain-http images, served by the app (see [`image_url`]).
pub const IMAGE_SCHEME: &str = "xorvio-image";

/// The largest image passed on; posters and logos are far smaller.
const MOST_BYTES: usize = 20 << 20;

/// Images fetched at once, so a wall of posters doesn't open dozens of
/// connections to one provider (a browser opens six).
static AT_ONCE: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(8));

/// The address the interface loads a plain-http image from (it makes the
/// same address itself: `imageSrc` in `src/lib/images.ts`).
pub fn image_url(link: &str) -> String {
    let encoded: String = url::form_urlencoded::byte_serialize(link.as_bytes()).collect();
    if cfg!(windows) {
        format!("http://{IMAGE_SCHEME}.localhost/{encoded}")
    } else {
        format!("{IMAGE_SCHEME}://localhost/{encoded}")
    }
}

/// The plain-http address in an image address's path ("/<encoded address>"),
/// or `None` when it isn't one.
pub fn parse_image_path(request_path: &str) -> Option<String> {
    let encoded = request_path.strip_prefix('/')?;
    let link = url::form_urlencoded::parse(format!("p={encoded}").as_bytes()).find(|(key, _)| key == "p")?.1.into_owned();
    is_plain_http(&link).then_some(link)
}

fn is_plain_http(link: &str) -> bool {
    url::Url::parse(link).is_ok_and(|parsed| parsed.scheme() == "http" && parsed.host_str().is_some_and(|host| !host.is_empty()))
}

/// What the bytes are, whatever the server calls them (image servers often
/// say `application/octet-stream`); otherwise the image type it declares.
/// `None` for anything that isn't an image.
pub(crate) fn image_type(declared: Option<&str>, bytes: &[u8]) -> Option<String> {
    let sniffed = if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && matches!(&bytes[8..12], b"avif" | b"avis") {
        Some("image/avif")
    } else {
        None
    };
    sniffed.map(str::to_string).or_else(|| {
        let declared = declared?.split(';').next()?.trim().to_ascii_lowercase();
        declared.starts_with("image/").then_some(declared)
    })
}

impl Core {
    /// An image at a plain-http address, for the interface: its bytes and
    /// type. Anything else at the address (an error page, a playlist) is
    /// refused.
    pub async fn plain_http_image(&self, link: &str) -> Result<(Vec<u8>, String)> {
        if !is_plain_http(link) {
            return Err(SourceError::Unsupported("That isn't a plain-http image address.".into()).into());
        }
        let _turn = AT_ONCE.acquire().await?;
        let reply = self.http.api(ApiCall::get(link, &[])).await?;
        if reply.bytes.len() > MOST_BYTES {
            return Err(SourceError::Unsupported("That image is too large.".into()).into());
        }
        let content_type = image_type(reply.content_type.as_deref(), &reply.bytes)
            .ok_or_else(|| SourceError::Unsupported("That address doesn't hold an image.".into()))?;
        Ok((reply.bytes, content_type))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemorySecrets;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A plain-http image server on 127.0.0.1: a JPEG at /poster.jpg (sent
    /// as octet-stream, as image servers often do), a page anywhere else.
    async fn image_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut buffer = [0u8; 2048];
                    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                        match socket.read(&mut buffer).await {
                            Ok(0) | Err(_) => return,
                            Ok(count) => head.extend_from_slice(&buffer[..count]),
                        }
                    }
                    let poster = String::from_utf8_lossy(&head).starts_with("GET /poster.jpg ");
                    let (kind, body): (&str, &[u8]) =
                        if poster { ("application/octet-stream", &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10]) } else { ("text/html", b"<html>Not here</html>") };
                    let header = format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = socket.write_all(header.as_bytes()).await;
                    let _ = socket.write_all(body).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        base
    }

    #[tokio::test]
    async fn plain_http_images_are_fetched_and_nothing_else() {
        let base = image_server().await;
        let folder = tempfile::tempdir().unwrap();
        let core = Core::open(folder.path(), Arc::new(MemorySecrets::new())).unwrap();
        let (bytes, kind) = core.plain_http_image(&format!("{base}/poster.jpg")).await.unwrap();
        assert_eq!((bytes.len(), kind.as_str()), (6, "image/jpeg"));
        assert!(core.plain_http_image(&format!("{base}/login")).await.unwrap_err().to_string().contains("doesn't hold an image"));
        assert!(core.plain_http_image("https://image.example/a.jpg").await.is_err());
    }

    #[test]
    fn addresses_round_trip_and_stay_plain_http() {
        let link = "http://cmc.example:8080/images/movies//poster one.jpg?size=w342&x=a+b";
        let address = image_url(link);
        let parsed = url::Url::parse(&address).unwrap();
        assert_eq!(parse_image_path(parsed.path()).as_deref(), Some(link));
        // What the interface makes (encodeURIComponent) reads the same.
        assert_eq!(
            parse_image_path("/http%3A%2F%2Fcmc.example%3A8080%2Fimages%2Fmovies%2F%2Fposter%20one.jpg%3Fsize%3Dw342%26x%3Da%2Bb").as_deref(),
            Some(link)
        );
        for other in ["https://image.example/a.jpg", "file:///etc/passwd", "http://", "xorvio-art://poster/a", "/relative.jpg", ""] {
            let encoded: String = url::form_urlencoded::byte_serialize(other.as_bytes()).collect();
            assert_eq!(parse_image_path(&format!("/{encoded}")), None, "{other}");
        }
    }

    #[test]
    fn only_images_are_passed_on() {
        assert_eq!(image_type(Some("application/octet-stream"), &[0xFF, 0xD8, 0xFF, 0xE0]).as_deref(), Some("image/jpeg"));
        assert_eq!(image_type(None, b"\x89PNG\r\n\x1a\n....").as_deref(), Some("image/png"));
        assert_eq!(image_type(Some("text/plain"), b"RIFF\0\0\0\0WEBPVP8 ").as_deref(), Some("image/webp"));
        assert_eq!(image_type(Some("image/svg+xml; charset=utf-8"), b"<svg/>").as_deref(), Some("image/svg+xml"));
        assert_eq!(image_type(Some("text/html"), b"<html>Not found</html>"), None);
        assert_eq!(image_type(None, b"#EXTM3U"), None);
    }
}
