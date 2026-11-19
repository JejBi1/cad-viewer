//! Small URL helpers: splitting, loopback detection, percent encoding and query parsing.
//! Only what the OAuth flow and the redirect policy need.

use std::net::IpAddr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UrlParts {
    /// Lower-case scheme.
    pub scheme: String,
    /// Lower-case host, without brackets for IPv6.
    pub host: String,
    pub port: Option<u16>,
    /// Path and query, starting with `/`, or empty.
    pub path_query: String,
}

impl UrlParts {
    pub fn effective_port(&self) -> Option<u16> {
        self.port.or(match self.scheme.as_str() {
            "https" => Some(443),
            "http" => Some(80),
            _ => None,
        })
    }

    pub fn same_origin(&self, other: &UrlParts) -> bool {
        self.scheme == other.scheme
            && self.host == other.host
            && self.effective_port() == other.effective_port()
    }
}

/// Splits an absolute `http(s)` URL. Returns `None` for anything unusual: other schemes,
/// user info, empty host, bad port, non-ASCII or control characters.
pub(crate) fn split_url(url: &str) -> Option<UrlParts> {
    if !url.bytes().all(|b| (0x21..=0x7E).contains(&b)) {
        return None;
    }
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(auth_end);
    if authority.contains('@') || authority.is_empty() {
        return None;
    }
    let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
        let close = inner.find(']')?;
        let host = &inner[..close];
        let after = &inner[close + 1..];
        let port = match after {
            "" => None,
            _ => Some(after.strip_prefix(':')?.parse::<u16>().ok()?),
        };
        (host.to_owned(), port)
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_owned(), Some(p.parse::<u16>().ok()?)),
            None => (authority.to_owned(), None),
        }
    };
    if host.is_empty() {
        return None;
    }
    let host = host.to_ascii_lowercase();
    let host_ok = host
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b':');
    if !host_ok {
        return None;
    }
    // A fragment never goes on the wire.
    let path_query = tail.split('#').next().unwrap_or("").to_owned();
    Some(UrlParts { scheme, host, port, path_query })
}

pub(crate) fn is_loopback_host(host: &str) -> bool {
    host == "localhost" || host.parse::<IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false)
}

/// Percent-encodes everything except the RFC 3986 unreserved characters.
pub(crate) fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(char::from(b"0123456789ABCDEF"[(b >> 4) as usize]));
            out.push(char::from(b"0123456789ABCDEF"[(b & 15) as usize]));
        }
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Strict percent decoding: a bad escape or a result that is not UTF-8 gives `None`.
/// `+` becomes a space (form encoding).
pub(crate) fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hi = hex_val(*bytes.get(i + 1)?)?;
                let lo = hex_val(*bytes.get(i + 2)?)?;
                out.push(hi << 4 | lo);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// Parses `a=b&c=d` into decoded pairs. A pair that cannot be decoded is skipped.
pub(crate) fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            Some((percent_decode(k)?, percent_decode(v)?))
        })
        .collect()
}

/// `application/x-www-form-urlencoded` body from pairs.
pub(crate) fn form_body(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Compares two strings without stopping at the first difference.
pub(crate) fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_urls() {
        let p = split_url("https://www.googleapis.com/drive/v3/files/x?alt=media").unwrap();
        assert_eq!(p.scheme, "https");
        assert_eq!(p.host, "www.googleapis.com");
        assert_eq!(p.port, None);
        assert_eq!(p.effective_port(), Some(443));
        assert_eq!(p.path_query, "/drive/v3/files/x?alt=media");
        let p = split_url("HTTP://127.0.0.1:8080").unwrap();
        assert_eq!((p.scheme.as_str(), p.host.as_str(), p.port), ("http", "127.0.0.1", Some(8080)));
        assert_eq!(p.path_query, "");
        let p = split_url("http://[::1]:99/x#frag").unwrap();
        assert_eq!((p.host.as_str(), p.port, p.path_query.as_str()), ("::1", Some(99), "/x"));
    }

    #[test]
    fn rejects_odd_urls() {
        for u in [
            "ftp://x.com/", "https://", "https://user@host/", "https://host:abc/", "https://host:99999/",
            "https://ho st/", "https://hóst/", "//host/x", "host/x", "https://[::1/", "https://[::1]x/",
            "https://ho_st/",
        ] {
            assert_eq!(split_url(u), None, "{u}");
        }
    }

    #[test]
    fn loopback_detection() {
        for h in ["127.0.0.1", "localhost", "::1", "127.1.2.3"] {
            assert!(is_loopback_host(h), "{h}");
        }
        for h in ["example.com", "128.0.0.1", "0.0.0.0", "127.0.0.1.evil.com", "localhost.evil.com"] {
            assert!(!is_loopback_host(h), "{h}");
        }
    }

    #[test]
    fn same_origin_compares_effective_port() {
        let a = split_url("https://a.com/x").unwrap();
        assert!(a.same_origin(&split_url("https://A.com:443/y").unwrap()));
        assert!(!a.same_origin(&split_url("https://a.com:444/y").unwrap()));
        assert!(!a.same_origin(&split_url("http://a.com/y").unwrap()));
    }

    #[test]
    fn percent_round_trip() {
        let src = "4/0AX 4X&=+č~-._";
        let enc = percent_encode(src);
        assert_eq!(enc, "4%2F0AX%204X%26%3D%2B%C4%8D~-._");
        assert_eq!(percent_decode(&enc).unwrap(), src);
        assert_eq!(percent_decode("a+b").unwrap(), "a b");
        assert_eq!(percent_decode("%zz"), None);
        assert_eq!(percent_decode("%4"), None);
        assert_eq!(percent_decode("%C3"), None); // not valid UTF-8 alone
    }

    #[test]
    fn query_parsing() {
        let q = parse_query("code=4%2F0AbC&state=xyz&error=&flag");
        assert_eq!(
            q,
            vec![
                ("code".to_owned(), "4/0AbC".to_owned()),
                ("state".to_owned(), "xyz".to_owned()),
                ("error".to_owned(), String::new()),
                ("flag".to_owned(), String::new()),
            ]
        );
        assert!(parse_query("a=%zz").is_empty());
    }

    #[test]
    fn form_body_encodes() {
        assert_eq!(form_body(&[("a", "b c"), ("d&", "e=")]), "a=b%20c&d%26=e%3D");
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
    }
}
