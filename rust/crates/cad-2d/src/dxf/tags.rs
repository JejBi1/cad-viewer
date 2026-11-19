//! Group code / value reader for ASCII and binary DXF files.
//!
//! Values stay raw bytes (no EOL) until a caller asks for a number or a string, so the reader itself never decodes text.
//! Garbage lines are skipped and counted in `errors` (the "recover" mode of ezdxf).
use std::borrow::Cow;

/// "AutoCAD Binary DXF\r\n\x1a\0".
pub const BINARY_SENTINEL: &[u8] = b"AutoCAD Binary DXF\r\n\x1a\0";

#[derive(Clone, Debug)]
pub struct Tag<'a> {
    pub code: i32,
    pub val: Cow<'a, [u8]>,
}

impl<'a> Tag<'a> {
    pub fn f64(&self) -> f64 {
        parse_f64(&self.val)
    }

    pub fn i64(&self) -> i64 {
        parse_i64(&self.val)
    }

    pub fn i32(&self) -> i32 {
        self.i64().clamp(i32::MIN as i64, i32::MAX as i64) as i32
    }

    pub fn is_empty(&self) -> bool {
        self.val.is_empty()
    }
}

/// Lenient float parse: surrounding blanks, `D` exponents and a leading `+` are accepted, anything else gives 0.
pub fn parse_f64(b: &[u8]) -> f64 {
    let s = trim_ascii(b);
    if s.is_empty() {
        return 0.0;
    }
    if let Ok(t) = std::str::from_utf8(s) {
        if let Ok(v) = t.parse::<f64>() {
            return if v.is_finite() { v } else { 0.0 };
        }
        let t2 = t.replace(['D', 'd'], "E");
        if let Ok(v) = t2.parse::<f64>() {
            return if v.is_finite() { v } else { 0.0 };
        }
        // "1.5abc" -> longest numeric prefix
        let mut end = 0;
        for (i, c) in t.char_indices() {
            if c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E') {
                end = i + c.len_utf8();
            } else {
                break;
            }
        }
        while end > 0 {
            if let Ok(v) = t[..end].parse::<f64>() {
                return if v.is_finite() { v } else { 0.0 };
            }
            end -= 1;
        }
    }
    0.0
}

pub fn parse_i64(b: &[u8]) -> i64 {
    let s = trim_ascii(b);
    if s.is_empty() {
        return 0;
    }
    if let Ok(t) = std::str::from_utf8(s) {
        if let Ok(v) = t.parse::<i64>() {
            return v;
        }
        // "12.0" or "1E3" written for an integer tag
        let f = parse_f64(s);
        if f.abs() < 9.0e18 {
            return f as i64;
        }
    }
    0
}

pub fn trim_ascii(mut b: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = b {
        if first.is_ascii_whitespace() {
            b = rest;
        } else {
            break;
        }
    }
    while let [rest @ .., last] = b {
        if last.is_ascii_whitespace() {
            b = rest;
        } else {
            break;
        }
    }
    b
}

/// Kind of value stored under a group code (used for binary DXF and for deciding what a tag means).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Str,
    Int16,
    Int32,
    Int64,
    Float,
    Bool,
    Chunk,
}

pub fn kind_of(code: i32) -> Kind {
    match code {
        -5..=9 => Kind::Str,
        10..=59 => Kind::Float,
        60..=79 => Kind::Int16,
        90..=99 => Kind::Int32,
        100..=102 => Kind::Str,
        105 => Kind::Str,
        110..=149 => Kind::Float,
        160..=169 => Kind::Int64,
        170..=179 => Kind::Int16,
        210..=239 => Kind::Float,
        270..=289 => Kind::Int16,
        290..=299 => Kind::Bool,
        300..=309 => Kind::Str,
        310..=319 => Kind::Chunk,
        320..=369 => Kind::Str,
        370..=389 => Kind::Int16,
        390..=399 => Kind::Str,
        400..=409 => Kind::Int16,
        410..=419 => Kind::Str,
        420..=429 => Kind::Int32,
        430..=439 => Kind::Str,
        440..=459 => Kind::Int32,
        460..=469 => Kind::Float,
        470..=481 => Kind::Str,
        999 => Kind::Str,
        1000..=1003 => Kind::Str,
        1004 => Kind::Chunk,
        1005 => Kind::Str,
        1010..=1059 => Kind::Float,
        1060..=1070 => Kind::Int16,
        1071 => Kind::Int32,
        _ => Kind::Str,
    }
}

