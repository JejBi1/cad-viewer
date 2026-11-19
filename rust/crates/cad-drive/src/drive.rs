//! Google Drive API v3, read only.
//!
//! The only requests made here are `files.get` for metadata and `files.get?alt=media` for the
//! content, both with HTTP GET. There is no code path that creates, changes, copies, moves,
//! trashes, shares or deletes anything on the Drive.
//!
//! The local file name never comes from the Drive file name. It is built from the file id
//! (validated by the link rule) and the modified time (validated as a UTC timestamp), plus one
//! extension from a fixed list. The Drive name is only returned as display text.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use md5::{Digest, Md5};
use serde::Deserialize;

use crate::error::{DriveError, format_size};
use crate::http::{Http, Kind, MAX_JSON_BYTES, Reply, io_to_net_error};
use crate::link::is_valid_file_id;
use crate::oauth::CancelToken;
use crate::pkce::random_token;
use crate::secret::Secret;

/// File types the viewer can load. The extension of the Drive name must be one of these.
pub const SUPPORTED_EXTENSIONS: [&str; 7] = ["step", "stp", "iges", "igs", "stl", "dxf", "dwg"];

/// The `fields` parameter of `files.get`: exactly what is needed, nothing more.
const METADATA_FIELDS: &str = "id,name,size,mimeType,modifiedTime,md5Checksum";

/// Mime types of Google's own documents, folders and shortcuts. They have no binary content.
const GOOGLE_APPS_MIME_PREFIX: &str = "application/vnd.google-apps.";

/// Metadata of one Drive file as far as this crate needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMetadata {
    pub id: String,
    /// Display name with control and bidirectional characters replaced. Never used as a path.
    pub name: String,
    /// Absent for folders and Google documents.
    pub size: Option<u64>,
    pub mime_type: String,
    /// RFC 3339 UTC, for example `2026-01-15T10:30:00.123Z`.
    pub modified_time: String,
    /// Lower-case hex MD5, present for binary files.
    pub md5_checksum: Option<String>,
}

/// Returns the lower-case extension (without dot) if the name ends in a supported one.
pub fn supported_extension(name: &str) -> Option<&'static str> {
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    let ext = ext.to_ascii_lowercase();
    SUPPORTED_EXTENSIONS.iter().copied().find(|e| *e == ext)
}

/// Replaces control characters and bidirectional formatting characters (which can make an
/// extension look different from what it is) and limits the length to 255 characters.
pub fn sanitize_display_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            let bidi = matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}');
            if c.is_control() || bidi { '_' } else { c }
        })
        .take(255)
        .collect();
    if cleaned.trim().is_empty() { "soubor".to_owned() } else { cleaned }
}

#[derive(Deserialize)]
struct MetadataJson {
    id: String,
    name: String,
    #[serde(default)]
    size: Option<String>,
    #[serde(rename = "mimeType", default)]
    mime_type: Option<String>,
    #[serde(rename = "modifiedTime")]
    modified_time: String,
    #[serde(rename = "md5Checksum", default)]
    md5_checksum: Option<String>,
}

/// Validates the answer of `files.get` against the id that was asked for.
pub(crate) fn parse_metadata(requested_id: &str, body: &[u8]) -> Result<FileMetadata, DriveError> {
    let bad = |what: &str| DriveError::BadResponse(what.to_owned());
    let json: MetadataJson = serde_json::from_slice(body).map_err(|_| bad("popis souboru nelze přečíst"))?;
    if json.id != requested_id || !is_valid_file_id(&json.id) {
        return Err(bad("identifikátor souboru nesouhlasí"));
    }
    if parse_rfc3339_utc(&json.modified_time).is_none() {
        return Err(bad("neplatný čas změny souboru"));
    }
    let size = match json.size {
        Some(s) => Some(s.parse::<u64>().map_err(|_| bad("neplatná velikost souboru"))?),
        None => None,
    };
    let md5_checksum = match json.md5_checksum {
        Some(m) => {
            let m = m.to_ascii_lowercase();
            if m.len() != 32 || !m.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(bad("neplatný kontrolní součet"));
            }
            Some(m)
        }
        None => None,
    };
    Ok(FileMetadata {
        id: json.id,
        name: sanitize_display_name(&json.name),
        size,
        mime_type: json.mime_type.unwrap_or_default(),
        modified_time: json.modified_time,
        md5_checksum,
    })
}

