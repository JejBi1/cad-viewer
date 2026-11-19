//! The one place that talks HTTP. Built on `ureq` with rustls.
//!
//! Rules enforced here:
//! * redirects are never followed by the library; this module follows at most three itself
//!   and only to the same origin or to Google API / content hosts over HTTPS;
//! * the `Authorization` header goes only to the original origin or to `*.googleapis.com`;
//! * responses with a status of 400 and above are returned to the caller, not turned into
//!   errors, so the caller can map them to Czech messages.
//!
//! `get` is the only call used for the Drive API. `post_form` exists for the OAuth token
//! endpoint and is not used by `drive.rs` (a test checks that).

use std::io::Read;
use std::time::Duration;

use ureq::tls::{RootCerts, TlsConfig};

use crate::error::DriveError;
use crate::secret::Secret;
use crate::urlutil::{UrlParts, split_url};

const MAX_REDIRECTS: usize = 3;
/// Upper bound for every JSON answer (metadata, tokens, error bodies).
pub(crate) const MAX_JSON_BYTES: u64 = 1024 * 1024;
const API_TIMEOUT: Duration = Duration::from_secs(30);
const STALL_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) struct Http {
    /// Calls with a total time limit (metadata, tokens).
    api: ureq::Agent,
    /// Downloads: no total limit, but a read that stalls fails after 30 s.
    download: ureq::Agent,
}

pub(crate) struct Reply {
    pub status: u16,
    pub content_length: Option<u64>,
    body: ureq::Body,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Api,
    Download,
}

fn net_error(e: ureq::Error) -> DriveError {
    let text = match e {
        ureq::Error::Timeout(_) | ureq::Error::BodyStalled => "vypršel časový limit".to_owned(),
        ureq::Error::HostNotFound => "server nebyl nalezen (zkontrolujte připojení k internetu)".to_owned(),
        ureq::Error::ConnectionFailed => "nelze se připojit k serveru".to_owned(),
        ureq::Error::Io(io) => format!("chyba sítě ({})", io.kind()),
        ureq::Error::BodyExceedsLimit(_) => "odpověď serveru je větší, než je povoleno".to_owned(),
        ureq::Error::TooManyRedirects | ureq::Error::RedirectFailed => "příliš mnoho přesměrování".to_owned(),
        other => other.to_string(),
    };
    DriveError::Network(text)
}

pub(crate) fn io_to_net_error(e: &std::io::Error) -> DriveError {
    match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
            DriveError::Network("vypršel časový limit".into())
        }
        kind => DriveError::Network(format!("spojení bylo přerušeno ({kind})")),
    }
}

fn build_agent(direct: bool, kind: Kind) -> ureq::Agent {
    let tls = TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build();
    let mut builder = ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .tls_config(tls)
        .user_agent("CADViewer/2.0")
        .timeout_resolve(Some(Duration::from_secs(10)))
        .timeout_connect(Some(Duration::from_secs(15)));
    builder = match kind {
        Kind::Api => builder.timeout_global(Some(API_TIMEOUT)),
        Kind::Download => builder.timeout_per_read(Some(STALL_TIMEOUT)),
    };
    if direct {
        // Loopback test servers must never be sent through a proxy from the environment.
        builder = builder.proxy(None);
    }
    builder.build().new_agent()
}

fn is_google_api_host(host: &str) -> bool {
    host == "googleapis.com" || host.ends_with(".googleapis.com")
}

fn is_google_content_host(host: &str) -> bool {
    host == "googleusercontent.com" || host.ends_with(".googleusercontent.com")
}

fn secure_default_port(p: &UrlParts) -> bool {
    p.scheme == "https" && p.effective_port() == Some(443)
}

pub(crate) fn redirect_allowed(origin: &UrlParts, target: &UrlParts) -> bool {
    target.same_origin(origin)
        || (secure_default_port(target)
            && (is_google_api_host(&target.host) || is_google_content_host(&target.host)))
}

pub(crate) fn may_send_auth(origin: &UrlParts, target: &UrlParts) -> bool {
    target.same_origin(origin) || (secure_default_port(target) && is_google_api_host(&target.host))
}

/// Turns a `Location` header into an absolute URL. Only absolute URLs and `/path` are supported.
fn resolve_location(current: &UrlParts, location: &str) -> Option<String> {
    if location.starts_with("//") {
        return None;
    }
    if location.starts_with('/') {
        let host = if current.host.contains(':') {
            format!("[{}]", current.host)
        } else {
            current.host.clone()
        };
        let port = current.port.map(|p| format!(":{p}")).unwrap_or_default();
        return Some(format!("{}://{host}{port}{location}", current.scheme));
    }
    Some(location.to_owned())
}

impl Http {
    /// `direct` switches off the proxy from the environment. It is used when every endpoint is
    /// a loopback address (tests).
    pub fn new(direct: bool) -> Self {
        Http {
            api: build_agent(direct, Kind::Api),
            download: build_agent(direct, Kind::Download),
        }
    }

