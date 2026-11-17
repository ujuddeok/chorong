//! The bounded https fetch (feature `fetch`, on in `cho-cli` and the viewer). Kept apart
//! so crates that only check investigations don't link a network stack.

use crate::source::{
    FETCH_TIMEOUT, MAX_INVESTIGATION_BYTES, MAX_REDIRECTS, SourceError, check_url,
};
use std::time::Duration;

/// Fetch limits. Production uses [`FetchPolicy::default`]; tests shrink the limits and
/// allow plain HTTP on loopback.
#[derive(Debug, Clone)]
pub(crate) struct FetchPolicy {
    pub max_bytes: u64,
    pub timeout: Duration,
    pub max_redirects: u32,
    pub allow_loopback_http: bool,
}

impl Default for FetchPolicy {
    fn default() -> Self {
        Self {
            max_bytes: MAX_INVESTIGATION_BYTES,
            timeout: FETCH_TIMEOUT,
            max_redirects: MAX_REDIRECTS,
            allow_loopback_http: false,
        }
    }
}

/// Fetches `url` with `ureq` (rustls, platform roots, the environment's proxy). Sends no
/// credentials or cookies; follows redirects by hand, each to the same scheme and checked
/// like the original URL.
/// Returns the body and the final URL after redirects.
pub(crate) fn fetch(url: &str, policy: &FetchPolicy) -> Result<(Vec<u8>, String), SourceError> {
    let mut current = check_url(url, policy.allow_loopback_http)?;
    let scheme = current.scheme().to_string();
    let deadline = std::time::Instant::now() + policy.timeout;
    let mut config = ureq::Agent::config_builder()
        .timeout_global(Some(policy.timeout))
        .http_status_as_error(false)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .https_only(!policy.allow_loopback_http)
        .user_agent(concat!("cho/", env!("CARGO_PKG_VERSION")))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .unversioned_rustls_crypto_provider(std::sync::Arc::new(
                    rustls::crypto::ring::default_provider(),
                ))
                .build(),
        );
    if policy.allow_loopback_http {
        config = config.proxy(None);
    }
    let agent: ureq::Agent = config.build().into();
    for _ in 0..=policy.max_redirects {
        let host = current.host_str().unwrap_or("the host").to_string();
        if std::time::Instant::now() >= deadline {
            return Err(SourceError::Fetch(format!(
                "Can't reach {host} (timed out)"
            )));
        }
        let mut resp = agent
            .get(current.as_str())
            .header("Accept", "application/json")
            .call()
            .map_err(|e| {
                let cause = match &e {
                    ureq::Error::Timeout(_) => "timed out".to_string(),
                    ureq::Error::HostNotFound => "host not found".to_string(),
                    ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => {
                        "connection refused".to_string()
                    }
                    other => other.to_string(),
                };
                SourceError::Fetch(format!("Can't reach {host} ({cause})"))
            })?;
        let status = resp.status().as_u16();
        if (300..400).contains(&status) {
            let location = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    SourceError::Fetch(format!("{host} redirected without a location"))
                })?;
            let next = current
                .join(location)
                .map_err(|_| SourceError::Fetch(format!("{host} redirected to an invalid URL")))?;
            if next.scheme() != scheme {
                return Err(SourceError::Fetch(format!(
                    "{host} redirected to a `{}:` URL; only {scheme}:// is followed",
                    next.scheme()
                )));
            }
            current = check_url(next.as_str(), policy.allow_loopback_http)?;
            continue;
        }
        if status != 200 {
            return Err(SourceError::Fetch(format!(
                "{host} answered HTTP {status} for the investigation"
            )));
        }
        let declared = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        if let Some(len) = declared
            && len > policy.max_bytes
        {
            return Err(too_large_fetch(&host, policy.max_bytes));
        }
        return resp
            .body_mut()
            .with_config()
            .limit(policy.max_bytes)
            .read_to_vec()
            .map_err(|e| match e {
                ureq::Error::BodyExceedsLimit(_) => too_large_fetch(&host, policy.max_bytes),
                ureq::Error::Timeout(_) => {
                    SourceError::Fetch(format!("Can't reach {host} (timed out)"))
                }
                other => SourceError::Fetch(format!("reading the reply from {host}: {other}")),
            })
            .map(|body| (body, current.to_string()));
    }
    Err(SourceError::Fetch(format!(
        "more than {} redirects fetching the investigation",
        policy.max_redirects
    )))
}

