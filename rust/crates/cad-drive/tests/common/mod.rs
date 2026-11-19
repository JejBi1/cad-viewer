//! Test helpers shared by the integration tests: a small HTTP server on 127.0.0.1 that plays
//! Google (token endpoint and Drive API), and a browser that plays the user.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cad_drive::{Browser, DriveConfig, DriveError, Endpoints, Secret, TokenStore};

pub const FILE_ID: &str = "1AbCdEfGhIjKlMnOpQrStUvWxYz_0123456-";
pub const MODIFIED: &str = "2026-01-15T10:30:00.123Z";

// ------------------------------------------------------------------------------------------
// HTTP server
// ------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn query_params(&self) -> HashMap<String, String> {
        parse_query(&self.query)
    }

    pub fn form(&self) -> HashMap<String, String> {
        parse_query(&String::from_utf8_lossy(&self.body))
    }
}

#[derive(Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Claim this many bytes in Content-Length but send only `body`.
    pub declared_len: Option<usize>,
    /// Send no Content-Length and end the body by closing the connection.
    pub close_delimited: bool,
}

impl Response {
    pub fn bytes(status: u16, body: Vec<u8>) -> Self {
        Response { status, headers: Vec::new(), body, declared_len: None, close_delimited: false }
    }

    pub fn json(status: u16, body: &str) -> Self {
        Self::bytes(status, body.as_bytes().to_vec()).header("Content-Type", "application/json")
    }

    pub fn empty(status: u16) -> Self {
        Self::bytes(status, Vec::new())
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn declared(mut self, len: usize) -> Self {
        self.declared_len = Some(len);
        self
    }

    pub fn close_delimited(mut self) -> Self {
        self.close_delimited = true;
        self
    }
}

pub struct MockServer {
    pub addr: SocketAddr,
    log: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
}

impl MockServer {
    pub fn start<F>(handler: F) -> Self
    where
        F: Fn(&Request) -> Response + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let handler = Arc::new(handler);
        {
            let (log, stop) = (log.clone(), stop.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let (log, handler) = (log.clone(), handler.clone());
                    std::thread::spawn(move || serve(stream, &*handler, &log));
                }
            });
        }
        MockServer { addr, log, stop }
    }

    pub fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn requests(&self) -> Vec<Request> {
        self.log.lock().unwrap().clone()
    }

    pub fn count(&self, pred: impl Fn(&Request) -> bool) -> usize {
        self.requests().iter().filter(|r| pred(r)).count()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr); // wakes the accept loop
    }
}