/// Checks that the file may be downloaded: a real file with a supported extension that is not
/// larger than `max_size`. Returns the extension and the size.
pub(crate) fn check_fetchable(meta: &FileMetadata, max_size: u64) -> Result<(&'static str, u64), DriveError> {
    if meta.mime_type.starts_with(GOOGLE_APPS_MIME_PREFIX) {
        return Err(DriveError::NotDownloadable { name: meta.name.clone() });
    }
    let ext = supported_extension(&meta.name)
        .ok_or_else(|| DriveError::UnsupportedType { name: meta.name.clone() })?;
    let size = meta
        .size
        .ok_or_else(|| DriveError::NotDownloadable { name: meta.name.clone() })?;
    if size > max_size {
        return Err(DriveError::TooLarge {
            name: meta.name.clone(),
            size: format_size(size),
            limit: format_size(max_size),
        });
    }
    Ok((ext, size))
}

/// Local file name: `<fileId>_<modifiedTime digits>.<ext>`. The id and the time are validated,
/// the extension comes from [`SUPPORTED_EXTENSIONS`], so the result can never contain a path.
pub fn cache_file_name(file_id: &str, modified_time: &str, ext: &str) -> Result<String, DriveError> {
    if !is_valid_file_id(file_id) || !SUPPORTED_EXTENSIONS.contains(&ext) {
        return Err(DriveError::BadResponse("neplatný název místní kopie".into()));
    }
    let t = parse_rfc3339_utc(modified_time)
        .ok_or_else(|| DriveError::BadResponse("neplatný čas změny souboru".into()))?;
    Ok(format!("{file_id}_{}.{ext}", t.compact))
}

/// A validated UTC timestamp.
pub(crate) struct Timestamp {
    /// `YYYYMMDDTHHMMSS[fraction]Z`, safe in a file name.
    pub compact: String,
    pub time: SystemTime,
}

