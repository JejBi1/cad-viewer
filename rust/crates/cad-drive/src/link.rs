//! Strict parsing of `cadviewer://open?fileId=<id>`.
//!
//! The link arrives from the operating system when a web page or another program starts the
//! registered handler, so it is untrusted input. The accepted shape is exactly
//! `cadviewer://open?fileId=<id>` (an optional `/` after `open` is tolerated) where `<id>`
//! matches `^[A-Za-z0-9_-]{10,128}$`. Everything else is rejected, in particular percent
//! encoding anywhere, a second or foreign parameter, a fragment, user info, a port, a path,
//! white space and non-ASCII characters.

use crate::error::LinkError;

/// URL scheme registered for the viewer.
pub const LINK_SCHEME: &str = "cadviewer";
/// The only supported command (host part).
pub const LINK_COMMAND: &str = "open";
/// Name of the single parameter.
pub const LINK_PARAM: &str = "fileId";
/// Longest accepted link. The longest valid one is 24 + 128 = 152 characters.
pub const MAX_LINK_LEN: usize = 256;
pub const FILE_ID_MIN_LEN: usize = 10;
pub const FILE_ID_MAX_LEN: usize = 128;

/// `^[A-Za-z0-9_-]{10,128}$` without a regex engine.
pub fn is_valid_file_id(id: &str) -> bool {
    let len = id.len();
    (FILE_ID_MIN_LEN..=FILE_ID_MAX_LEN).contains(&len)
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A validated Drive file reference. The id can only be created through validation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DriveLink {
    file_id: String,
}

impl DriveLink {
    /// Builds a link from an id that comes from somewhere else (for example the Drive page).
    pub fn new(file_id: &str) -> Result<Self, LinkError> {
        if !is_valid_file_id(file_id) {
            return Err(LinkError::BadFileId);
        }
        Ok(DriveLink {
            file_id: file_id.to_owned(),
        })
    }

    pub fn file_id(&self) -> &str {
        &self.file_id
    }

    /// The canonical link text, `cadviewer://open?fileId=<id>`.
    pub fn to_url(&self) -> String {
        format!("{LINK_SCHEME}://{LINK_COMMAND}?{LINK_PARAM}={}", self.file_id)
    }
}

/// Parses and validates a link. See the module documentation for the accepted shape.
pub fn parse_link(url: &str) -> Result<DriveLink, LinkError> {
    if url.is_empty() {
        return Err(LinkError::Empty);
    }
    if url.len() > MAX_LINK_LEN {
        return Err(LinkError::TooLong);
    }
    // Printable ASCII only: no white space, control characters, quotes are caught later,
    // and nothing outside ASCII (look-alike letters, bidi controls).
    if !url.bytes().all(|b| (0x21..=0x7E).contains(&b)) {
        return Err(LinkError::BadCharacters);
    }
    if url.contains('%') {
        return Err(LinkError::PercentEncoding);
    }
    if url.contains('#') || url.contains('\\') {
        return Err(LinkError::BadStructure);
    }

    let (scheme, rest) = url.split_once("://").ok_or(LinkError::BadScheme)?;
    if !scheme.eq_ignore_ascii_case(LINK_SCHEME) {
        return Err(LinkError::BadScheme);
    }

    let (authority, query) = match rest.split_once('?') {
        Some((a, q)) => (a, Some(q)),
        None => (rest, None),
    };

    // Host: exactly `open`, optionally followed by a single `/`.
    let host_end = authority.find(['/', ':', '@']).unwrap_or(authority.len());
    let (host, tail) = authority.split_at(host_end);
    if !host.eq_ignore_ascii_case(LINK_COMMAND) {
        return Err(LinkError::BadHost);
    }
    if !(tail.is_empty() || tail == "/") {
        return Err(LinkError::BadStructure);
    }

    let query = match query {
        Some(q) if !q.is_empty() => q,
        _ => return Err(LinkError::MissingFileId),
    };

    let mut file_id: Option<&str> = None;
    let mut duplicates = false;
    let mut foreign = false;
    let mut missing_value = false;
    for pair in query.split('&') {
        let (key, value) = match pair.split_once('=') {
            Some((k, v)) => (k, Some(v)),
            None => (pair, None),
        };
        if key != LINK_PARAM {
            foreign = true;
            continue;
        }
        match value {
            Some(v) if !v.is_empty() => {
                if file_id.is_some() {
                    duplicates = true;
                } else {
                    file_id = Some(v);
                }
            }
            _ => missing_value = true,
        }
    }

    if foreign {
        return Err(LinkError::UnexpectedParameter);
    }
    if duplicates {
        return Err(LinkError::DuplicateFileId);
    }
    if missing_value && file_id.is_none() {
        return Err(LinkError::MissingFileId);
    }
    if missing_value {
        // `fileId=<id>&fileId=` is a second parameter as well.
        return Err(LinkError::DuplicateFileId);
    }
    let id = file_id.ok_or(LinkError::MissingFileId)?;
    DriveLink::new(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "1AbCdEfGhIjKlMnOpQrStUvWxYz_0123456-";

    fn ok(url: &str) -> String {
        parse_link(url).unwrap().file_id().to_owned()
    }

    #[test]
    fn accepts_canonical_link() {
        assert_eq!(ok(&format!("cadviewer://open?fileId={ID}")), ID);
        assert_eq!(parse_link(&format!("cadviewer://open?fileId={ID}")).unwrap().to_url(), format!("cadviewer://open?fileId={ID}"));
    }

    #[test]
    fn accepts_harmless_variants() {
        assert_eq!(ok(&format!("cadviewer://open/?fileId={ID}")), ID);
        assert_eq!(ok(&format!("CADVIEWER://OPEN?fileId={ID}")), ID);
        assert_eq!(ok(&format!("CadViewer://Open/?fileId={ID}")), ID);
    }

    #[test]
    fn id_length_limits() {
        let min = "a".repeat(10);
        let max = "b".repeat(128);
        assert_eq!(ok(&format!("cadviewer://open?fileId={min}")), min);
        assert_eq!(ok(&format!("cadviewer://open?fileId={max}")), max);
        assert_eq!(
            parse_link(&format!("cadviewer://open?fileId={}", "a".repeat(9))),
            Err(LinkError::BadFileId)
        );
        assert_eq!(
            parse_link(&format!("cadviewer://open?fileId={}", "a".repeat(129))),
            Err(LinkError::BadFileId)
        );
    }

    #[test]
    fn id_character_rule() {
        for bad in [
            "abc.def.ghijk", "abc~def~ghij", "abc/def/ghij", "abc=def=ghij", "abc;def;ghij",
            "abc+def+ghij", "abc'def'ghij", "abc\"def\"ghij", "abc<def>ghij", "abc:def:ghij",
            "abc,def,ghij", "abc*def*ghij", "..........",
        ] {
            assert_eq!(
                parse_link(&format!("cadviewer://open?fileId={bad}")),
                Err(LinkError::BadFileId),
                "{bad}"
            );
        }
        assert!(is_valid_file_id("0123456789"));
        assert!(!is_valid_file_id(""));
        assert!(!is_valid_file_id("příliš-dlouhý-id"));
    }

    #[test]
    fn rejects_empty_and_long() {
        assert_eq!(parse_link(""), Err(LinkError::Empty));
        let long = format!("cadviewer://open?fileId={ID}&{}", "x".repeat(300));
        assert_eq!(parse_link(&long), Err(LinkError::TooLong));
        // Exactly at the limit still reaches the parser, one more is too long.
        let at_limit = format!("cadviewer://open?fileId={ID}{}", "&".repeat(MAX_LINK_LEN - 24 - ID.len()));
        assert_eq!(at_limit.len(), MAX_LINK_LEN);
        assert_ne!(parse_link(&at_limit), Err(LinkError::TooLong));
        assert_eq!(parse_link(&format!("{at_limit}&")), Err(LinkError::TooLong));
    }

    #[test]
    fn rejects_bad_scheme() {
        for url in [
            format!("http://open?fileId={ID}"),
            format!("https://open?fileId={ID}"),
            format!("file://open?fileId={ID}"),
            format!("cadviewer:open?fileId={ID}"),
            format!("cadviewer:/open?fileId={ID}"),
            format!("cadviewer2://open?fileId={ID}"),
            format!("xcadviewer://open?fileId={ID}"),
            format!("://open?fileId={ID}"),
            format!("open?fileId={ID}"),
            format!("javascript://open?fileId={ID}"),
        ] {
            assert_eq!(parse_link(&url), Err(LinkError::BadScheme), "{url}");
        }
    }

    #[test]
    fn rejects_bad_host_and_structure() {
        for url in [
            format!("cadviewer://close?fileId={ID}"),
            format!("cadviewer://opens?fileId={ID}"),
            format!("cadviewer://?fileId={ID}"),
            format!("cadviewer://evil.com?fileId={ID}"),
            format!("cadviewer://openx?fileId={ID}"),
        ] {
            assert_eq!(parse_link(&url), Err(LinkError::BadHost), "{url}");
        }
        for url in [
            format!("cadviewer://open/extra?fileId={ID}"),
            format!("cadviewer://open//?fileId={ID}"),
            format!("cadviewer://open:80?fileId={ID}"),
            format!("cadviewer://open@evil?fileId={ID}"),
            format!("cadviewer://user:pw@open?fileId={ID}"),
            format!("cadviewer://open?fileId={ID}#frag"),
            format!("cadviewer://open#x?fileId={ID}"),
            format!("cadviewer:\\\\open?fileId={ID}"),
            format!("cadviewer://open\\x?fileId={ID}"),
            format!("cadviewer://open?fileId={ID}\\"),
        ] {
            let r = parse_link(&url);
            assert!(
                matches!(r, Err(LinkError::BadStructure) | Err(LinkError::BadScheme) | Err(LinkError::BadHost)),
                "{url} -> {r:?}"
            );
            assert!(r.is_err(), "{url}");
        }
    }

    #[test]
    fn rejects_missing_duplicate_and_foreign_parameters() {
        assert_eq!(parse_link("cadviewer://open"), Err(LinkError::MissingFileId));
        assert_eq!(parse_link("cadviewer://open?"), Err(LinkError::MissingFileId));
        assert_eq!(parse_link("cadviewer://open?fileId="), Err(LinkError::MissingFileId));
        assert_eq!(parse_link("cadviewer://open?fileId"), Err(LinkError::MissingFileId));
        assert_eq!(
            parse_link(&format!("cadviewer://open?fileId={ID}&fileId={ID}")),
            Err(LinkError::DuplicateFileId)
        );
        assert_eq!(
            parse_link(&format!("cadviewer://open?fileId={ID}&fileId=")),
            Err(LinkError::DuplicateFileId)
        );
        for url in [
            format!("cadviewer://open?fileId={ID}&path=C:/x"),
            format!("cadviewer://open?path=x&fileId={ID}"),
            format!("cadviewer://open?fileid={ID}"),
            format!("cadviewer://open?FILEID={ID}"),
            format!("cadviewer://open?file={ID}"),
            format!("cadviewer://open?fileId={ID}&"),
            format!("cadviewer://open?&fileId={ID}"),
            format!("cadviewer://open?fileId={ID}&&"),
            "cadviewer://open?url=https://evil.example/x.step".to_owned(),
            "cadviewer://open?=x".to_owned(),
        ] {
            assert_eq!(parse_link(&url), Err(LinkError::UnexpectedParameter), "{url}");
        }
    }

    #[test]
    fn rejects_percent_encoding_everywhere() {
        for url in [
            format!("cadviewer://open?fileId=%41{ID}"),
            format!("cadviewer://open?fileId={ID}%00"),
            format!("cadviewer://open?%66ileId={ID}"),
            format!("cadviewer://%6Fpen?fileId={ID}"),
            format!("cadviewer%3A//open?fileId={ID}"),
            format!("cadviewer://open?fileId={ID}%26x%3D1"),
            format!("cadviewer://open?fileId={ID}%"),
            "cadviewer://open?fileId=..%2F..%2Fetc%2Fpasswd".to_owned(),
        ] {
            assert_eq!(parse_link(&url), Err(LinkError::PercentEncoding), "{url}");
        }
    }

    #[test]
    fn rejects_whitespace_controls_and_non_ascii() {
        for url in [
            format!(" cadviewer://open?fileId={ID}"),
            format!("cadviewer://open?fileId={ID} "),
            format!("cadviewer://open?fileId={ID}\n"),
            format!("cadviewer://open?fileId={ID}\r\n"),
            format!("cadviewer://open?fileId={ID}\0"),
            format!("cadviewer://open?fileId={ID}\t"),
            format!("cadviewer://open?fileId={ID}\u{202E}"),
            format!("cadviewer://open?fileId={ID}á"),
            format!("cadviewer://open?fileId={ID}\" --evil \""),
            format!("cadviewer://open?fileId={ID} --other"),
            format!("cadviewer://open?fileId={ID}\u{ff06}x"),
        ] {
            let r = parse_link(&url);
            assert_eq!(r, Err(LinkError::BadCharacters), "{url:?} -> {r:?}");
        }
        // Quote characters inside an otherwise printable URL never reach a command line.
        for url in [
            format!("cadviewer://open?fileId={ID}\"--evil"),
            format!("cadviewer://open?fileId=\"{ID}\""),
            format!("cadviewer://open?fileId={ID}'"),
        ] {
            assert_eq!(parse_link(&url), Err(LinkError::BadFileId), "{url}");
        }
    }

    #[test]
    fn drive_link_new_validates() {
        assert!(DriveLink::new(ID).is_ok());
        assert_eq!(DriveLink::new("short"), Err(LinkError::BadFileId));
        assert_eq!(DriveLink::new("../../etc/passwd"), Err(LinkError::BadFileId));
        assert_eq!(DriveLink::new(ID).unwrap().to_url(), format!("cadviewer://open?fileId={ID}"));
    }

    #[test]
    fn long_garbage_never_panics() {
        for n in 0..300 {
            let s: String = (0..n).map(|i| char::from(0x21 + (i % 90) as u8)).collect();
            let _ = parse_link(&s);
            let _ = parse_link(&format!("cadviewer://open?fileId={s}"));
        }
    }
}
