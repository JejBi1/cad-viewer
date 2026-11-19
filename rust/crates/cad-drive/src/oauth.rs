//! OAuth 2.0 for desktop apps (RFC 8252): authorization code with PKCE (S256), the system
//! browser, and a redirect to a loopback address with a random port.
//!
//! * the listener is bound to 127.0.0.1 only and lives just for the sign-in;
//! * the `state` value (32 random bytes) must match, otherwise the request is ignored, so a
//!   stray or hostile request to the port cannot finish or break the sign-in;
//! * a timeout and a cancel flag end the wait;
//! * only the scope `drive.file` is requested and the granted scope is verified.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::config::{DriveConfig, SCOPE};
use crate::error::DriveError;
use crate::http::{Http, MAX_JSON_BYTES};
use crate::pkce::{Pkce, random_token};
use crate::secret::Secret;
use crate::urlutil::{constant_time_eq, form_body, parse_query, percent_encode, split_url};

/// Path of the loopback redirect.
pub const CALLBACK_PATH: &str = "/callback";

/// Lets the caller stop a sign-in or a download from another thread.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Opens a URL for the user. The application can supply its own (for example one that shows a
/// message first); [`SystemBrowser`] starts the default browser.
pub trait Browser: Send + Sync {
    fn open_url(&self, url: &str) -> Result<(), DriveError>;
}

/// The default browser of the user.
pub struct SystemBrowser;

impl Browser for SystemBrowser {
    fn open_url(&self, url: &str) -> Result<(), DriveError> {
        // Only plain web addresses, never a file path or another scheme.
        if split_url(url).is_none() {
            return Err(DriveError::BrowserFailed("neplatná adresa".into()));
        }
        open::that_detached(url).map_err(|e| DriveError::BrowserFailed(e.to_string()))
    }
}

/// Answer of the token endpoint.
pub(crate) struct Tokens {
    pub access: Secret,
    pub refresh: Option<Secret>,
    pub expires_in: Duration,
}

#[derive(Deserialize)]
struct TokenJson {
    access_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
}

#[derive(Deserialize)]
struct OAuthErrorJson {
    #[serde(default)]
    error: Option<String>,
}

pub(crate) fn build_auth_url(config: &DriveConfig, redirect_uri: &str, state: &str, challenge: &str) -> String {
    let params = [
        ("client_id", config.client_id.as_str()),
        ("redirect_uri", redirect_uri),
        ("response_type", "code"),
        ("scope", SCOPE),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
        // A refresh token is only issued together with the consent screen.
        ("access_type", "offline"),
        ("prompt", "consent"),
    ];
    let query = params
        .iter()
        .map(|(k, v)| format!("{k}={}", percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let sep = if config.endpoints.auth_url.contains('?') { '&' } else { '?' };
    format!("{}{sep}{query}", config.endpoints.auth_url)
}

/// Runs the whole interactive sign-in and returns the tokens.
pub(crate) fn sign_in(
    config: &DriveConfig,
    http: &Http,
    browser: &dyn Browser,
    cancel: &CancelToken,
) -> Result<Tokens, DriveError> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|e| DriveError::Loopback(e.to_string()))?;
    let port = listener
        .local_addr()
        .map_err(|e| DriveError::Loopback(e.to_string()))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
    let state = random_token(32)?;
    let pkce = Pkce::generate()?;
    let auth_url = build_auth_url(config, &redirect_uri, &state, &pkce.challenge);

    let deadline = Instant::now() + config.sign_in_timeout;
    browser.open_url(&auth_url)?;
    let code = wait_for_callback(&listener, &state, deadline, cancel)?;
    drop(listener);

    let mut pairs: Vec<(&str, &str)> = vec![
        ("client_id", config.client_id.as_str()),
        ("code", code.expose()),
        ("code_verifier", pkce.verifier.expose()),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri.as_str()),
    ];
    if let Some(secret) = &config.client_secret {
        pairs.push(("client_secret", secret.expose()));
    }
    let tokens = token_request(config, http, &pairs, true)?;
    Ok(tokens)
}

/// Gets a new access token from the stored refresh token.
/// `invalid_grant` (revoked or expired) is reported as [`DriveError::SignInRequired`].
pub(crate) fn refresh(config: &DriveConfig, http: &Http, refresh_token: &Secret) -> Result<Tokens, DriveError> {
    let mut pairs: Vec<(&str, &str)> = vec![
        ("client_id", config.client_id.as_str()),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.expose()),
    ];
    if let Some(secret) = &config.client_secret {
        pairs.push(("client_secret", secret.expose()));
    }
    token_request(config, http, &pairs, false)
}