fn too_large_fetch(host: &str, limit: u64) -> SourceError {
    SourceError::TooLarge(format!(
        "the investigation from {host} is larger than the {limit}-byte limit"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    type Reply = Box<dyn FnOnce(&mut std::net::TcpStream, u16) + Send>;

    /// Serves one canned reply per connection, in order, on loopback.
    fn serve(replies: Vec<Reply>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for reply in replies {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                // Read the request head and refuse to see credentials or cookies.
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                    let lower = line.to_ascii_lowercase();
                    assert!(!lower.starts_with("authorization:"));
                    assert!(!lower.starts_with("cookie:"));
                    line.clear();
                }
                reply(&mut stream, port);
            }
        });
        port
    }

    fn respond(
        status: &str,
        headers: &str,
        body: &[u8],
    ) -> impl FnOnce(&mut std::net::TcpStream, u16) {
        let (status, headers, body) = (status.to_string(), headers.to_string(), body.to_vec());
        move |s, _| {
            let _ = write!(
                s,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
                body.len()
            );
            let _ = s.write_all(&body);
        }
    }

    fn policy() -> FetchPolicy {
        FetchPolicy {
            max_bytes: 1024,
            timeout: Duration::from_secs(2),
            max_redirects: 2,
            allow_loopback_http: true,
        }
    }

    #[test]
    fn fetches_and_follows_same_scheme_redirects() {
        let port = serve(vec![
            Box::new(|s, port| {
                respond(
                    "302 Found",
                    &format!("Location: http://127.0.0.1:{port}/b\r\n"),
                    b"",
                )(s, port)
            }),
            Box::new(respond("200 OK", "", b"{\"chorong\":1}")),
        ]);
        let (body, final_url) = fetch(&format!("http://127.0.0.1:{port}/a"), &policy()).unwrap();
        assert_eq!(body, b"{\"chorong\":1}");
        // The final URL after redirects is recorded, for the UI to show its host.
        assert_eq!(final_url, format!("http://127.0.0.1:{port}/b"));
    }

    #[test]
    fn refuses_redirect_to_another_scheme() {
        let port = serve(vec![Box::new(respond(
            "301 Moved",
            "Location: file:///etc/passwd\r\n",
            b"",
        ))]);
        let err = fetch(&format!("http://127.0.0.1:{port}/a"), &policy()).unwrap_err();
        assert!(err.to_string().contains("`file:`"), "{err}");
    }

    #[test]
    fn refuses_too_many_redirects() {
        let hop = |s: &mut std::net::TcpStream, port: u16| {
            respond(
                "302 Found",
                &format!("Location: http://127.0.0.1:{port}/x\r\n"),
                b"",
            )(s, port)
        };
        let port = serve(vec![Box::new(hop), Box::new(hop), Box::new(hop)]);
        let err = fetch(&format!("http://127.0.0.1:{port}/a"), &policy()).unwrap_err();
        assert!(err.to_string().contains("redirects"), "{err}");
    }

    #[test]
    fn enforces_the_size_cap() {
        // Declared too large.
        let port = serve(vec![Box::new(respond("200 OK", "", &[b' '; 2048]))]);
        let err = fetch(&format!("http://127.0.0.1:{port}/a"), &policy()).unwrap_err();
        assert!(matches!(err, SourceError::TooLarge(_)), "{err}");
        // Undeclared (no Content-Length, body runs until close).
        let port = serve(vec![Box::new(|s: &mut std::net::TcpStream, _| {
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
            let _ = s.write_all(&[b' '; 4096]);
        })]);
        let err = fetch(&format!("http://127.0.0.1:{port}/a"), &policy()).unwrap_err();
        assert!(matches!(err, SourceError::TooLarge(_)), "{err}");
    }

    #[test]
    fn times_out() {
        let port = serve(vec![Box::new(|_: &mut std::net::TcpStream, _| {
            std::thread::sleep(Duration::from_secs(4));
        })]);
        let mut p = policy();
        p.timeout = Duration::from_millis(300);
        let err = fetch(&format!("http://127.0.0.1:{port}/a"), &p).unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
    }

    #[test]
    fn non_200_is_an_error() {
        let port = serve(vec![Box::new(respond("404 Not Found", "", b"nope"))]);
        let err = fetch(&format!("http://127.0.0.1:{port}/a"), &policy()).unwrap_err();
        assert!(err.to_string().contains("404"), "{err}");
    }

    #[test]
    fn production_never_allows_plain_http() {
        assert!(fetch("http://127.0.0.1:9/a", &FetchPolicy::default()).is_err());
    }
}