/// Accepts exactly `YYYY-MM-DDTHH:MM:SS[.f{1,9}]Z` with a valid calendar date (1970 and later).
pub(crate) fn parse_rfc3339_utc(s: &str) -> Option<Timestamp> {
    let b = s.as_bytes();
    if b.len() < 20 || !s.is_ascii() {
        return None;
    }
    let digits = |from: usize, to: usize| -> Option<i64> {
        let part = s.get(from..to)?;
        if part.bytes().all(|c| c.is_ascii_digit()) { part.parse().ok() } else { None }
    };
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (y, mo, d) = (digits(0, 4)?, digits(5, 7)?, digits(8, 10)?);
    let (h, mi, sec) = (digits(11, 13)?, digits(14, 16)?, digits(17, 19)?);
    let mut rest = &s[19..];
    let mut frac = "";
    if let Some(after_dot) = rest.strip_prefix('.') {
        let n = after_dot.bytes().take_while(u8::is_ascii_digit).count();
        if !(1..=9).contains(&n) {
            return None;
        }
        frac = &after_dot[..n];
        rest = &after_dot[n..];
    }
    if rest != "Z" {
        return None;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let month_len = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if !(1970..=2999).contains(&y) || d < 1 || d > month_len || h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    let days = days_from_civil(y, mo, d);
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec;
    let nanos: u32 = format!("{frac:0<9}").parse().ok()?;
    let time = UNIX_EPOCH + Duration::new(u64::try_from(secs).ok()?, nanos);
    let compact = format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{sec:02}{frac}Z");
    Some(Timestamp { compact, time })
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[derive(Deserialize)]
struct ApiErrorJson {
    error: Option<ApiErrorBody>,
}

#[derive(Deserialize)]
struct ApiErrorBody {
    #[serde(default)]
    errors: Vec<ApiErrorItem>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Deserialize)]
struct ApiErrorItem {
    #[serde(default)]
    reason: Option<String>,
}

/// Maps an HTTP status of the Drive API (and the reason in its error body) to a Czech error.
pub(crate) fn status_error(status: u16, body: &[u8]) -> DriveError {
    match status {
        401 => DriveError::Unauthorized,
        403 => {
            let parsed: Option<ApiErrorJson> = serde_json::from_slice(body).ok();
            let limited = parsed
                .and_then(|j| j.error)
                .map(|e| {
                    e.status.as_deref() == Some("RESOURCE_EXHAUSTED")
                        || e.errors.iter().any(|i| {
                            matches!(
                                i.reason.as_deref(),
                                Some("rateLimitExceeded")
                                    | Some("userRateLimitExceeded")
                                    | Some("dailyLimitExceeded")
                                    | Some("sharingRateLimitExceeded")
                                    | Some("quotaExceeded")
                            )
                        })
                })
                .unwrap_or(false);
            if limited { DriveError::Unavailable(403) } else { DriveError::Forbidden }
        }
        404 => DriveError::NotFound,
        408 | 429 | 500..=599 => DriveError::Unavailable(status),
        other => DriveError::HttpStatus(other),
    }
}

fn read_error_body(reply: Reply) -> Vec<u8> {
    reply.read_limited(16 * 1024).unwrap_or_default()
}

fn files_url(api_base: &str, file_id: &str, query: &str) -> Result<String, DriveError> {
    if !is_valid_file_id(file_id) {
        return Err(DriveError::Link(crate::error::LinkError::BadFileId));
    }
    Ok(format!("{api_base}/files/{file_id}?{query}&supportsAllDrives=true"))
}

/// `GET files/{id}?fields=...`
pub(crate) fn get_metadata(
    http: &Http,
    api_base: &str,
    token: &Secret,
    file_id: &str,
) -> Result<FileMetadata, DriveError> {
    let fields = crate::urlutil::percent_encode(METADATA_FIELDS);
    let url = files_url(api_base, file_id, &format!("fields={fields}"))?;
    let reply = http.get(Kind::Api, &url, Some(token))?;
    let status = reply.status;
    let body = reply.read_limited(MAX_JSON_BYTES)?;
    if status != 200 {
        return Err(status_error(status, &body));
    }
    parse_metadata(file_id, &body)
}

/// Removes the temporary file unless the download finished and was renamed.
struct TempGuard {
    path: PathBuf,
    keep: bool,
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// True if `path` already holds a complete copy (same size as on the Drive).
pub(crate) fn is_up_to_date(path: &Path, size: u64) -> bool {
    fs::metadata(path).map(|m| m.is_file() && m.len() == size).unwrap_or(false)
}

fn md5_hex(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// `GET files/{id}?alt=media` into `<dir>/<final_name>`, written atomically: the content goes
/// to a temporary file in the same folder and is renamed only when it is complete and
/// verified (size, and MD5 if the Drive supplied one).
#[allow(clippy::too_many_arguments)]
pub(crate) fn download(
    http: &Http,
    api_base: &str,
    token: &Secret,
    meta: &FileMetadata,
    size: u64,
    dir: &Path,
    final_name: &str,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<PathBuf, DriveError> {
    let url = files_url(api_base, &meta.id, "alt=media")?;
    let reply = http.get(Kind::Download, &url, Some(token))?;
    if reply.status != 200 {
        let status = reply.status;
        return Err(status_error(status, &read_error_body(reply)));
    }
    if let Some(declared) = reply.content_length
        && declared != size
    {
        return Err(DriveError::BadResponse(
            "velikost souboru se mezitím změnila, otevřete ho prosím znovu".into(),
        ));
    }

    fs::create_dir_all(dir)?;
    let final_path = dir.join(final_name);
    let tmp_path = dir.join(format!(".{final_name}.{}.part", random_token(6)?));
    // The guard is declared before the file so the file is closed first when both are dropped.
    let mut guard = TempGuard { path: tmp_path.clone(), keep: false };
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&tmp_path)?;

    let mut reader = reply.into_reader();
    let mut hasher = Md5::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done: u64 = 0;
    let mut reported: u64 = 0;
    progress(0, size);
    loop {
        if cancel.is_cancelled() {
            return Err(DriveError::DownloadCancelled);
        }
        let n = match reader.read(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_to_net_error(&e)),
        };
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > size {
            return Err(DriveError::BadResponse("server poslal víc dat, než odpovídá velikosti souboru".into()));
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])?;
        if done - reported >= 256 * 1024 {
            reported = done;
            progress(done, size);
        }
    }
    if done != size {
        return Err(DriveError::BadResponse("stahování nebylo úplné".into()));
    }
    if let Some(expected) = &meta.md5_checksum
        && md5_hex(&hasher.finalize()) != *expected
    {
        return Err(DriveError::ChecksumMismatch);
    }
    file.flush()?;
    // The file gets the modified time from the Drive, so the same version always has the same
    // size and mtime and the conversion cache of the viewer can recognise it again.
    if let Some(ts) = parse_rfc3339_utc(&meta.modified_time) {
        let _ = file.set_modified(ts.time);
    }
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp_path, &final_path)?;
    guard.keep = true;
    progress(size, size);
    Ok(final_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "1AbCdEfGhIjKlMnOpQrStUvWxYz_0123456-";

    fn meta(name: &str, size: Option<u64>, mime: &str) -> FileMetadata {
        FileMetadata {
            id: ID.into(),
            name: name.into(),
            size,
            mime_type: mime.into(),
            modified_time: "2026-01-15T10:30:00.123Z".into(),
            md5_checksum: None,
        }
    }

    #[test]
    fn extension_check() {
        for (n, e) in [
            ("a.step", "step"), ("A.STP", "stp"), ("x.y.IgEs", "iges"), ("m.igs", "igs"),
            ("p.stl", "stl"), ("d.DXF", "dxf"), ("d.dwg", "dwg"), ("díl č.1.step", "step"),
        ] {
            assert_eq!(supported_extension(n), Some(e), "{n}");
        }
        for n in [
            "a.exe", "a.step.exe", "a", "", ".step", "a.step ", "a.ste", "a.stepx", "step", "a.step.",
            "a.pdf", "a.zip", "a.stp\u{0}", "archiv.step.zip",
        ] {
            assert_eq!(supported_extension(n), None, "{n:?}");
        }
    }

    #[test]
    fn display_name_is_sanitised() {
        assert_eq!(sanitize_display_name("díl.step"), "díl.step");
        assert_eq!(sanitize_display_name("a\u{202E}pets.exe"), "a_pets.exe");
        assert_eq!(sanitize_display_name("a\nb\u{0}c.step"), "a_b_c.step");
        assert_eq!(sanitize_display_name("   "), "soubor");
        assert_eq!(sanitize_display_name(""), "soubor");
        assert_eq!(sanitize_display_name(&"a".repeat(400)).chars().count(), 255);
        // Path parts survive as text: the name is data, never a path.
        assert_eq!(sanitize_display_name("..\\..\\x.step"), "..\\..\\x.step");
    }

    #[test]
    fn rfc3339_parsing() {
        let t = parse_rfc3339_utc("2026-01-15T10:30:00.123Z").unwrap();
        assert_eq!(t.compact, "20260115T103000123Z");
        assert_eq!(t.time.duration_since(UNIX_EPOCH).unwrap(), Duration::new(1_768_473_000, 123_000_000));
        let t = parse_rfc3339_utc("1970-01-01T00:00:00Z").unwrap();
        assert_eq!(t.time, UNIX_EPOCH);
        assert_eq!(t.compact, "19700101T000000Z");
        let t = parse_rfc3339_utc("2024-02-29T23:59:59.999999999Z").unwrap();
        // Windows keeps SystemTime in units of 100 ns, so compare with that resolution.
        let d = t.time.duration_since(UNIX_EPOCH).unwrap();
        assert_eq!(d.as_secs(), 1_709_251_199);
        assert!(d.subsec_nanos() >= 999_999_900, "{d:?}");
        assert!(t.compact.ends_with("999999999Z"));
        for bad in [
            "", "2026-01-15", "2026-01-15T10:30:00", "2026-01-15T10:30:00+01:00", "2026-01-15 10:30:00Z",
            "2026-13-15T10:30:00Z", "2026-02-30T10:30:00Z", "2025-02-29T10:30:00Z", "2026-01-15T24:30:00Z",
            "2026-01-15T10:60:00Z", "2026-01-15T10:30:60Z", "2026-01-15T10:30:00.Z", "2026-01-15T10:30:00.1234567890Z",
            "1969-12-31T23:59:59Z", "2026-01-15T10:30:00z", "2026-01-15T10:30:00Zx", "../../etc/passwd/x.step",
            "2026-01-15T10:30:00.123Z\u{0}", "２０２６-01-15T10:30:00Z", "2026-01-15T10:30:00.12a3Z",
        ] {
            assert!(parse_rfc3339_utc(bad).is_none(), "{bad:?}");
        }
        // 2000-03-01 is 11017 days after the epoch.
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
    }

    #[test]
    fn cache_name_is_derived_from_id_and_time_only() {
        assert_eq!(
            cache_file_name(ID, "2026-01-15T10:30:00.123Z", "step").unwrap(),
            format!("{ID}_20260115T103000123Z.step")
        );
        assert_ne!(
            cache_file_name(ID, "2026-01-15T10:30:00.123Z", "step").unwrap(),
            cache_file_name(ID, "2026-01-15T10:30:01.123Z", "step").unwrap()
        );
        assert!(cache_file_name("../x", "2026-01-15T10:30:00Z", "step").is_err());
        assert!(cache_file_name(ID, "2026-01-15T10:30:00Z", "exe").is_err());
        assert!(cache_file_name(ID, "evil/../time", "step").is_err());
        let name = cache_file_name(ID, "2026-01-15T10:30:00Z", "dwg").unwrap();
        assert!(!name.contains(['/', '\\', ':']));
        assert_eq!(Path::new(&name).components().count(), 1);
    }

    #[test]
    fn metadata_parsing() {
        let body = format!(
            r#"{{"id":"{ID}","name":"Díl 1.STEP","size":"12345","mimeType":"application/octet-stream",
               "modifiedTime":"2026-01-15T10:30:00.123Z","md5Checksum":"D41D8CD98F00B204E9800998ECF8427E"}}"#
        );
        let m = parse_metadata(ID, body.as_bytes()).unwrap();
        assert_eq!(m.name, "Díl 1.STEP");
        assert_eq!(m.size, Some(12345));
        assert_eq!(m.md5_checksum.as_deref(), Some("d41d8cd98f00b204e9800998ecf8427e"));

        let folder = format!(
            r#"{{"id":"{ID}","name":"Složka","mimeType":"application/vnd.google-apps.folder","modifiedTime":"2026-01-15T10:30:00Z"}}"#
        );
        let m = parse_metadata(ID, folder.as_bytes()).unwrap();
        assert_eq!(m.size, None);

        let other_id = body.replace(ID, "ZZZZZZZZZZZZZZZZZZZZ");
        for bad in [
            other_id.as_str(),
            "not json",
            "{}",
            &body.replace("12345", "12x45"),
            &body.replace("12345", "-5"),
            &body.replace("2026-01-15T10:30:00.123Z", "yesterday"),
            &body.replace("D41D8CD98F00B204E9800998ECF8427E", "xyz"),
        ] {
            assert!(
                matches!(parse_metadata(ID, bad.as_bytes()), Err(DriveError::BadResponse(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn fetch_checks() {
        let m = meta("a.step", Some(100), "application/octet-stream");
        assert_eq!(check_fetchable(&m, 100).unwrap(), ("step", 100));
        assert!(matches!(check_fetchable(&m, 99), Err(DriveError::TooLarge { .. })));
        assert!(matches!(
            check_fetchable(&meta("a.exe", Some(1), "application/octet-stream"), 100),
            Err(DriveError::UnsupportedType { .. })
        ));
        assert!(matches!(
            check_fetchable(&meta("a.step", None, "application/octet-stream"), 100),
            Err(DriveError::NotDownloadable { .. })
        ));
        assert!(matches!(
            check_fetchable(&meta("a.step", Some(1), "application/vnd.google-apps.folder"), 100),
            Err(DriveError::NotDownloadable { .. })
        ));
        assert!(matches!(
            check_fetchable(&meta("a.step", Some(1), "application/vnd.google-apps.document"), 100),
            Err(DriveError::NotDownloadable { .. })
        ));
    }

    #[test]
    fn status_mapping() {
        assert_eq!(status_error(401, b""), DriveError::Unauthorized);
        assert_eq!(status_error(404, b""), DriveError::NotFound);
        assert_eq!(status_error(403, b"{}"), DriveError::Forbidden);
        assert_eq!(status_error(403, b"garbage"), DriveError::Forbidden);
        assert_eq!(
            status_error(403, br#"{"error":{"errors":[{"reason":"userRateLimitExceeded"}]}}"#),
            DriveError::Unavailable(403)
        );
        assert_eq!(
            status_error(403, br#"{"error":{"status":"RESOURCE_EXHAUSTED"}}"#),
            DriveError::Unavailable(403)
        );
        assert_eq!(
            status_error(403, br#"{"error":{"errors":[{"reason":"forbidden"}]}}"#),
            DriveError::Forbidden
        );
        assert_eq!(status_error(429, b""), DriveError::Unavailable(429));
        assert_eq!(status_error(503, b""), DriveError::Unavailable(503));
        assert_eq!(status_error(418, b""), DriveError::HttpStatus(418));
    }

    #[test]
    fn up_to_date_check() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.step");
        assert!(!is_up_to_date(&p, 3));
        fs::write(&p, b"abc").unwrap();
        assert!(is_up_to_date(&p, 3));
        assert!(!is_up_to_date(&p, 4));
        assert!(!is_up_to_date(dir.path(), 0));
    }
}