fn serve(stream: TcpStream, handler: &(dyn Fn(&Request) -> Response + Send + Sync), log: &Mutex<Vec<Request>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let target = parts.next().unwrap_or("/").to_owned();
    let (path, query) = target.split_once('?').map(|(p, q)| (p.to_owned(), q.to_owned())).unwrap_or((target, String::new()));
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    let len: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    let _ = reader.read_exact(&mut body);
    let request = Request { method, path, query, headers, body };
    log.lock().unwrap().push(request.clone());

    let response = handler(&request);
    let mut out = format!("HTTP/1.1 {} X\r\nConnection: close\r\n", response.status).into_bytes();
    for (k, v) in &response.headers {
        out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    if !response.close_delimited {
        out.extend_from_slice(format!("Content-Length: {}\r\n", response.declared_len.unwrap_or(response.body.len())).as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    let mut stream = stream;
    let _ = stream.write_all(&out);
    let _ = stream.write_all(&response.body);
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
    // Give the client time to read before the socket is closed.
    std::thread::sleep(Duration::from_millis(20));
}

// ------------------------------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------------------------------

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap();
                out.push(u8::from_str_radix(hex, 16).unwrap());
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap()
}

pub fn parse_query(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect()
}

/// A raw HTTP/1.1 GET to `http://127.0.0.1:port/...`, returns (status, body).
pub fn http_get(url: &str) -> (u16, String) {
    let rest = url.strip_prefix("http://").expect("http url");
    let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
    let mut s = TcpStream::connect(host).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").unwrap();
    let mut text = String::new();
    let _ = s.read_to_string(&mut text);
    let status = text.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_owned()).unwrap_or_default();
    (status, body)
}

pub fn config_for(server: &MockServer, with_secret: bool) -> DriveConfig {
    let base = server.base();
    let mut cfg = DriveConfig::new("test-client.apps.example")
        .unwrap()
        .with_endpoints(Endpoints {
            auth_url: format!("{base}/auth"),
            token_url: format!("{base}/token"),
            api_base: format!("{base}/drive/v3"),
        })
        .unwrap()
        .with_sign_in_timeout(Duration::from_secs(10));
    if with_secret {
        cfg = cfg.with_client_secret("test-client-secret");
    }
    cfg
}

pub const TOKEN_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";

pub fn token_json(access: &str, refresh: Option<&str>) -> String {
    let refresh = refresh.map(|r| format!(r#""refresh_token":"{r}","#)).unwrap_or_default();
    format!(
        r#"{{"access_token":"{access}",{refresh}"expires_in":3599,"scope":"{TOKEN_SCOPE}","token_type":"Bearer"}}"#
    )
}

/// A token store the test can look into after the session took ownership of a clone.
#[derive(Clone, Default)]
pub struct SharedStore(pub Arc<Mutex<Option<String>>>);

impl SharedStore {
    pub fn with(token: &str) -> Self {
        SharedStore(Arc::new(Mutex::new(Some(token.to_owned()))))
    }
    pub fn get(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }
}

impl TokenStore for SharedStore {
    fn load_refresh_token(&self) -> Result<Option<Secret>, DriveError> {
        Ok(self.get().map(Secret::new))
    }
    fn save_refresh_token(&self, token: &Secret) -> Result<(), DriveError> {
        *self.0.lock().unwrap() = Some(token.expose().to_owned());
        Ok(())
    }
    fn clear(&self) -> Result<(), DriveError> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}

/// Plays the user's browser: receives the authorization URL and runs a script on a thread.
pub struct ScriptedBrowser {
    pub opened: Mutex<Vec<String>>,
    script: Arc<dyn Fn(&str) + Send + Sync>,
}

impl ScriptedBrowser {
    pub fn new(script: impl Fn(&str) + Send + Sync + 'static) -> Self {
        ScriptedBrowser { opened: Mutex::new(Vec::new()), script: Arc::new(script) }
    }

    /// A browser that must not be used.
    pub fn forbidden() -> Self {
        Self::new(|_| panic!("the browser must not be opened"))
    }

    /// A browser that never answers.
    pub fn silent() -> Self {
        Self::new(|_| {})
    }

    pub fn open_count(&self) -> usize {
        self.opened.lock().unwrap().len()
    }
}

impl Browser for ScriptedBrowser {
    fn open_url(&self, url: &str) -> Result<(), DriveError> {
        self.opened.lock().unwrap().push(url.to_owned());
        // The listener is already bound, so the script may connect right away; it runs on its
        // own thread because the sign-in only starts accepting after this call returns.
        let url = url.to_owned();
        let script = self.script.clone();
        std::thread::spawn(move || script(&url));
        Ok(())
    }
}

/// Percent-encodes everything except unreserved characters.
pub fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Collects failed checks made on other threads, so the test can report them together.
#[derive(Clone, Default)]
pub struct Problems(pub Arc<Mutex<Vec<String>>>);

impl Problems {
    pub fn check(&self, ok: bool, what: impl Into<String>) {
        if !ok {
            self.0.lock().unwrap().push(what.into());
        }
    }
    pub fn assert_none(&self) {
        let p = self.0.lock().unwrap();
        assert!(p.is_empty(), "problems: {p:#?}");
    }
}

pub fn metadata_json(id: &str, name: &str, size: usize, md5: Option<&str>, modified: &str) -> String {
    let md5 = md5.map(|m| format!(r#","md5Checksum":"{m}""#)).unwrap_or_default();
    let name = serde_json::to_string(name).unwrap();
    format!(
        r#"{{"id":"{id}","name":{name},"size":"{size}","mimeType":"application/octet-stream","modifiedTime":"{modified}"{md5}}}"#
    )
}
