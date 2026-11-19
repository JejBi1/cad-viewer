//! OAuth flow against a local mock server. No real Google endpoint is contacted.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cad_drive::{CancelToken, DriveError, DriveSession, challenge_for, parse_link};
use common::*;

#[derive(Default)]
struct Seen {
    challenge: String,
    redirect_uri: String,
    state: String,
}

fn link() -> cad_drive::DriveLink {
    parse_link(&format!("cadviewer://open?fileId={FILE_ID}")).unwrap()
}

fn session(server: &MockServer, store: &SharedStore, secret: bool) -> DriveSession {
    DriveSession::with_store(config_for(server, secret), Box::new(store.clone())).unwrap()
}

/// Drive API part of the mock: metadata for the test file, authorised with `token`.
fn metadata_response(req: &Request, token: &str) -> Response {
    if req.header("authorization") != Some(&format!("Bearer {token}")) {
        return Response::json(401, r#"{"error":{"code":401}}"#);
    }
    Response::json(200, &metadata_json(FILE_ID, "a.step", 3, None, MODIFIED))
}

/// What the user does in the browser: a few stray requests first, then the real redirect.
fn good_user(seen: Arc<Mutex<Seen>>, problems: Problems, code: &'static str) -> ScriptedBrowser {
    ScriptedBrowser::new(move |auth_url| {
        let (endpoint, query) = auth_url.split_once('?').unwrap();
        problems.check(endpoint.ends_with("/auth"), "auth endpoint");
        let q = parse_query(query);
        problems.check(q["response_type"] == "code", "response_type");
        problems.check(q["code_challenge_method"] == "S256", "method");
        problems.check(q["scope"] == "https://www.googleapis.com/auth/drive.file", "scope");
        problems.check(q["client_id"] == "test-client.apps.example", "client_id");
        problems.check(q["redirect_uri"].starts_with("http://127.0.0.1:"), "loopback redirect");
        problems.check(q["state"].len() >= 40, "state is long enough");
        problems.check(q["code_challenge"].len() == 43, "challenge length");
        {
            let mut s = seen.lock().unwrap();
            s.challenge = q["code_challenge"].clone();
            s.redirect_uri = q["redirect_uri"].clone();
            s.state = q["state"].clone();
        }
        let cb = q["redirect_uri"].clone();
        // Another path on the loopback port.
        let (status, _) = http_get(&cb.replace("/callback", "/other"));
        problems.check(status == 404, format!("other path gave {status}"));
        // Wrong or missing state must not finish or break the sign-in.
        let (status, _) = http_get(&format!("{cb}?code=EVIL&state=wrong"));
        problems.check(status == 400, format!("wrong state gave {status}"));
        let (status, _) = http_get(&format!("{cb}?code=EVIL"));
        problems.check(status == 400, format!("missing state gave {status}"));
        let (status, _) = http_get(&format!("{cb}?error=access_denied&state=wrong"));
        problems.check(status == 400, format!("error with wrong state gave {status}"));
        // The real answer, with a code that needs percent decoding.
        let (status, body) = http_get(&format!("{cb}?code={}&state={}", enc(code), enc(&q["state"])));
        problems.check(status == 200, format!("real callback gave {status}"));
        problems.check(body.contains("Přihlášení dokončeno"), "czech success page");
    })
}

#[test]
fn interactive_sign_in_with_pkce_state_and_secret() {
    const CODE: &str = "4/0AX4XfWh+code=&x";
    let seen = Arc::new(Mutex::new(Seen::default()));
    let problems = Problems::default();
    let token_calls = Arc::new(AtomicUsize::new(0));
    let server = {
        let (seen, problems, calls) = (seen.clone(), problems.clone(), token_calls.clone());
        MockServer::start(move |req| {
            if req.method == "POST" && req.path == "/token" {
                calls.fetch_add(1, Ordering::SeqCst);
                let f = req.form();
                let s = seen.lock().unwrap();
                problems.check(req.header("content-type") == Some("application/x-www-form-urlencoded"), "content type");
                problems.check(f["grant_type"] == "authorization_code", "grant_type");
                problems.check(f["code"] == CODE, format!("code {:?}", f["code"]));
                problems.check(f["client_id"] == "test-client.apps.example", "client_id");
                problems.check(f["client_secret"] == "test-client-secret", "client_secret");
                problems.check(f["redirect_uri"] == s.redirect_uri, "redirect_uri");
                // PKCE: the verifier sent now must hash to the challenge sent in the browser.
                problems.check(challenge_for(&f["code_verifier"]) == s.challenge, "pkce verifier matches challenge");
                problems.check(f["code_verifier"].len() >= 43, "verifier length");
                return Response::json(200, &token_json("AT-1", Some("RT-1")));
            }
            Response::empty(404)
        })
    };
    let store = SharedStore::default();
    let session = session(&server, &store, true);
    let browser = good_user(seen.clone(), problems.clone(), CODE);

    session.sign_in(&browser, &CancelToken::new()).unwrap();

    problems.assert_none();
    assert_eq!(token_calls.load(Ordering::SeqCst), 1);
    assert_eq!(browser.open_count(), 1);
    assert_eq!(store.get().as_deref(), Some("RT-1"));
    assert!(session.has_stored_sign_in());
    // The listener is closed after the sign-in: the port refuses connections.
    let redirect = seen.lock().unwrap().redirect_uri.clone();
    let addr = redirect.trim_start_matches("http://").split('/').next().unwrap().to_owned();
    assert!(std::net::TcpStream::connect(&addr).is_err(), "loopback listener still open");

    session.sign_out().unwrap();
    assert_eq!(store.get(), None);
}

#[test]
fn two_sign_ins_use_different_ports_state_and_verifier() {
    let seen_a = Arc::new(Mutex::new(Seen::default()));
    let seen_b = Arc::new(Mutex::new(Seen::default()));
    let problems = Problems::default();
    let server = MockServer::start(|req| {
        if req.path == "/token" {
            Response::json(200, &token_json("AT", Some("RT")))
        } else {
            Response::empty(404)
        }
    });
    for seen in [&seen_a, &seen_b] {
        let store = SharedStore::default();
        let session = session(&server, &store, false);
        session
            .sign_in(&good_user(seen.clone(), problems.clone(), "code"), &CancelToken::new())
            .unwrap();
    }
    problems.assert_none();
    let (a, b) = (seen_a.lock().unwrap(), seen_b.lock().unwrap());
    assert_ne!(a.state, b.state);
    assert_ne!(a.challenge, b.challenge);
    // Ports are random per sign-in. Two equal ports in a row are possible but very unlikely,
    // so only the state and the challenge are required to differ.
    assert!(a.redirect_uri.starts_with("http://127.0.0.1:") && b.redirect_uri.starts_with("http://127.0.0.1:"));
    // And the client secret is not sent when none is configured.
    let posts: Vec<_> = server.requests().into_iter().filter(|r| r.path == "/token").collect();
    assert_eq!(posts.len(), 2);
    assert!(posts.iter().all(|r| !r.form().contains_key("client_secret")));
}

#[test]
fn user_denies_access() {
    let token_calls = Arc::new(AtomicUsize::new(0));
    let server = {
        let calls = token_calls.clone();
        MockServer::start(move |req| {
            if req.path == "/token" {
                calls.fetch_add(1, Ordering::SeqCst);
            }
            Response::empty(404)
        })
    };
    let store = SharedStore::default();
    let session = session(&server, &store, false);
    let page = Arc::new(Mutex::new(String::new()));
    let browser = {
        let page = page.clone();
        ScriptedBrowser::new(move |url| {
            let q = parse_query(url.split_once('?').unwrap().1);
            let (_, body) = http_get(&format!("{}?error=access_denied&state={}", q["redirect_uri"], enc(&q["state"])));
            *page.lock().unwrap() = body;
        })
    };
    let err = session.sign_in(&browser, &CancelToken::new()).unwrap_err();
    assert_eq!(err, DriveError::AccessDenied);
    assert_eq!(err.to_string(), "Přístup ke Google Drive nebyl povolen.");
    assert_eq!(token_calls.load(Ordering::SeqCst), 0);
    std::thread::sleep(Duration::from_millis(100));
    assert!(page.lock().unwrap().contains("nebylo dokončeno"));
    assert_eq!(store.get(), None);
}

#[test]
fn hostile_requests_never_finish_the_sign_in() {
    let token_calls = Arc::new(AtomicUsize::new(0));
    let server = {
        let calls = token_calls.clone();
        MockServer::start(move |req| {
            if req.path == "/token" {
                calls.fetch_add(1, Ordering::SeqCst);
            }
            Response::json(200, &token_json("AT", Some("RT")))
        })
    };
    let cfg = config_for(&server, false).with_sign_in_timeout(Duration::from_millis(1200));
    let store = SharedStore::default();
    let session = DriveSession::with_store(cfg, Box::new(store.clone())).unwrap();
    let problems = Problems::default();
    let browser = {
        let problems = problems.clone();
        ScriptedBrowser::new(move |url| {
            let q = parse_query(url.split_once('?').unwrap().1);
            let cb = &q["redirect_uri"];
            for attempt in [
                "code=A&state=", "code=A&state=AAAA", "state=&code=A", "code=A", "error=access_denied", "code=&state=x",
            ] {
                let (status, _) = http_get(&format!("{cb}?{attempt}"));
                problems.check(status == 400, format!("{attempt} gave {status}"));
            }
            // An idle connection (browser pre-connect) is dropped after a short read timeout.
            let host = cb.trim_start_matches("http://").split('/').next().unwrap();
            let _idle = std::net::TcpStream::connect(host).unwrap();
            std::thread::sleep(Duration::from_millis(100));
        })
    };
    let started = Instant::now();
    let err = session.sign_in(&browser, &CancelToken::new()).unwrap_err();
    problems.assert_none();
    assert_eq!(err, DriveError::SignInTimeout);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(token_calls.load(Ordering::SeqCst), 0);
    assert_eq!(store.get(), None);
}

/// A browser that connects to the loopback port and sends the request head one byte at a time, never finishing it.
fn dripping_browser(stop: Arc<std::sync::atomic::AtomicBool>) -> ScriptedBrowser {
    ScriptedBrowser::new(move |url| {
        use std::io::Write;
        let q = parse_query(url.split_once('?').unwrap().1);
        let host = q["redirect_uri"].trim_start_matches("http://").split('/').next().unwrap().to_owned();
        let mut s = std::net::TcpStream::connect(host).unwrap();
        let head = b"GET /callback?code=A&state=x HTTP/1.1\r\nHost: x\r\nX-Pad: ";
        let pad = std::iter::repeat_n(b'a', 100_000);
        let started = Instant::now();
        for b in head.iter().copied().chain(pad) {
            // the drip itself is bounded, so a regression ends the test instead of hanging it
            if stop.load(Ordering::SeqCst) || started.elapsed() > Duration::from_secs(8) || s.write_all(&[b]).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    })
}

#[test]
fn a_client_that_drips_bytes_cannot_outlast_the_sign_in_deadline() {
    let server = MockServer::start(|_| Response::empty(404));
    let cfg = config_for(&server, false).with_sign_in_timeout(Duration::from_millis(700));
    let session = DriveSession::with_store(cfg, Box::new(SharedStore::default())).unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let started = Instant::now();
    let err = session.sign_in(&dripping_browser(stop.clone()), &CancelToken::new()).unwrap_err();
    let elapsed = started.elapsed();
    stop.store(true, Ordering::SeqCst);
    assert_eq!(err, DriveError::SignInTimeout);
    assert!(elapsed < Duration::from_millis(2500), "took {elapsed:?}");
}

#[test]
fn cancel_is_honoured_while_a_client_drips_bytes() {
    let server = MockServer::start(|_| Response::empty(404));
    let session = session(&server, &SharedStore::default(), false);
    let cancel = CancelToken::new();
    {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            cancel.cancel();
        });
    }
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let started = Instant::now();
    let err = session.sign_in(&dripping_browser(stop.clone()), &cancel).unwrap_err();
    let elapsed = started.elapsed();
    stop.store(true, Ordering::SeqCst);
    assert_eq!(err, DriveError::Cancelled);
    assert!(elapsed < Duration::from_millis(1200), "took {elapsed:?}");
}