    fn agent(&self, kind: Kind) -> &ureq::Agent {
        match kind {
            Kind::Api => &self.api,
            Kind::Download => &self.download,
        }
    }

    /// GET with an optional bearer token. Follows allowed redirects.
    pub fn get(&self, kind: Kind, url: &str, bearer: Option<&Secret>) -> Result<Reply, DriveError> {
        let origin = split_url(url).ok_or_else(|| DriveError::BadConfig("neplatná adresa".into()))?;
        let mut current_url = url.to_owned();
        let mut current = origin.clone();
        for hop in 0..=MAX_REDIRECTS {
            let mut request = self.agent(kind).get(&current_url);
            if let Some(token) = bearer
                && may_send_auth(&origin, &current)
            {
                request = request.header("Authorization", format!("Bearer {}", token.expose()));
            }
            let response = request.call().map_err(net_error)?;
            let status = response.status().as_u16();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                if hop == MAX_REDIRECTS {
                    return Err(DriveError::Network("příliš mnoho přesměrování".into()));
                }
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or(DriveError::HttpStatus(status))?
                    .to_owned();
                let next_url = resolve_location(&current, &location)
                    .ok_or_else(|| DriveError::BadResponse("neplatné přesměrování".into()))?;
                let next = split_url(&next_url)
                    .ok_or_else(|| DriveError::BadResponse("neplatné přesměrování".into()))?;
                if !redirect_allowed(&origin, &next) {
                    return Err(DriveError::BadResponse("přesměrování na nedůvěryhodný server".into()));
                }
                current_url = next_url;
                current = next;
                continue;
            }
            return Ok(Reply::from_response(response));
        }
        Err(DriveError::Network("příliš mnoho přesměrování".into()))
    }

    /// POST of a form body to the OAuth token endpoint. Redirects are not followed.
    pub fn post_form(&self, url: &str, body: &str) -> Result<Reply, DriveError> {
        let response = self
            .api
            .post(url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send(body)
            .map_err(net_error)?;
        Ok(Reply::from_response(response))
    }
}

impl Reply {
    fn from_response(response: ureq::http::Response<ureq::Body>) -> Self {
        let status = response.status().as_u16();
        let (_, body) = response.into_parts();
        Reply { status, content_length: body.content_length(), body }
    }

    /// Reads a small answer completely. Fails if it is longer than `limit`.
    pub fn read_limited(mut self, limit: u64) -> Result<Vec<u8>, DriveError> {
        self.body.with_config().limit(limit).read_to_vec().map_err(net_error)
    }

    pub fn into_reader(self) -> impl Read {
        self.body.into_reader()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(u: &str) -> UrlParts {
        split_url(u).unwrap()
    }

    #[test]
    fn redirect_policy() {
        let origin = p("https://www.googleapis.com/drive/v3/files/x?alt=media");
        assert!(redirect_allowed(&origin, &p("https://www.googleapis.com/other")));
        assert!(redirect_allowed(&origin, &p("https://content.googleapis.com/x")));
        assert!(redirect_allowed(&origin, &p("https://doc-0g-0s-docs.googleusercontent.com/x")));
        assert!(!redirect_allowed(&origin, &p("http://www.googleapis.com/x")));
        assert!(!redirect_allowed(&origin, &p("https://evil.com/x")));
        assert!(!redirect_allowed(&origin, &p("https://googleapis.com.evil.com/x")));
        assert!(!redirect_allowed(&origin, &p("https://evilgoogleapis.com/x")));
        assert!(!redirect_allowed(&origin, &p("https://www.googleapis.com:8443/x")));
        assert!(!redirect_allowed(&origin, &p("https://127.0.0.1/x")));
    }

    #[test]
    fn auth_header_policy() {
        let origin = p("https://www.googleapis.com/drive/v3/files/x");
        assert!(may_send_auth(&origin, &origin));
        assert!(may_send_auth(&origin, &p("https://content.googleapis.com/x")));
        assert!(!may_send_auth(&origin, &p("https://doc.googleusercontent.com/x")));
        assert!(!may_send_auth(&origin, &p("https://evil.com/x")));
        let local = p("http://127.0.0.1:5000/drive/v3/files/x");
        assert!(may_send_auth(&local, &p("http://127.0.0.1:5000/other")));
        assert!(!may_send_auth(&local, &p("http://127.0.0.1:5001/other")));
        assert!(redirect_allowed(&local, &p("http://127.0.0.1:5000/other")));
        assert!(!redirect_allowed(&local, &p("http://127.0.0.1:5001/other")));
    }

    #[test]
    fn location_resolution() {
        let cur = p("http://127.0.0.1:5000/a/b");
        assert_eq!(resolve_location(&cur, "/x?y=1").unwrap(), "http://127.0.0.1:5000/x?y=1");
        assert_eq!(resolve_location(&cur, "https://a.com/x").unwrap(), "https://a.com/x");
        assert_eq!(resolve_location(&cur, "//evil.com/x"), None);
        let v6 = p("http://[::1]:5000/a");
        assert_eq!(resolve_location(&v6, "/x").unwrap(), "http://[::1]:5000/x");
    }
}