fn token_request(
    config: &DriveConfig,
    http: &Http,
    pairs: &[(&str, &str)],
    require_scope: bool,
) -> Result<Tokens, DriveError> {
    let body = form_body(pairs);
    let reply = http.post_form(&config.endpoints.token_url, &body)?;
    let status = reply.status;
    let bytes = reply.read_limited(MAX_JSON_BYTES)?;
    if status != 200 {
        return Err(map_oauth_error(status, &bytes, !require_scope));
    }
    let json: TokenJson = serde_json::from_slice(&bytes)
        .map_err(|_| DriveError::BadResponse("odpověď přihlašovacího serveru nelze přečíst".into()))?;
    if json.access_token.is_empty() {
        return Err(DriveError::BadResponse("chybí přístupový token".into()));
    }
    if let Some(t) = &json.token_type
        && !t.eq_ignore_ascii_case("bearer")
    {
        return Err(DriveError::BadResponse("neočekávaný typ tokenu".into()));
    }
    match &json.scope {
        Some(granted) => {
            if !granted.split_whitespace().any(|s| s == SCOPE) {
                return Err(DriveError::ScopeMissing);
            }
        }
        None if require_scope => return Err(DriveError::ScopeMissing),
        None => {}
    }
    Ok(Tokens {
        access: Secret::new(json.access_token),
        refresh: json.refresh_token.filter(|t| !t.is_empty()).map(Secret::new),
        // Without a lifetime assume the shortest one Google uses in practice.
        expires_in: Duration::from_secs(json.expires_in.unwrap_or(300)),
    })
}

