use std::path::Path;

use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio_stream::StreamExt;

use crate::error::{ArtifactError, Result};

/// Outcome of a completed download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadOutcome {
    /// Final URL after redirects.
    pub final_url: String,
    /// Server `ETag`, when the final response carried one.
    pub etag: Option<String>,
    /// Bytes written to the destination.
    pub bytes: u64,
}

/// Size-capped, resumable HTTPS downloader.
///
/// Downloads into a `.part` file using `Range` + `If-None-Match`/`ETag` so an
/// interrupted transfer can continue from its offset. Redirects are **not**
/// followed automatically: every hop is handed to `on_redirect`, which the
/// caller uses to re-validate SSRF policy and approval before continuing.
#[derive(Debug, Clone)]
pub struct Downloader {
    client: reqwest::Client,
    max_redirects: usize,
    require_https: bool,
}

impl Downloader {
    /// Builds a downloader.
    ///
    /// `connect_timeout` / `total_timeout` bound each hop; `None` uses the
    /// defaults. `max_redirects` caps hops per download.
    pub fn new(
        connect_timeout: Option<std::time::Duration>,
        total_timeout: Option<std::time::Duration>,
        max_redirects: usize,
    ) -> Result<Self> {
        // Ring TLS provider (no aws-lc native build; cross-compiles to
        // Windows cleanly). Installing it once is idempotent.
        drop(rustls::crypto::ring::default_provider().install_default());
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("ene-artifact/", env!("CARGO_PKG_VERSION")));
        if let Some(timeout) = connect_timeout {
            builder = builder.connect_timeout(timeout);
        }
        if let Some(timeout) = total_timeout {
            builder = builder.timeout(timeout);
        }
        let client = builder
            .build()
            .map_err(|e| ArtifactError::Key(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            client,
            max_redirects,
            require_https: true,
        })
    }

    /// Test-only constructor that accepts plain HTTP (used to exercise the
    /// transfer mechanics against a loopback test server; the production
    /// [`Downloader::new`] always requires https).
    #[cfg(test)]
    pub(crate) fn test_new() -> Self {
        drop(rustls::crypto::ring::default_provider().install_default());
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("test client"),
            max_redirects: 3,
            require_https: false,
        }
    }

    /// Downloads `url` into `destination` (a `.part` file).
    ///
    /// `expected_sha256` / `expected_size` come from the signed catalog and
    /// are verified after the transfer. `max_bytes` caps the total size at
    /// every hop. `on_redirect` is invoked for each redirect target and must
    /// return `Ok` for hops the caller's policy allows.
    ///
    /// If `destination` already exists, the download resumes from its current
    /// length (the server must support `Range`; a `200` response restarts the
    /// transfer from scratch).
    pub async fn download_to(
        &self,
        url: &str,
        destination: &Path,
        expected_sha256: &str,
        expected_size: u64,
        max_bytes: u64,
        on_redirect: &(dyn Fn(&str) -> Result<()> + Sync),
    ) -> Result<DownloadOutcome> {
        let mut current_url = url.to_string();
        let mut etag: Option<String> = None;
        let mut bytes = 0_u64;
        for _hop in 0..=self.max_redirects {
            let outcome = self
                .fetch_once(&current_url, destination, etag.as_deref(), bytes, max_bytes)
                .await?;
            if let Some(location) = outcome.redirect_to {
                on_redirect(&location)?;
                current_url = location;
                // ETag belongs to the resource, not the hop: keep the value
                // so a resumed transfer after a redirect still matches.
                continue;
            }
            bytes = outcome.bytes;
            etag = outcome.etag;
            if bytes != expected_size {
                return Err(ArtifactError::SizeMismatch {
                    artifact: url.to_string(),
                    expected: expected_size,
                    actual: bytes,
                });
            }
            if crate::digest::verify_sha256(destination, expected_sha256)? {
                return Ok(DownloadOutcome {
                    final_url: current_url,
                    etag,
                    bytes,
                });
            }
            return Err(ArtifactError::DigestMismatch {
                artifact: url.to_string(),
                expected: expected_sha256.to_string(),
                actual: crate::digest::sha256_hex(&std::fs::read(destination)?),
            });
        }
        Err(ArtifactError::TooManyRedirects {
            limit: self.max_redirects,
        })
    }

    async fn fetch_once(
        &self,
        url: &str,
        destination: &Path,
        etag: Option<&str>,
        offset: u64,
        max_bytes: u64,
    ) -> Result<FetchOnce> {
        let parsed = url::Url::parse(url).map_err(|e| ArtifactError::Transport {
            url: url.to_string(),
            message: format!("invalid URL: {e}"),
        })?;
        if self.require_https && parsed.scheme() != "https" {
            return Err(ArtifactError::UnsupportedScheme {
                scheme: parsed.scheme().to_string(),
            });
        }

        let mut request = self.client.get(url);
        if offset > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
            if let Some(etag) = etag {
                request = request.header(reqwest::header::IF_NONE_MATCH, etag);
            }
        }
        let response = request
            .send()
            .await
            .map_err(|e| ArtifactError::transport(url, &e))?;
        let status = response.status();

        if status.is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            return Ok(FetchOnce {
                bytes: offset,
                etag: None,
                redirect_to: Some(location.ok_or_else(|| ArtifactError::HttpStatus {
                    url: url.to_string(),
                    status: status.as_u16(),
                })?),
            });
        }

        let write_offset = match status.as_u16() {
            200 => 0_u64,
            206 => offset,
            416 => {
                // Range not satisfiable: the server says the resource is
                // already fully present at our offset.
                let current = std::fs::metadata(destination).map_or(0, |m| m.len());
                if current >= offset {
                    return Ok(FetchOnce {
                        bytes: current,
                        etag: None,
                        redirect_to: None,
                    });
                }
                return Err(ArtifactError::HttpStatus {
                    url: url.to_string(),
                    status: 416,
                });
            }
            _ => {
                return Err(ArtifactError::HttpStatus {
                    url: url.to_string(),
                    status: status.as_u16(),
                });
            }
        };

        let new_etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let mut total = write_offset;
        if write_offset == 0 {
            // The server ignored our Range; restart from an empty file.
            let file = std::fs::File::create(destination)?;
            file.set_len(0)?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(destination)
            .await?;
        file.seek(std::io::SeekFrom::Start(write_offset)).await?;

        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ArtifactError::transport(url, &e))?;
            total = total.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
            if total > max_bytes {
                return Err(ArtifactError::SizeExceeded {
                    max: max_bytes,
                    got: total,
                });
            }
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        file.sync_all().await?;
        Ok(FetchOnce {
            bytes: total,
            etag: new_etag,
            redirect_to: None,
        })
    }
}