#[test]
fn sign_in_times_out() {
    let server = MockServer::start(|_| Response::empty(404));
    let cfg = config_for(&server, false).with_sign_in_timeout(Duration::from_millis(300));
    let session = DriveSession::with_store(cfg, Box::new(SharedStore::default())).unwrap();
    let started = Instant::now();
    let err = session.sign_in(&ScriptedBrowser::silent(), &CancelToken::new()).unwrap_err();
    assert_eq!(err, DriveError::SignInTimeout);
    assert_eq!(err.to_string(), "Přihlášení ke Google Drive vypršelo. Zkuste to prosím znovu.");
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn sign_in_can_be_cancelled() {
    let server = MockServer::start(|_| Response::empty(404));
    let store = SharedStore::default();
    let session = session(&server, &store, false);
    let cancel = CancelToken::new();
    {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            cancel.cancel();
        });
    }
    let started = Instant::now();
    let err = session.sign_in(&ScriptedBrowser::silent(), &cancel).unwrap_err();
    assert_eq!(err, DriveError::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn browser_failure_is_reported() {
    struct Broken;
    impl cad_drive::Browser for Broken {
        fn open_url(&self, _url: &str) -> Result<(), DriveError> {
            Err(DriveError::BrowserFailed("není žádný prohlížeč".into()))
        }
    }
    let server = MockServer::start(|_| Response::empty(404));
    let session = session(&server, &SharedStore::default(), false);
    let err = session.sign_in(&Broken, &CancelToken::new()).unwrap_err();
    assert!(err.to_string().starts_with("Nepodařilo se otevřít webový prohlížeč"));
}

#[test]
fn missing_scope_is_rejected_and_nothing_is_stored() {
    let server = MockServer::start(|req| {
        if req.path == "/token" {
            Response::json(
                200,
                r#"{"access_token":"AT-SECRET-VALUE","refresh_token":"RT-SECRET-VALUE","expires_in":3599,"scope":"https://www.googleapis.com/auth/drive.readonly","token_type":"Bearer"}"#,
            )
        } else {
            Response::empty(404)
        }
    });
    let store = SharedStore::default();
    let session = session(&server, &store, false);
    let problems = Problems::default();
    let browser = good_user(Arc::new(Mutex::new(Seen::default())), problems, "code");
    let err = session.sign_in(&browser, &CancelToken::new()).unwrap_err();
    assert_eq!(err, DriveError::ScopeMissing);
    let text = format!("{err} {err:?}");
    assert!(!text.contains("SECRET-VALUE"), "{text}");
    assert_eq!(store.get(), None);
}

#[test]
fn token_endpoint_errors_are_czech_and_do_not_leak() {
    let server = MockServer::start(|req| {
        if req.path == "/token" {
            Response::json(503, r#"{"error":"backend_error","access_token":"LEAK-ME"}"#)
        } else {
            Response::empty(404)
        }
    });
    let session = session(&server, &SharedStore::default(), false);
    let browser = good_user(Arc::new(Mutex::new(Seen::default())), Problems::default(), "code");
    let err = session.sign_in(&browser, &CancelToken::new()).unwrap_err();
    assert!(matches!(err, DriveError::SignInFailed(_) | DriveError::Unavailable(_)), "{err:?}");
    assert!(!format!("{err} {err:?}").contains("LEAK-ME"));
}

#[test]
fn stored_refresh_token_is_used_without_a_browser() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let server = {
        let calls = calls.clone();
        MockServer::start(move |req| match req.path.as_str() {
            "/token" => {
                calls.lock().unwrap().push(req.form());
                // A refresh answer normally has no new refresh token.
                Response::json(200, &token_json("AT-2", None))
            }
            p if p == format!("/drive/v3/files/{FILE_ID}") => metadata_response(req, "AT-2"),
            _ => Response::empty(404),
        })
    };
    let store = SharedStore::with("RT-OLD");
    let session = session(&server, &store, true);
    let browser = ScriptedBrowser::forbidden();
    let meta = session
        .metadata(&link(), &browser, &CancelToken::new(), &mut |_| {})
        .unwrap();
    assert_eq!(meta.name, "a.step");
    // A second call reuses the access token in memory: no second token request.
    session.metadata(&link(), &browser, &CancelToken::new(), &mut |_| {}).unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["grant_type"], "refresh_token");
    assert_eq!(calls[0]["refresh_token"], "RT-OLD");
    assert_eq!(calls[0]["client_id"], "test-client.apps.example");
    assert_eq!(calls[0]["client_secret"], "test-client-secret");
    assert_eq!(store.get().as_deref(), Some("RT-OLD"));
    assert_eq!(browser.open_count(), 0);
}