pub struct TagReader<'a> {
    data: &'a [u8],
    pos: usize,
    binary: bool,
    /// Number of garbage lines / truncated tags that had to be skipped.
    pub errors: u32,
}

impl<'a> TagReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        // a UTF-8 BOM in front of a text DXF is tolerated
        let data = data.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(data);
        let binary = data.starts_with(BINARY_SENTINEL);
        TagReader { data, pos: if binary { BINARY_SENTINEL.len() } else { 0 }, binary, errors: 0 }
    }

    pub fn is_binary(&self) -> bool {
        self.binary
    }

    fn line(&mut self) -> Option<&'a [u8]> {
        if self.pos >= self.data.len() {
            return None;
        }
        let rest = &self.data[self.pos..];
        let (line, adv) = match memchr::memchr(b'\n', rest) {
            Some(i) => (&rest[..i], i + 1),
            None => (rest, rest.len()),
        };
        self.pos += adv;
        Some(line.strip_suffix(b"\r").unwrap_or(line))
    }

    pub fn next_tag(&mut self) -> Option<Tag<'a>> {
        if self.binary {
            return self.next_binary();
        }
        loop {
            let code_line = self.line()?;
            let code_txt = trim_ascii(code_line);
            let code = match std::str::from_utf8(code_txt).ok().and_then(|t| t.parse::<i32>().ok()) {
                Some(c) => c,
                None => {
                    if !code_txt.is_empty() {
                        self.errors += 1;
                    }
                    continue; // resynchronise on the next line
                }
            };
            return match self.line() {
                Some(v) => {
                    if code == 999 {
                        continue; // comment
                    }
                    // group codes 0-9 and 100-102, 105, 300-309 ... keep their blanks (text); numbers are trimmed by the parsers
                    Some(Tag { code, val: Cow::Borrowed(v) })
                }
                None => {
                    self.errors += 1; // code without value at the end of the file
                    None
                }
            };
        }
    }

    /// Next tag of a binary file; comments (group 999) are skipped in a loop, so that a file made of millions of them
    /// cannot exhaust the stack.
    fn next_binary(&mut self) -> Option<Tag<'a>> {
        loop {
            let t = self.next_binary_raw()?;
            if t.code != 999 {
                return Some(t);
            }
        }
    }

    fn next_binary_raw(&mut self) -> Option<Tag<'a>> {
        let d = self.data;
        let mut p = self.pos;
        if p + 2 > d.len() {
            return None;
        }
        let code = i16::from_le_bytes([d[p], d[p + 1]]) as i32;
        p += 2;
        let kind = kind_of(code);
        let need = |p: usize, n: usize| p + n <= d.len();
        let val: Vec<u8> = match kind {
            Kind::Str => {
                let end = memchr::memchr(0, &d[p..]).map(|i| p + i);
                match end {
                    Some(e) => {
                        let v = d[p..e].to_vec();
                        p = e + 1;
                        v
                    }
                    None => {
                        self.errors += 1;
                        self.pos = d.len();
                        return None;
                    }
                }
            }
            Kind::Int16 => {
                if !need(p, 2) {
                    self.errors += 1;
                    self.pos = d.len();
                    return None;
                }
                let v = i16::from_le_bytes([d[p], d[p + 1]]);
                p += 2;
                v.to_string().into_bytes()
            }
            Kind::Int32 => {
                if !need(p, 4) {
                    self.errors += 1;
                    self.pos = d.len();
                    return None;
                }
                let v = i32::from_le_bytes([d[p], d[p + 1], d[p + 2], d[p + 3]]);
                p += 4;
                v.to_string().into_bytes()
            }
            Kind::Int64 => {
                if !need(p, 8) {
                    self.errors += 1;
                    self.pos = d.len();
                    return None;
                }
                let mut a = [0u8; 8];
                a.copy_from_slice(&d[p..p + 8]);
                p += 8;
                i64::from_le_bytes(a).to_string().into_bytes()
            }
            Kind::Float => {
                if !need(p, 8) {
                    self.errors += 1;
                    self.pos = d.len();
                    return None;
                }
                let mut a = [0u8; 8];
                a.copy_from_slice(&d[p..p + 8]);
                p += 8;
                format!("{:?}", f64::from_le_bytes(a)).into_bytes()
            }
            Kind::Bool => {
                if !need(p, 1) {
                    self.errors += 1;
                    self.pos = d.len();
                    return None;
                }
                let v = d[p];
                p += 1;
                v.to_string().into_bytes()
            }
            Kind::Chunk => {
                if !need(p, 1) {
                    self.errors += 1;
                    self.pos = d.len();
                    return None;
                }
                let n = d[p] as usize;
                p += 1;
                if !need(p, n) {
                    self.errors += 1;
                    self.pos = d.len();
                    return None;
                }
                let v = d[p..p + n].to_vec();
                p += n;
                v
            }
        };
        self.pos = p;
        Some(Tag { code, val: Cow::Owned(val) })
    }
}

