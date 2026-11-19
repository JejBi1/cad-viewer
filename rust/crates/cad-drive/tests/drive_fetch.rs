//! Metadata and download against a local mock of the Drive API. No real Google endpoint is used.

mod common;

use std::collections::HashSet;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use cad_drive::{CancelToken, DriveError, DriveFile, DriveSession, FetchRequest, Progress, parse_link};
use common::*;
use md5::{Digest, Md5};

const MB: u64 = 1024 * 1024;

#[derive(Clone, Copy, PartialEq)]
enum Md5Mode {
    Correct,
    Wrong,
    Absent,
}

#[derive(Clone, Copy, PartialEq)]
enum Media {
    Normal,
    /// More bytes than the metadata says, with no Content-Length.
    Longer,
    /// Content-Length is right but the connection ends halfway.
    Truncated,
    /// 302 to the same server once, then the content.
    RedirectSameOrigin,
}

#[derive(Clone)]
struct Spec {
    name: String,
    content: Vec<u8>,
    modified: String,
    md5: Md5Mode,
    response_id: Option<String>,
    media: Media,
    /// Size reported in the metadata, if different from the content length.
    reported_size: Option<usize>,
    mime: String,
    metadata_status: Option<u16>,
    media_status: Option<u16>,
    media_redirect_to: Option<String>,
}

impl Spec {
    fn new(name: &str, content: &[u8]) -> Self {
        Spec {
            name: name.to_owned(),
            content: content.to_vec(),
            modified: MODIFIED.to_owned(),
            md5: Md5Mode::Correct,
            response_id: None,
            media: Media::Normal,
            reported_size: None,
            mime: "application/octet-stream".into(),
            metadata_status: None,
            media_status: None,
            media_redirect_to: None,
        }
    }
}