fn map_oauth_error(status: u16, body: &[u8], refreshing: bool) -> DriveError {
    let code = serde_json::from_slice::<OAuthErrorJson>(body)
        .ok()
        .and_then(|j| j.error)
        .filter(|c| c.len() <= 64 && c.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
    match code.as_deref() {
        Some("invalid_grant") if refreshing => DriveError::SignInRequired,
        Some("access_denied") => DriveError::AccessDenied,
        Some("invalid_client") | Some("unauthorized_client") => DriveError::BadConfig(
            "Google odmítl identifikátor klienta OAuth této aplikace".into(),
        ),
        Some(other) => DriveError::SignInFailed(other.to_owned()),
        None if status >= 500 => DriveError::Unavailable(status),
        None => DriveError::HttpStatus(status),
    }
}

enum Callback {
    Code(Secret),
    Failed(String),
    Ignore,
}

fn wait_for_callback(
    listener: &TcpListener,
    expected_state: &str,
    deadline: Instant,
    cancel: &CancelToken,
) -> Result<Secret, DriveError> {
    listener
        .set_nonblocking(true)
        .map_err(|e| DriveError::Loopback(e.to_string()))?;
    loop {
        if cancel.is_cancelled() {
            return Err(DriveError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(DriveError::SignInTimeout);
        }
        match listener.accept() {
            Ok((stream, _)) => match handle_connection(stream, expected_state, deadline, cancel) {
                Callback::Code(code) => return Ok(code),
                Callback::Failed(reason) => {
                    return Err(if reason == "access_denied" {
                        DriveError::AccessDenied
                    } else {
                        DriveError::SignInFailed(reason)
                    });
                }
                Callback::Ignore => {}
            },
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(DriveError::Loopback(e.to_string())),
        }
    }
}

const PAGE_OK: &str = "<!doctype html><html lang=\"cs\"><head><meta charset=\"utf-8\"><title>CAD prohlížeč</title></head>\
<body><h1>Přihlášení dokončeno</h1><p>Toto okno můžete zavřít a vrátit se do CAD prohlížeče.</p></body></html>";
const PAGE_DENIED: &str = "<!doctype html><html lang=\"cs\"><head><meta charset=\"utf-8\"><title>CAD prohlížeč</title></head>\
<body><h1>Přihlášení nebylo dokončeno</h1><p>Přístup ke Google Drive nebyl povolen. Toto okno můžete zavřít.</p></body></html>";
const PAGE_BAD: &str = "<!doctype html><html lang=\"cs\"><head><meta charset=\"utf-8\"><title>CAD prohlížeč</title></head>\
<body><h1>Neplatný požadavek</h1><p>Tato adresa slouží jen k dokončení přihlášení z CAD prohlížeče.</p></body></html>";

fn respond(stream: &mut TcpStream, status: &str, page: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\n\
Content-Security-Policy: default-src 'none'\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

/// How long one connection may take to deliver its request head. A browser sends it in one packet, so this only
/// limits clients that drip bytes; the sign-in deadline and the cancel flag cut it short as well.
const CONNECTION_BUDGET: Duration = Duration::from_millis(1500);
/// Longest single wait in a read, so a cancel is noticed quickly.
const READ_SLICE: Duration = Duration::from_millis(100);

fn handle_connection(mut stream: TcpStream, expected_state: &str, deadline: Instant, cancel: &CancelToken) -> Callback {
    // Accepted sockets may inherit the non-blocking mode of the listener.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let give_up = (Instant::now() + CONNECTION_BUDGET).min(deadline);

    let mut buf = [0u8; 8192];
    let mut len = 0;
    loop {
        // The whole head must arrive within the budget, however slowly the client sends it.
        let now = Instant::now();
        if cancel.is_cancelled() || now >= give_up {
            return Callback::Ignore;
        }
        let _ = stream.set_read_timeout(Some((give_up - now).min(READ_SLICE)));
        match stream.read(&mut buf[len..]) {
            Ok(0) => break,
            Ok(n) => {
                len += n;
                if buf[..len].windows(4).any(|w| w == b"\r\n\r\n") || len == buf.len() {
                    break;
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
            // A connection that sends nothing (browser pre-connect) or fails is dropped.
            Err(_) => return Callback::Ignore,
        }
    }
    let head = String::from_utf8_lossy(&buf[..len]);
    let Some(line) = head.lines().next() else {
        return Callback::Ignore;
    };
    let mut parts = line.split(' ');
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if method != "GET" {
        respond(&mut stream, "405 Method Not Allowed", PAGE_BAD);
        return Callback::Ignore;
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != CALLBACK_PATH {
        respond(&mut stream, "404 Not Found", PAGE_BAD);
        return Callback::Ignore;
    }
    let params = parse_query(query);
    let get = |name: &str| params.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
    let state_ok = get("state").is_some_and(|s| constant_time_eq(s, expected_state));
    if !state_ok {
        respond(&mut stream, "400 Bad Request", PAGE_BAD);
        return Callback::Ignore;
    }
    if let Some(error) = get("error") {
        respond(&mut stream, "200 OK", PAGE_DENIED);
        let clean: String = error
            .chars()
            .filter(|c| c.is_ascii_lowercase() || *c == '_')
            .take(64)
            .collect();
        return Callback::Failed(if clean.is_empty() { "neznámá chyba".into() } else { clean });
    }
    match get("code").filter(|c| !c.is_empty() && c.len() <= 2048) {
        Some(code) => {
            respond(&mut stream, "200 OK", PAGE_OK);
            Callback::Code(Secret::new(code))
        }
        None => {
            respond(&mut stream, "400 Bad Request", PAGE_BAD);
            Callback::Ignore
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urlutil::parse_query;

    #[test]
    fn auth_url_has_required_parameters() {
        let cfg = DriveConfig::new("client-1.apps.example").unwrap();
        let url = build_auth_url(&cfg, "http://127.0.0.1:5555/callback", "st&te", "chal");
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        let q = parse_query(url.split_once('?').unwrap().1);
        let get = |k: &str| q.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap();
        assert_eq!(get("client_id"), "client-1.apps.example");
        assert_eq!(get("redirect_uri"), "http://127.0.0.1:5555/callback");
        assert_eq!(get("response_type"), "code");
        assert_eq!(get("scope"), "https://www.googleapis.com/auth/drive.file");
        assert_eq!(get("code_challenge"), "chal");
        assert_eq!(get("code_challenge_method"), "S256");
        assert_eq!(get("state"), "st&te");
        assert_eq!(get("access_type"), "offline");
        assert_eq!(q.iter().filter(|(k, _)| k == "scope").count(), 1);
    }

    #[test]
    fn system_browser_refuses_anything_but_web_addresses() {
        // None of these reaches the operating system: they are refused before a program starts.
        for url in [
            "file:///C:/Windows/System32/calc.exe", "calc.exe", "javascript:alert(1)", "ms-settings:", "C:\\x.exe",
            "ftp://example.com/x", "", "https://", "https://exa mple.com/",
        ] {
            assert!(matches!(SystemBrowser.open_url(url), Err(DriveError::BrowserFailed(_))), "{url:?}");
        }
    }

    #[test]
    fn cancel_token_is_shared_between_clones() {
        let a = CancelToken::new();
        let b = a.clone();
        assert!(!b.is_cancelled());
        a.cancel();
        assert!(b.is_cancelled());
    }

    #[test]
    fn oauth_errors_map_to_czech_variants() {
        let b = br#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#;
        assert_eq!(map_oauth_error(400, b, true), DriveError::SignInRequired);
        assert_eq!(map_oauth_error(400, b, false), DriveError::SignInFailed("invalid_grant".into()));
        assert_eq!(
            map_oauth_error(400, br#"{"error":"access_denied"}"#, false),
            DriveError::AccessDenied
        );
        assert!(matches!(
            map_oauth_error(401, br#"{"error":"invalid_client"}"#, false),
            DriveError::BadConfig(_)
        ));
        assert_eq!(map_oauth_error(503, b"<html>", false), DriveError::Unavailable(503));
        assert_eq!(map_oauth_error(418, b"", false), DriveError::HttpStatus(418));
        // Odd error codes are not echoed.
        assert_eq!(map_oauth_error(400, br#"{"error":"X<script>"}"#, false), DriveError::HttpStatus(400));
    }
}