/// Text encoding of the DXF strings.
#[derive(Clone, Copy)]
pub struct TextEncoding(pub &'static encoding_rs::Encoding);

impl TextEncoding {
    pub fn utf8() -> Self {
        TextEncoding(encoding_rs::UTF_8)
    }

    /// `$DWGCODEPAGE` value ("ANSI_1250", "dos437", "iso8859-2" ...) to an encoding; None when unknown.
    pub fn from_codepage(name: &str) -> Option<TextEncoding> {
        let n = name.trim().to_ascii_lowercase();
        let label: String = match n.as_str() {
            "" => return None,
            "ascii" | "ansi_1252" | "us-ascii" => "windows-1252".into(),
            "utf-8" | "utf8" => "utf-8".into(),
            "iso8859-1" | "iso-8859-1" | "latin1" => "windows-1252".into(),
            "iso8859-2" => "iso-8859-2".into(),
            "iso8859-3" => "iso-8859-3".into(),
            "iso8859-4" => "iso-8859-4".into(),
            "iso8859-5" => "iso-8859-5".into(),
            "iso8859-6" => "iso-8859-6".into(),
            "iso8859-7" => "iso-8859-7".into(),
            "iso8859-8" => "iso-8859-8".into(),
            "iso8859-9" => "windows-1254".into(),
            "dos437" | "dos850" | "dos852" | "dos855" | "dos857" | "dos860" | "dos861" | "dos863" | "dos864"
            | "dos865" | "dos869" | "dos932" | "dos866" | "dos862" => dos_label(&n)?,
            "mac-roman" => "macintosh".into(),
            "big5" | "ansi_950" => "big5".into(),
            "gb2312" | "ansi_936" | "gbk" => "gbk".into(),
            "ksc5601" | "ansi_949" => "euc-kr".into(),
            "johab" | "ansi_1361" => "euc-kr".into(),
            "ansi_932" | "shiftjis" | "dos932x" => "shift_jis".into(),
            "ansi_874" | "dos874" => "windows-874".into(),
            other => {
                if let Some(num) = other.strip_prefix("ansi_") {
                    format!("windows-{}", num)
                } else if let Some(num) = other.strip_prefix("cp") {
                    format!("windows-{}", num)
                } else {
                    return None;
                }
            }
        };
        encoding_rs::Encoding::for_label(label.as_bytes()).map(TextEncoding)
    }

    pub fn decode(&self, bytes: &[u8]) -> String {
        if bytes.iter().all(|b| b.is_ascii()) {
            // SAFETY-free fast path: ASCII is valid UTF-8
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let (s, _, _) = self.0.decode(bytes);
        s.into_owned()
    }
}

fn dos_label(name: &str) -> Option<String> {
    let num = name.strip_prefix("dos")?;
    // encoding_rs knows IBM866 only; the other DOS pages have no label: use the closest Windows page
    Some(
        match num {
            "866" => "ibm866",
            "437" | "850" | "860" | "861" | "863" | "865" => "windows-1252",
            "852" => "windows-1250",
            "855" | "866x" => "windows-1251",
            "857" => "windows-1254",
            "862" => "windows-1255",
            "864" => "windows-1256",
            "869" => "windows-1253",
            "932" => "shift_jis",
            _ => return None,
        }
        .to_string(),
    )
}

/// Replaces `\U+XXXX` escapes of old DXF files by the character (also surrogate pairs written as two escapes).
pub fn unescape_unicode(s: &str) -> String {
    if !s.contains("\\U+") && !s.contains("\\u+") {
        return s.to_string();
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut pending_high: Option<u32> = None;
    while i < b.len() {
        if i + 7 <= b.len() && b[i] == b'\\' && (b[i + 1] == b'U' || b[i + 1] == b'u') && b[i + 2] == b'+' {
            if let Ok(h) = std::str::from_utf8(&b[i + 3..i + 7]) {
                if let Ok(cp) = u32::from_str_radix(h, 16) {
                    if (0xD800..0xDC00).contains(&cp) {
                        pending_high = Some(cp);
                    } else if (0xDC00..0xE000).contains(&cp) {
                        if let Some(hi) = pending_high.take() {
                            let c = 0x10000 + ((hi - 0xD800) << 10) + (cp - 0xDC00);
                            if let Some(ch) = char::from_u32(c) {
                                out.push(ch);
                            }
                        }
                    } else if let Some(ch) = char::from_u32(cp) {
                        out.push(ch);
                    }
                    i += 7;
                    continue;
                }
            }
        }
        // copy one UTF-8 char
        let ch_len = utf8_len(b[i]);
        out.push_str(&s[i..(i + ch_len).min(s.len())]);
        i += ch_len;
    }
    out
}

fn utf8_len(first: u8) -> usize {
    match first {
        0..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_tags_with_crlf_and_garbage() {
        let data = b"  0\r\nSECTION\r\ngarbage\r\n  2\r\nHEADER\r\n";
        let mut r = TagReader::new(data);
        let t = r.next_tag().unwrap();
        assert_eq!((t.code, &*t.val), (0, &b"SECTION"[..]));
        let t = r.next_tag().unwrap();
        assert_eq!((t.code, &*t.val), (2, &b"HEADER"[..]));
        assert!(r.next_tag().is_none());
        assert_eq!(r.errors, 1);
    }

    #[test]
    fn number_parsing_is_lenient() {
        assert_eq!(parse_f64(b" 1.5 "), 1.5);
        assert_eq!(parse_f64(b"1.0D2"), 100.0);
        assert_eq!(parse_f64(b"abc"), 0.0);
        assert_eq!(parse_i64(b" 12 "), 12);
        assert_eq!(parse_i64(b"3.0"), 3);
        assert_eq!(parse_f64(b"NaN"), 0.0);
    }

    #[test]
    fn binary_dxf_values() {
        let mut d = BINARY_SENTINEL.to_vec();
        d.extend_from_slice(&0i16.to_le_bytes());
        d.extend_from_slice(b"LINE\0");
        d.extend_from_slice(&10i16.to_le_bytes());
        d.extend_from_slice(&2.5f64.to_le_bytes());
        d.extend_from_slice(&70i16.to_le_bytes());
        d.extend_from_slice(&7i16.to_le_bytes());
        let mut r = TagReader::new(&d);
        assert!(r.is_binary());
        let t = r.next_tag().unwrap();
        assert_eq!((t.code, &*t.val), (0, &b"LINE"[..]));
        assert_eq!(r.next_tag().unwrap().f64(), 2.5);
        assert_eq!(r.next_tag().unwrap().i64(), 7);
        assert!(r.next_tag().is_none());
    }

    #[test]
    fn a_million_binary_comments_are_skipped_without_recursion() {
        // 1 000 000 comments (group 999, an empty string each): a recursive reader needs a stack frame for each of them
        let mut d = BINARY_SENTINEL.to_vec();
        for _ in 0..1_000_000 {
            d.extend_from_slice(&999i16.to_le_bytes());
            d.push(0);
        }
        d.extend_from_slice(&0i16.to_le_bytes());
        d.extend_from_slice(b"LINE\0");
        let mut r = TagReader::new(&d);
        let t = r.next_tag().unwrap();
        assert_eq!((t.code, &*t.val), (0, &b"LINE"[..]));
        assert!(r.next_tag().is_none());
        // comments at the very end of the data end the stream
        let mut d = BINARY_SENTINEL.to_vec();
        for _ in 0..10 {
            d.extend_from_slice(&999i16.to_le_bytes());
            d.push(0);
        }
        assert!(TagReader::new(&d).next_tag().is_none());
    }

    #[test]
    fn codepages() {
        let e = TextEncoding::from_codepage("ANSI_1250").unwrap();
        assert_eq!(e.decode(&[0xE8, 0xF8, 0x9E]), "čřž");
        let e = TextEncoding::from_codepage("ANSI_1252").unwrap();
        assert_eq!(e.decode(&[0xE9]), "é");
        assert!(TextEncoding::from_codepage("nonsense").is_none());
        assert_eq!(unescape_unicode("A\\U+00E9B\\U+4E2D"), "AéB中");
    }
}