fn md5_hex(data: &[u8]) -> String {
    Md5::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

struct Fixture {
    server: MockServer,
    spec: Arc<Mutex<Spec>>,
    store: SharedStore,
    cache: tempfile::TempDir,
    media_calls: Arc<AtomicUsize>,
    token_calls: Arc<AtomicUsize>,
}

impl Fixture {
    fn new(spec: Spec) -> Self {
        let spec = Arc::new(Mutex::new(spec));
        let media_calls = Arc::new(AtomicUsize::new(0));
        let token_calls = Arc::new(AtomicUsize::new(0));
        let server = {
            let (spec, media_calls, token_calls) = (spec.clone(), media_calls.clone(), token_calls.clone());
            MockServer::start(move |req| handle(req, &spec.lock().unwrap().clone(), &media_calls, &token_calls))
        };
        Fixture {
            server,
            spec,
            store: SharedStore::with("RT"),
            cache: tempfile::tempdir().unwrap(),
            media_calls,
            token_calls,
        }
    }

    fn session(&self) -> DriveSession {
        DriveSession::with_store(config_for(&self.server, false), Box::new(self.store.clone())).unwrap()
    }

    fn fetch_with(&self, session: &DriveSession, max: u64, events: &mut Vec<Progress>) -> Result<DriveFile, DriveError> {
        let link = parse_link(&format!("cadviewer://open?fileId={FILE_ID}")).unwrap();
        let request = FetchRequest { cache_dir: self.cache.path(), max_size: max, cancel: CancelToken::new() };
        session.fetch(&link, &request, &ScriptedBrowser::forbidden(), &mut |p| events.push(p))
    }

    fn fetch(&self, max: u64) -> Result<DriveFile, DriveError> {
        self.fetch_with(&self.session(), max, &mut Vec::new())
    }

    fn media_calls(&self) -> usize {
        self.media_calls.load(Ordering::SeqCst)
    }

    fn cache_entries(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.cache.path())
            .map(|d| d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        v.sort();
        v
    }
}

fn handle(req: &Request, spec: &Spec, media_calls: &AtomicUsize, token_calls: &AtomicUsize) -> Response {
    if req.path == "/token" {
        token_calls.fetch_add(1, Ordering::SeqCst);
        return Response::json(200, &token_json("AT", None));
    }
    let file_path = format!("/drive/v3/files/{FILE_ID}");
    if req.path != file_path {
        return Response::json(404, r#"{"error":{"code":404}}"#);
    }
    if req.method != "GET" {
        return Response::json(405, "{}");
    }
    if req.header("authorization") != Some("Bearer AT") {
        return Response::json(401, "{}");
    }
    let q = req.query_params();
    if q.get("alt").map(String::as_str) == Some("media") {
        media_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(status) = spec.media_status {
            return Response::json(status, r#"{"error":{"code":1,"errors":[{"reason":"forbidden"}]}}"#);
        }
        if let Some(target) = &spec.media_redirect_to {
            return Response::empty(302).header("Location", target);
        }
        if spec.media == Media::RedirectSameOrigin && !q.contains_key("redirected") {
            return Response::empty(302).header("Location", &format!("{file_path}?alt=media&redirected=1&supportsAllDrives=true"));
        }
        let data = &spec.content;
        return match spec.media {
            Media::Normal | Media::RedirectSameOrigin => Response::bytes(200, data.clone()),
            Media::Longer => {
                let mut longer = data.clone();
                longer.extend_from_slice(&[7u8; 4096]);
                Response::bytes(200, longer).close_delimited()
            }
            Media::Truncated => Response::bytes(200, data[..data.len() / 2].to_vec()).declared(data.len()),
        };
    }
    // Metadata. Only the documented fields are requested.
    let fields = q.get("fields").cloned().unwrap_or_default();
    if fields != "id,name,size,mimeType,modifiedTime,md5Checksum" {
        return Response::json(400, r#"{"error":{"code":400}}"#);
    }
    if let Some(status) = spec.metadata_status {
        return Response::json(status, r#"{"error":{"code":1,"errors":[{"reason":"notFound"}]}}"#);
    }
    let md5 = match spec.md5 {
        Md5Mode::Correct => Some(md5_hex(&spec.content)),
        Md5Mode::Wrong => Some("0123456789abcdef0123456789abcdef".to_owned()),
        Md5Mode::Absent => None,
    };
    let id = spec.response_id.clone().unwrap_or_else(|| FILE_ID.to_owned());
    let size = spec.reported_size.unwrap_or(spec.content.len());
    let mut body = metadata_json(&id, &spec.name, size, md5.as_deref(), &spec.modified);
    if spec.mime != "application/octet-stream" {
        body = body.replace("application/octet-stream", &spec.mime);
    }
    Response::json(200, &body)
}

fn step(content: &[u8]) -> Fixture {
    Fixture::new(Spec::new("Díl 1.STEP", content))
}

#[test]
fn downloads_into_cache_with_derived_name() {
    let content = b"ISO-10303-21;\nHEADER;\nENDSEC;\nEND-ISO-10303-21;\n";
    let fx = step(content);
    let mut events = Vec::new();
    let file = fx.fetch_with(&fx.session(), 10 * MB, &mut events).unwrap();

    let expected_name = format!("{FILE_ID}_20260115T103000123Z.step");
    assert_eq!(file.local_path, fx.cache.path().join(&expected_name));
    assert_eq!(file.display_name, "Díl 1.STEP");
    assert_eq!(file.file_id, FILE_ID);
    assert_eq!(file.size, content.len() as u64);
    assert_eq!(file.modified_time, MODIFIED);
    assert!(!file.from_cache);
    assert_eq!(fs::read(&file.local_path).unwrap(), content);
    // Only the final file is left, no temporary part file.
    assert_eq!(fx.cache_entries(), vec![expected_name]);
    // The modified time of the local copy is the Drive's.
    let mtime = fs::metadata(&file.local_path).unwrap().modified().unwrap().duration_since(UNIX_EPOCH).unwrap();
    assert_eq!(mtime.as_secs(), 1_768_473_000);
    assert_eq!(mtime.subsec_millis(), 123);

    assert_eq!(events.first(), Some(&Progress::ReadingMetadata));
    assert!(events.iter().any(|e| matches!(e, Progress::Downloading { total, .. } if *total == content.len() as u64)));
    assert_eq!(events.last(), Some(&Progress::Finished));
}

#[test]
fn only_get_requests_and_bearer_token_reach_drive() {
    let fx = step(b"abc");
    fx.fetch(MB).unwrap();
    let reqs = fx.server.requests();
    let drive: Vec<_> = reqs.iter().filter(|r| r.path.starts_with("/drive/")).collect();
    assert_eq!(drive.len(), 2, "metadata and media");
    for r in &drive {
        assert_eq!(r.method, "GET");
        assert_eq!(r.header("authorization"), Some("Bearer AT"));
        assert!(r.body.is_empty());
        assert!(r.query_params().contains_key("supportsAllDrives"));
    }
    assert_eq!(drive[0].query_params().get("alt"), None);
    assert_eq!(drive[1].query_params().get("alt").map(String::as_str), Some("media"));
    // The token endpoint is the only POST.
    assert!(reqs.iter().filter(|r| r.method != "GET").all(|r| r.path == "/token" && r.method == "POST"));
    // Only the file asked for was touched.
    let paths: HashSet<_> = drive.iter().map(|r| r.path.clone()).collect();
    assert_eq!(paths, HashSet::from([format!("/drive/v3/files/{FILE_ID}")]));
}

#[test]
fn drive_name_never_becomes_a_path() {
    for evil in [
        "..\\..\\Windows\\System32\\evil.STEP",
        "../../etc/cron.d/evil.stp",
        "C:\\autoexec.dwg",
        "con.step",
        "a\u{202E}pets.dxf",
        "dir/sub/part.IGES",
    ] {
        let fx = Fixture::new(Spec::new(evil, b"data"));
        let file = fx.fetch(MB).unwrap();
        assert_eq!(file.local_path.parent().unwrap(), fx.cache.path(), "{evil}");
        let name = file.local_path.file_name().unwrap().to_str().unwrap().to_owned();
        assert!(name.starts_with(FILE_ID), "{name}");
        assert!(!name.contains(['/', '\\', ':']), "{name}");
        assert_eq!(fx.cache_entries(), vec![name]);
        // The original name comes back as data, with control and bidi characters replaced.
        assert!(!file.display_name.contains('\u{202E}'));
        assert_eq!(file.display_name, cad_drive::sanitize_display_name(evil));
    }
}

#[test]
fn unsupported_extensions_are_refused_before_download() {
    for name in ["virus.exe", "plan.pdf", "model.step.exe", "noext", "archive.zip", ".step", "x.stepx"] {
        let fx = Fixture::new(Spec::new(name, b"data"));
        let err = fx.fetch(MB).unwrap_err();
        assert!(matches!(err, DriveError::UnsupportedType { .. }), "{name}: {err:?}");
        assert!(err.to_string().contains("nemá podporovanou příponu"));
        assert_eq!(fx.media_calls(), 0, "{name}");
        assert!(fx.cache_entries().is_empty());
    }
    for name in ["a.step", "a.stp", "a.iges", "a.igs", "a.stl", "a.dxf", "a.dwg", "A.DWG"] {
        let fx = Fixture::new(Spec::new(name, b"data"));
        fx.fetch(MB).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
fn size_limit_is_checked_from_metadata_and_while_streaming() {
    let fx = step(&vec![1u8; 1000]);
    let err = fx.fetch(999).unwrap_err();
    assert!(matches!(err, DriveError::TooLarge { .. }), "{err:?}");
    assert!(err.to_string().contains("příliš velký"));
    assert_eq!(fx.media_calls(), 0);
    assert!(fx.cache_entries().is_empty());
    // Exactly at the limit is fine.
    fx.fetch(1000).unwrap();

    // A server that sends more than it announced is stopped.
    let mut spec = Spec::new("a.step", &vec![2u8; 1000]);
    spec.media = Media::Longer;
    let fx = Fixture::new(spec);
    let err = fx.fetch(MB).unwrap_err();
    assert!(matches!(err, DriveError::BadResponse(_)), "{err:?}");
    assert!(fx.cache_entries().is_empty(), "{:?}", fx.cache_entries());

    // Metadata and Content-Length disagree: the file changed in between.
    let mut spec = Spec::new("a.step", &vec![3u8; 1000]);
    spec.reported_size = Some(900);
    let fx = Fixture::new(spec);
    assert!(matches!(fx.fetch(MB).unwrap_err(), DriveError::BadResponse(_)));
    assert!(fx.cache_entries().is_empty());
}

#[test]
fn incomplete_download_leaves_nothing_behind() {
    let mut spec = Spec::new("a.step", &vec![9u8; 100_000]);
    spec.media = Media::Truncated;
    let fx = Fixture::new(spec);
    assert!(fx.fetch(MB).is_err());
    assert!(fx.cache_entries().is_empty(), "{:?}", fx.cache_entries());
}

#[test]
fn checksum_is_verified() {
    let mut spec = Spec::new("a.step", b"some content");
    spec.md5 = Md5Mode::Wrong;
    let fx = Fixture::new(spec);
    let err = fx.fetch(MB).unwrap_err();
    assert_eq!(err, DriveError::ChecksumMismatch);
    assert!(err.to_string().contains("poškozený"));
    assert!(fx.cache_entries().is_empty());

    let mut spec = Spec::new("a.step", b"some content");
    spec.md5 = Md5Mode::Absent;
    Fixture::new(spec).fetch(MB).unwrap();
}

#[test]
fn up_to_date_copy_is_reused_and_a_new_version_is_downloaded() {
    let fx = step(b"version one");
    let session = fx.session();
    let mut events = Vec::new();
    let first = fx.fetch_with(&session, MB, &mut events).unwrap();
    assert!(!first.from_cache);
    assert_eq!(fx.media_calls(), 1);

    events.clear();
    let second = fx.fetch_with(&session, MB, &mut events).unwrap();
    assert!(second.from_cache);
    assert_eq!(second.local_path, first.local_path);
    assert_eq!(fx.media_calls(), 1, "no second download");
    assert!(events.contains(&Progress::UsingCachedCopy));
    assert!(!events.iter().any(|e| matches!(e, Progress::Downloading { .. })));
    // A new session (new program start) reuses it too, and gets a token without a browser.
    let third = fx.fetch_with(&fx.session(), MB, &mut Vec::new()).unwrap();
    assert!(third.from_cache);
    assert_eq!(fx.media_calls(), 1);

    // The file changes on the Drive: new modified time, new local name, new download.
    {
        let mut spec = fx.spec.lock().unwrap();
        spec.content = b"version two, longer".to_vec();
        spec.modified = "2026-02-01T08:00:00.000Z".into();
    }
    let fourth = fx.fetch_with(&session, MB, &mut Vec::new()).unwrap();
    assert!(!fourth.from_cache);
    assert_ne!(fourth.local_path, first.local_path);
    assert_eq!(fs::read(&fourth.local_path).unwrap(), b"version two, longer");
    assert_eq!(fs::read(&first.local_path).unwrap(), b"version one", "the old copy is not touched");
    assert_eq!(fx.media_calls(), 2);
}

#[test]
fn a_copy_with_the_wrong_size_is_replaced() {
    let fx = step(b"the real content");
    let name = format!("{FILE_ID}_20260115T103000123Z.step");
    fs::write(fx.cache.path().join(&name), b"partial").unwrap();
    let file = fx.fetch(MB).unwrap();
    assert!(!file.from_cache);
    assert_eq!(fs::read(&file.local_path).unwrap(), b"the real content");
    assert_eq!(fx.cache_entries(), vec![name]);
}

#[test]
fn cache_folder_is_created() {
    let fx = step(b"x");
    let nested = fx.cache.path().join("a").join("b");
    let link = parse_link(&format!("cadviewer://open?fileId={FILE_ID}")).unwrap();
    let request = FetchRequest { cache_dir: &nested, max_size: MB, cancel: CancelToken::new() };
    let file = fx.session().fetch(&link, &request, &ScriptedBrowser::forbidden(), &mut |_| {}).unwrap();
    assert!(file.local_path.starts_with(&nested));
    assert!(file.local_path.is_file());
}

#[test]
fn folders_and_google_documents_cannot_be_downloaded() {
    for mime in ["application/vnd.google-apps.folder", "application/vnd.google-apps.document", "application/vnd.google-apps.shortcut"] {
        let mut spec = Spec::new("Složka.step", b"x");
        spec.mime = mime.into();
        let fx = Fixture::new(spec);
        let err = fx.fetch(MB).unwrap_err();
        assert!(matches!(err, DriveError::NotDownloadable { .. }), "{mime}: {err:?}");
        assert_eq!(fx.media_calls(), 0);
    }
}

#[test]
fn response_for_another_file_is_rejected() {
    let mut spec = Spec::new("a.step", b"x");
    spec.response_id = Some("ZZZZZZZZZZZZZZZZZZZZZZ".into());
    let fx = Fixture::new(spec);
    assert!(matches!(fx.fetch(MB).unwrap_err(), DriveError::BadResponse(_)));
    assert_eq!(fx.media_calls(), 0);
}

#[test]
fn http_errors_map_to_czech_messages() {
    let cases = [
        (404, DriveError::NotFound),
        (403, DriveError::Forbidden),
        (429, DriveError::Unavailable(429)),
        (503, DriveError::Unavailable(503)),
    ];
    for (status, expected) in cases {
        let mut spec = Spec::new("a.step", b"x");
        spec.metadata_status = Some(status);
        let err = Fixture::new(spec).fetch(MB).unwrap_err();
        assert_eq!(err, expected, "{status}");
        assert!(!err.to_string().is_empty());
    }
    let mut spec = Spec::new("a.step", b"x");
    spec.media_status = Some(403);
    let fx = Fixture::new(spec);
    assert_eq!(fx.fetch(MB).unwrap_err(), DriveError::Forbidden);
    assert!(fx.cache_entries().is_empty());
    assert!(DriveError::NotFound.to_string().starts_with("Soubor nebyl na Google Drive nalezen"));
}

#[test]
fn same_origin_redirect_keeps_the_token() {
    let mut spec = Spec::new("a.step", b"redirected content");
    spec.media = Media::RedirectSameOrigin;
    let fx = Fixture::new(spec);
    let file = fx.fetch(MB).unwrap();
    assert_eq!(fs::read(&file.local_path).unwrap(), b"redirected content");
    let media: Vec<_> = fx.server.requests().into_iter().filter(|r| r.query_params().contains_key("alt")).collect();
    assert_eq!(media.len(), 2);
    assert!(media.iter().all(|r| r.header("authorization") == Some("Bearer AT")));
}

#[test]
fn redirect_to_another_origin_is_refused_and_the_token_is_not_sent() {
    let evil = MockServer::start(|_| Response::bytes(200, b"evil data".to_vec()));
    let mut spec = Spec::new("a.step", b"x");
    spec.media_redirect_to = Some(format!("{}/steal", evil.base()));
    let fx = Fixture::new(spec);
    let err = fx.fetch(MB).unwrap_err();
    assert!(matches!(err, DriveError::BadResponse(_)), "{err:?}");
    assert!(evil.requests().is_empty(), "the other server was contacted");
    assert!(fx.cache_entries().is_empty());

    // Plain http to a remote-looking host and other schemes are refused as well.
    for target in ["http://example.com/x", "ftp://example.com/x", "file:///C:/x", "//evil.example/x", "javascript:alert(1)"] {
        let mut spec = Spec::new("a.step", b"x");
        spec.media_redirect_to = Some(target.to_owned());
        assert!(Fixture::new(spec).fetch(MB).is_err(), "{target}");
    }
}

#[test]
fn download_can_be_cancelled_and_leaves_nothing() {
    let fx = step(&vec![5u8; 3 * MB as usize]);
    let cancel = CancelToken::new();
    let link = parse_link(&format!("cadviewer://open?fileId={FILE_ID}")).unwrap();
    let request = FetchRequest { cache_dir: fx.cache.path(), max_size: 10 * MB, cancel: cancel.clone() };
    let err = fx
        .session()
        .fetch(&link, &request, &ScriptedBrowser::forbidden(), &mut |p| {
            if matches!(p, Progress::Downloading { done, .. } if done > 0) {
                cancel.cancel();
            }
        })
        .unwrap_err();
    assert_eq!(err, DriveError::DownloadCancelled);
    assert_eq!(err.to_string(), "Stahování bylo přerušeno.");
    assert!(fx.cache_entries().is_empty(), "{:?}", fx.cache_entries());
}

#[test]
fn download_progress_is_monotonic_and_complete() {
    let fx = step(&vec![4u8; 2 * MB as usize + 17]);
    let mut events = Vec::new();
    fx.fetch_with(&fx.session(), 10 * MB, &mut events).unwrap();
    let totals: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            Progress::Downloading { done, total } => {
                assert_eq!(*total, 2 * MB + 17);
                Some(*done)
            }
            _ => None,
        })
        .collect();
    assert!(totals.len() >= 3);
    assert!(totals.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(*totals.last().unwrap(), 2 * MB + 17);
}

#[test]
fn sign_in_is_not_needed_twice_per_session() {
    let fx = step(b"x");
    let session = fx.session();
    fx.fetch_with(&session, MB, &mut Vec::new()).unwrap();
    // Everything after the first call uses the access token kept in memory.
    {
        let mut spec = fx.spec.lock().unwrap();
        spec.modified = "2027-01-01T00:00:00Z".into();
    }
    fx.fetch_with(&session, MB, &mut Vec::new()).unwrap();
    assert_eq!(fx.token_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn bad_link_values_never_reach_the_network() {
    // A DriveLink can only be made through validation, so a hostile id cannot get into a URL.
    for id in ["../../etc/passwd", "short", "id with space xxxxxxx", "a/b/c/d/e/f/g/h/i/j"] {
        assert!(cad_drive::DriveLink::new(id).is_err(), "{id}");
    }
    let fx = step(b"x");
    assert_eq!(fx.server.requests().len(), 0);
}