struct FetchOnce {
    bytes: u64,
    etag: Option<String>,
    redirect_to: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Minimal HTTP/1.1 server: serves `body` with `ETag` + `Range` support.
    async fn serve(body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buf = Vec::new();
            let mut tmp = [0u8; 1024];
            loop {
                let n = socket.read(&mut tmp).await.expect("read");
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&buf).to_string();
            let range_start = request.lines().find_map(|line| {
                line.strip_prefix("Range: ")
                    .and_then(|range| range.trim().strip_prefix("bytes="))
                    .and_then(|spec| spec.split('-').next())
                    .and_then(|start| start.parse::<usize>().ok())
            });
            let (status_line, content_range, payload) = match range_start {
                Some(start) => (
                    "206 Partial Content",
                    format!(
                        "Content-Range: bytes {start}-{}/{}\r\n",
                        body.len() - 1,
                        body.len()
                    ),
                    &body[start..],
                ),
                None => ("200 OK", String::new(), body),
            };
            let head = format!(
                "HTTP/1.1 {status_line}\r\n{content_range}ETag: \"v1\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            socket.write_all(head.as_bytes()).await.expect("write head");
            socket.write_all(payload).await.expect("write body");
        });
        format!("http://{addr}/artifact.bin")
    }

    fn no_redirects(_url: &str) -> Result<()> {
        Err(ArtifactError::RedirectRejected("not expected".to_string()))
    }

    #[tokio::test]
    async fn downloads_and_verifies() {
        let body: &'static [u8] = b"artifact payload 1234567890";
        let url = serve(body).await;
        let dir = tempfile::tempdir().expect("tempdir");
        let part = dir.path().join("artifact.part");
        let digest = crate::digest::sha256_hex(body);
        let outcome = Downloader::test_new()
            .download_to(&url, &part, &digest, body.len() as u64, 1024, &no_redirects)
            .await
            .expect("download");
        assert_eq!(outcome.bytes, body.len() as u64);
        assert_eq!(outcome.etag.as_deref(), Some("\"v1\""));
        assert_eq!(std::fs::read(&part).expect("read"), body);
    }

    #[tokio::test]
    async fn resumes_from_existing_part() {
        let body: &'static [u8] = b"artifact payload 1234567890";
        let url = serve(body).await;
        let dir = tempfile::tempdir().expect("tempdir");
        let part = dir.path().join("artifact.part");
        std::fs::write(&part, &body[..7]).expect("seed part");
        let digest = crate::digest::sha256_hex(body);
        let outcome = Downloader::test_new()
            .download_to(&url, &part, &digest, body.len() as u64, 1024, &no_redirects)
            .await
            .expect("resumed download");
        assert_eq!(outcome.bytes, body.len() as u64);
        assert_eq!(std::fs::read(&part).expect("read"), body);
    }

    #[tokio::test]
    async fn size_cap_aborts() {
        let body: &'static [u8] = b"0123456789";
        let url = serve(body).await;
        let dir = tempfile::tempdir().expect("tempdir");
        let part = dir.path().join("artifact.part");
        let digest = crate::digest::sha256_hex(body);
        let err = Downloader::test_new()
            .download_to(&url, &part, &digest, body.len() as u64, 5, &no_redirects)
            .await
            .expect_err("cap must trip");
        assert!(matches!(err, ArtifactError::SizeExceeded { .. }));
    }

    #[tokio::test]
    async fn digest_mismatch_fails() {
        let body: &'static [u8] = b"payload";
        let url = serve(body).await;
        let dir = tempfile::tempdir().expect("tempdir");
        let part = dir.path().join("artifact.part");
        let err = Downloader::test_new()
            .download_to(
                &url,
                &part,
                &"0".repeat(64),
                body.len() as u64,
                1024,
                &no_redirects,
            )
            .await
            .expect_err("digest mismatch");
        assert!(matches!(err, ArtifactError::DigestMismatch { .. }));
    }

    #[tokio::test]
    async fn redirects_require_caller_validation() {
        let body: &'static [u8] = b"redirected payload";
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buf = Vec::new();
            let mut tmp = [0u8; 1024];
            loop {
                let n = socket.read(&mut tmp).await.expect("read");
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let head = "HTTP/1.1 302 Found\r\nLocation: https://example.test/final.bin\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            socket.write_all(head.as_bytes()).await.expect("write");
            let _ = buf;
        });
        let url = format!("http://{addr}/start.bin");
        let dir = tempfile::tempdir().expect("tempdir");
        let part = dir.path().join("artifact.part");
        let digest = crate::digest::sha256_hex(body);
        let err = Downloader::test_new()
            .download_to(&url, &part, &digest, body.len() as u64, 1024, &no_redirects)
            .await
            .expect_err("redirect must be rejected by the caller policy");
        assert!(matches!(err, ArtifactError::RedirectRejected(_)));
    }
}