#[test]
fn rotated_refresh_token_replaces_the_stored_one() {
    let server = MockServer::start(|req| match req.path.as_str() {
        "/token" => Response::json(200, &token_json("AT-3", Some("RT-NEW"))),
        p if p == format!("/drive/v3/files/{FILE_ID}") => metadata_response(req, "AT-3"),
        _ => Response::empty(404),
    });
    let store = SharedStore::with("RT-OLD");
    session(&server, &store, false)
        .metadata(&link(), &ScriptedBrowser::forbidden(), &CancelToken::new(), &mut |_| {})
        .unwrap();
    assert_eq!(store.get().as_deref(), Some("RT-NEW"));
}

#[test]
fn revoked_refresh_token_falls_back_to_interactive_sign_in() {
    let problems = Problems::default();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let grants = Arc::new(Mutex::new(Vec::new()));
    let server = {
        let (grants, seen, problems) = (grants.clone(), seen.clone(), problems.clone());
        MockServer::start(move |req| match req.path.as_str() {
            "/token" => {
                let f = req.form();
                grants.lock().unwrap().push(f["grant_type"].clone());
                if f["grant_type"] == "refresh_token" {
                    return Response::json(400, r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#);
                }
                problems.check(
                    challenge_for(&f["code_verifier"]) == seen.lock().unwrap().challenge,
                    "pkce in fallback",
                );
                Response::json(200, &token_json("AT-9", Some("RT-9")))
            }
            p if p == format!("/drive/v3/files/{FILE_ID}") => metadata_response(req, "AT-9"),
            _ => Response::empty(404),
        })
    };
    let store = SharedStore::with("RT-REVOKED");
    let browser = good_user(seen.clone(), problems.clone(), "code");
    let meta = session(&server, &store, false)
        .metadata(&link(), &browser, &CancelToken::new(), &mut |_| {})
        .unwrap();
    assert_eq!(meta.id, FILE_ID);
    problems.assert_none();
    assert_eq!(*grants.lock().unwrap(), vec!["refresh_token", "authorization_code"]);
    assert_eq!(browser.open_count(), 1);
    assert_eq!(store.get().as_deref(), Some("RT-9"));
}

#[test]
fn refresh_failure_other_than_revocation_does_not_open_the_browser() {
    let server = MockServer::start(|req| {
        if req.path == "/token" { Response::empty(503) } else { Response::empty(404) }
    });
    let store = SharedStore::with("RT-KEEP");
    let err = session(&server, &store, false)
        .metadata(&link(), &ScriptedBrowser::forbidden(), &CancelToken::new(), &mut |_| {})
        .unwrap_err();
    assert_eq!(err, DriveError::Unavailable(503));
    assert_eq!(store.get().as_deref(), Some("RT-KEEP"));
}

#[test]
fn expired_access_token_is_renewed_once_after_401() {
    let api_calls = Arc::new(AtomicUsize::new(0));
    let token_calls = Arc::new(AtomicUsize::new(0));
    let server = {
        let (api_calls, token_calls) = (api_calls.clone(), token_calls.clone());
        MockServer::start(move |req| match req.path.as_str() {
            "/token" => {
                let n = token_calls.fetch_add(1, Ordering::SeqCst);
                Response::json(200, &token_json(if n == 0 { "AT-OLD" } else { "AT-NEW" }, None))
            }
            p if p == format!("/drive/v3/files/{FILE_ID}") => {
                api_calls.fetch_add(1, Ordering::SeqCst);
                metadata_response(req, "AT-NEW")
            }
            _ => Response::empty(404),
        })
    };
    let store = SharedStore::with("RT");
    session(&server, &store, false)
        .metadata(&link(), &ScriptedBrowser::forbidden(), &CancelToken::new(), &mut |_| {})
        .unwrap();
    assert_eq!(token_calls.load(Ordering::SeqCst), 2);
    assert_eq!(api_calls.load(Ordering::SeqCst), 2);
}

#[test]
fn persistent_401_is_reported_after_one_retry() {
    let api_calls = Arc::new(AtomicUsize::new(0));
    let server = {
        let api_calls = api_calls.clone();
        MockServer::start(move |req| match req.path.as_str() {
            "/token" => Response::json(200, &token_json("AT", None)),
            p if p == format!("/drive/v3/files/{FILE_ID}") => {
                api_calls.fetch_add(1, Ordering::SeqCst);
                Response::json(401, "{}")
            }
            _ => Response::empty(404),
        })
    };
    let err = session(&server, &SharedStore::with("RT"), false)
        .metadata(&link(), &ScriptedBrowser::forbidden(), &CancelToken::new(), &mut |_| {})
        .unwrap_err();
    assert_eq!(err, DriveError::Unauthorized);
    assert_eq!(api_calls.load(Ordering::SeqCst), 2);
}
