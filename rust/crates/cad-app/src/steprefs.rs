//! Refusal of STEP files that point to external documents on a network path (`thumbs.remote_step_refs`).
//!
//! Open CASCADE tries to read external references, and touching `\\server\share` from an automatic thumbnail would
//! make Windows send the user's NTLM credentials to that server. The file is scanned in 8 MiB pieces, never read
//! whole. First pass: external-document keywords (absent from ordinary files, then that is all). Only if present,
//! the second pass looks for a network path, a URL or an escaped slash anywhere in the file; it keeps its state from one
//! piece to the next (`NetScanner`), so no match can hide behind a piece boundary, however long the escape or the scheme is.
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// `_EXTERNAL_KEYWORDS`, and the short entity names of ISO 10303-21 that Open CASCADE reads for the same entities
/// (`#5=DCMFL(...)` is a `DOCUMENT_FILE`, `#6=EXTSRC(...)` an `EXTERNAL_SOURCE`). The short names are the ones in the
/// tables of the STEP reader of Open CASCADE 8.0 (`TKDESTEP`), so a file that spells the entities short is scanned
/// as well:
/// * `DCMFL` DOCUMENT_FILE, `EXTSRC` EXTERNAL_SOURCE, `EXIDAS` EXTERNAL_IDENTIFICATION_ASSIGNMENT;
/// * `EXDFIT` EXTERNALLY_DEFINED_ITEM, `EXDFSY` ..._SYMBOL, `EDHS` ..._HATCH_STYLE, `EDCF` ..._CURVE_FONT,
///   `EDTS` ..._TILE_STYLE, `EDTF` ..._TEXT_FONT, `EDGP` ..._GENERAL_PROPERTY.
///
/// The keyword pass only decides whether the second pass runs, so a name that also turns up in ordinary data costs one
/// more read of the file and nothing else.
pub const EXTERNAL_KEYWORDS: [&[u8]; 13] = [
    b"DOCUMENT_FILE",
    b"EXTERNAL_SOURCE",
    b"EXTERNALLY_DEFINED",
    b"DCMFL",
    b"EXTSRC",
    b"EXIDAS",
    b"EXDFIT",
    b"EXDFSY",
    b"EDHS",
    b"EDCF",
    b"EDTS",
    b"EDTF",
    b"EDGP",
];
/// `_PIECE`: bytes read at a time.
pub const PIECE: usize = 8 << 20;
/// Overlap kept between two pieces for the keyword pass (the longest keyword has 18 bytes).
const KEYWORD_TAIL: usize = 32;

/// Case-insensitive ASCII substring search.
fn contains_ci(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    hay.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

/// True when `buf` contains one of the external-document keywords.
pub fn has_keyword(buf: &[u8]) -> bool {
    EXTERNAL_KEYWORDS.iter().any(|k| contains_ci(buf, k))
}

fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

/// A path separator of Windows: both kinds of slash mean the same.
fn is_separator(b: u8) -> bool {
    matches!(b, b'\\' | b'/')
}

/// State of the search for a network reference, kept from one byte to the next (and so from one piece of the file to the
/// next). The pattern of `thumbs.remote_step_refs`, case-insensitive:
/// `'(?:[\\/]{2}|[A-Za-z][A-Za-z0-9+.-]*://)` (a quote and two separators in any mix of `\` and `/`, because Windows
/// takes `/\srv\share` for a network path as well as `\\srv\share`) or `\X\5C` / `\X\2F` or `\X2\` / `\X4\` followed
/// by groups of four hex digits up to a group `005C` or `002F`.
///
/// None of it is limited in length (a scheme of a thousand letters, a run of a million hex groups), so no fixed overlap
/// between two pieces can be enough: the matches that are under way are the state. The pattern after a quote can only be
/// under way once at a time (a new quote starts it again); the patterns after a backslash can be under way together
/// (`\X\X\5C` holds the start of a second one inside the first), so they are a set of states (`esc`).
#[derive(Clone, Copy, Debug, Default)]
pub struct NetScanner {
    quote: Quote,
    /// Bit set of the `E_*` states of the escape patterns that are under way.
    esc: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Quote {
    #[default]
    Idle,
    /// after the quote
    Opened,
    /// after the quote and one separator
    Separator,
    /// in a scheme (a letter, then letters, digits, `+`, `.`, `-`)
    Scheme,
    /// after the scheme and a colon
    Colon,
    /// after `:/`
    Slash,
}

// states of the patterns that start with a backslash
/// `\`
const E_SLASH: u16 = 1 << 0;
/// `\X`
const E_X: u16 = 1 << 1;
/// `\X\` (then `5C` or `2F`)
const E_X_SLASH: u16 = 1 << 2;
/// `\X\5`
const E_5: u16 = 1 << 3;
/// `\X\2`
const E_2: u16 = 1 << 4;
/// `\X2` or `\X4`
const E_X24: u16 = 1 << 5;
/// at the start of a group of four hex digits (after `\X2\`, `\X4\` or a whole group)
const E_G0: u16 = 1 << 6;
/// one, two or three digits into a group that is neither `005C` nor `002F`
const E_G1: u16 = 1 << 7;
const E_G2: u16 = 1 << 8;
const E_G3: u16 = 1 << 9;
/// `0` of a group that may still be `005C` or `002F`
const E_Z1: u16 = 1 << 10;
/// `00`
const E_Z2: u16 = 1 << 11;
/// `005`
const E_Z3A: u16 = 1 << 12;
/// `002`
const E_Z3B: u16 = 1 << 13;

impl NetScanner {
    pub fn new() -> NetScanner {
        NetScanner::default()
    }

    /// Takes the next piece of the file. True as soon as a network reference is complete; the scanner is then spent.
    pub fn feed(&mut self, piece: &[u8]) -> bool {
        for &b in piece {
            if self.step(b) {
                return true;
            }
        }
        false
    }

    fn step(&mut self, b: u8) -> bool {
        // the pattern after a quote
        self.quote = if b == b'\'' {
            Quote::Opened
        } else {
            match self.quote {
                Quote::Idle => Quote::Idle,
                Quote::Opened if is_separator(b) => Quote::Separator,
                Quote::Opened if b.is_ascii_alphabetic() => Quote::Scheme,
                Quote::Separator if is_separator(b) => return true,
                Quote::Scheme if b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-') => Quote::Scheme,
                Quote::Scheme if b == b':' => Quote::Colon,
                Quote::Colon if b == b'/' => Quote::Slash,
                Quote::Slash if b == b'/' => return true,
                _ => Quote::Idle,
            }
        };
        // the patterns after a backslash
        let hex = is_hex(b);
        let mut next = 0u16;
        let mut set = self.esc;
        while set != 0 {
            let state = set & set.wrapping_neg();
            set &= set - 1;
            next |= match state {
                E_SLASH if b.eq_ignore_ascii_case(&b'X') => E_X,
                E_X if b == b'\\' => E_X_SLASH,
                E_X if b == b'2' || b == b'4' => E_X24,
                E_X_SLASH if b == b'5' => E_5,
                E_X_SLASH if b == b'2' => E_2,
                E_5 if b.eq_ignore_ascii_case(&b'C') => return true,
                E_2 if b.eq_ignore_ascii_case(&b'F') => return true,
                E_X24 if b == b'\\' => E_G0,
                E_G0 if b == b'0' => E_Z1,
                E_G0 if hex => E_G1,
                E_G1 if hex => E_G2,
                E_G2 if hex => E_G3,
                E_G3 if hex => E_G0,
                E_Z1 if b == b'0' => E_Z2,
                E_Z1 if hex => E_G2,
                E_Z2 if b == b'5' => E_Z3A,
                E_Z2 if b == b'2' => E_Z3B,
                E_Z2 if hex => E_G3,
                E_Z3A if b.eq_ignore_ascii_case(&b'C') => return true,
                E_Z3B if b.eq_ignore_ascii_case(&b'F') => return true,
                E_Z3A | E_Z3B if hex => E_G0,
                _ => 0,
            };
        }
        if b == b'\\' {
            next |= E_SLASH;
        }
        self.esc = next;
        false
    }
}

/// True when `buf` as a whole contains a network reference (see [`NetScanner`]); the program reads files through
/// `scan_network`.
#[cfg(test)]
pub fn find_network_reference(buf: &[u8]) -> bool {
    NetScanner::new().feed(buf)
}

/// Fills `data` from `r` as far as the reader goes; the number of bytes (less than `data.len()` only at the end).
fn read_piece(r: &mut impl Read, data: &mut [u8]) -> io::Result<usize> {
    let mut got = 0;
    while got < data.len() {
        let k = r.read(&mut data[got..])?;
        if k == 0 {
            break;
        }
        got += k;
    }
    Ok(got)
}

/// Scans a reader in pieces: `test` is applied to the piece with `tail` bytes of the previous one in front. For patterns
/// of a known length (the keywords); what has no limit goes through a [`NetScanner`] (`scan_network`).
fn scan(mut r: impl Read, piece: usize, tail: usize, test: impl Fn(&[u8]) -> bool) -> io::Result<bool> {
    let mut carry: Vec<u8> = Vec::new();
    let mut data = vec![0u8; piece];
    loop {
        let got = read_piece(&mut r, &mut data)?;
        if got == 0 {
            return Ok(false);
        }
        let mut buf = std::mem::take(&mut carry);
        buf.extend_from_slice(&data[..got]);
        if test(&buf) {
            return Ok(true);
        }
        let from = buf.len().saturating_sub(tail);
        carry = buf[from..].to_vec();
        if got < piece {
            return Ok(false);
        }
    }
}

/// Scans a reader in pieces for a network reference; the scanner carries what is under way from one piece to the next.
fn scan_network(mut r: impl Read, piece: usize) -> io::Result<bool> {
    let mut scanner = NetScanner::new();
    let mut data = vec![0u8; piece];
    loop {
        let got = read_piece(&mut r, &mut data)?;
        if got == 0 {
            return Ok(false);
        }
        if scanner.feed(&data[..got]) {
            return Ok(true);
        }
        if got < piece {
            return Ok(false);
        }
    }
}

/// [`remote_step_refs`] over any reader, with the piece size given (for tests).
pub fn remote_refs_in(open: impl Fn() -> io::Result<Box<dyn Read>>, piece: usize) -> io::Result<bool> {
    if !scan(open()?, piece, KEYWORD_TAIL, has_keyword)? {
        return Ok(false);
    }
    scan_network(open()?, piece)
}

/// True when a STEP file references external documents on a network path / URL.
pub fn remote_step_refs(path: &Path) -> io::Result<bool> {
    remote_refs_in(|| Ok(Box::new(File::open(path)?) as Box<dyn Read>), PIECE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn check(data: &[u8], piece: usize) -> bool {
        let data = data.to_vec();
        remote_refs_in(|| Ok(Box::new(Cursor::new(data.clone())) as Box<dyn Read>), piece).unwrap()
    }

    #[test]
    fn plain_model_is_fine() {
        assert!(!check(b"ISO-10303-21;\nDATA;\n#1=CARTESIAN_POINT('',(0.,0.,0.));\nENDSEC;\n", PIECE));
    }

    #[test]
    fn keyword_without_network_path_is_fine() {
        assert!(!check(b"#5=DOCUMENT_FILE('a','b','c.step');\n", PIECE));
    }

    #[test]
    fn unc_path_and_urls_are_refused() {
        assert!(check(b"#5=DOCUMENT_FILE('\\\\\\\\server\\\\share\\\\x.step');", PIECE));
        assert!(check(b"#5=EXTERNAL_SOURCE('//server/x.step');", PIECE));
        assert!(check(b"#5=EXTERNALLY_DEFINED_ITEM('http://host/x.step');", PIECE));
        assert!(check(b"#5=externally_defined_item('FILE://host/x.step');", PIECE));
        assert!(check(b"#5=DOCUMENT_FILE('ftp+x.y-z://host/x');", PIECE));
    }

    #[test]
    fn short_entity_names_of_open_cascade_are_scanned() {
        // the STEP reader of Open CASCADE also takes `DCMFL` for DOCUMENT_FILE and `EXTSRC` for EXTERNAL_SOURCE
        assert!(check(b"#5=DCMFL('//server/x.step');", PIECE));
        assert!(check(b"#5=EXTSRC('\\\\\\\\server\\\\share\\\\x.step');", PIECE));
        assert!(check(b"#5=extsrc('http://host/x.step');", PIECE));
        for short in ["EXDFIT", "EXDFSY", "EDHS", "EDCF", "EDTS", "EDTF", "EDGP", "EXIDAS"] {
            let line = format!("#7={short}('//server/x');");
            assert!(check(line.as_bytes(), PIECE), "{short}");
            assert!(has_keyword(short.as_bytes()), "{short}");
        }
        // a short name alone, without a network path, is still fine
        assert!(!check(b"#5=DCMFL('x.step');", PIECE));
        assert!(!check(b"#5=EXTSRC('C:\\\\work\\\\x.step');", PIECE));
        // and the long names behave as before
        assert!(check(b"#5=DOCUMENT_FILE('//server/x.step');", PIECE));
    }

    #[test]
    fn any_two_separators_after_the_quote_make_a_network_path() {
        // Windows takes every mix of the two slashes for a UNC path
        for start in ["\\\\\\\\", "//", "/\\\\", "\\\\/"] {
            let text = format!("#5=DOCUMENT_FILE('{start}srv\\\\share\\\\x.step');");
            assert!(check(text.as_bytes(), PIECE), "{start}");
        }
        // the path of the report: '/\srv\share (a slash, then the escaped backslashes of the file)
        assert!(check(b"#5=DOCUMENT_FILE('/\\\\srv\\\\share');", PIECE));
        assert!(find_network_reference(b"('/\\srv"));
        assert!(find_network_reference(b"('\\/srv"));
        // one separator is a path on this drive, not a network path
        assert!(!find_network_reference(b"('/srv/share')"));
        assert!(!find_network_reference(b"('/"));
        assert!(!find_network_reference(b"('"));
        assert!(!find_network_reference(b"('a//b')"));
    }

    #[test]
    fn local_looking_names_are_fine() {
        assert!(!check(b"#5=DOCUMENT_FILE('C:\\\\work\\\\x.step');", PIECE));
        assert!(!check(b"#5=DOCUMENT_FILE('x.step');", PIECE));
        assert!(!check(b"#5=DOCUMENT_FILE('1abc://x');", PIECE));
    }

    #[test]
    fn escaped_slashes_are_refused() {
        assert!(check(b"#5=DOCUMENT_FILE('\\X\\5C\\X\\5Cserver');", PIECE));
        assert!(check(b"#5=DOCUMENT_FILE('a\\X\\2Fb');", PIECE));
        assert!(check(b"#5=DOCUMENT_FILE('\\X2\\00410042005C\\X0\\');", PIECE));
        assert!(check(b"#5=DOCUMENT_FILE('\\X4\\0041002F\\X0\\');", PIECE));
        assert!(!check(b"#5=DOCUMENT_FILE('\\X2\\00410042\\X0\\');", PIECE));
        assert!(!check(b"#5=DOCUMENT_FILE('\\X2\\004100G2005C\\X0\\');", PIECE));
    }

    #[test]
    fn match_across_two_pieces() {
        let mut data = vec![b' '; 100];
        data.extend_from_slice(b"EXTERNAL_SOURCE('//server/x')");
        // every position of the keyword and of the pattern straddles a piece boundary for some piece size
        for piece in 64..140 {
            assert!(check(&data, piece), "piece {piece}");
        }
    }

    /// The search as it was written before the scanner kept a state: one buffer, every position tried (what the fixed overlap
    /// of 256 bytes between two pieces approximated). The scanner must give the same answer on any whole buffer.
    fn reference(buf: &[u8]) -> bool {
        let n = buf.len();
        for i in 0..n {
            match buf[i] {
                b'\'' => {
                    let rest = &buf[i + 1..];
                    if rest.len() >= 2 && is_separator(rest[0]) && is_separator(rest[1]) {
                        return true;
                    }
                    if rest.first().is_some_and(|c| c.is_ascii_alphabetic()) {
                        let mut k = 1;
                        while k < rest.len() && (rest[k].is_ascii_alphanumeric() || matches!(rest[k], b'+' | b'.' | b'-')) {
                            k += 1;
                        }
                        if rest[k..].starts_with(b"://") {
                            return true;
                        }
                    }
                }
                b'\\' => {
                    let rest = &buf[i + 1..];
                    if rest.len() >= 2 && rest[0].eq_ignore_ascii_case(&b'X') {
                        if rest[1] == b'\\' {
                            let hex = &rest[2..];
                            if hex.len() >= 2 && (hex[..2].eq_ignore_ascii_case(b"5C") || hex[..2].eq_ignore_ascii_case(b"2F")) {
                                return true;
                            }
                        }
                        if matches!(rest[1], b'2' | b'4') && rest.get(2) == Some(&b'\\') {
                            let mut hex = &rest[3..];
                            while hex.len() >= 4 && hex[..4].iter().all(|b| is_hex(*b)) {
                                if hex[..4].eq_ignore_ascii_case(b"005C") || hex[..4].eq_ignore_ascii_case(b"002F") {
                                    return true;
                                }
                                hex = &hex[4..];
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        false
    }

    /// Strings made of the characters and of the pieces the patterns are made of, from a fixed seed: half of them single
    /// characters (most of them match nothing), half of them pieces (many matches, and many that stop just short of one).
    fn corpus(count: usize, max_len: usize) -> Vec<Vec<u8>> {
        const ALPHABET: &[u8] = b"'\\/Xx24505CcFf:aZ.+-_ 0003AbG";
        const PIECES: &[&[u8]] = &[
            b"'", b"\\", b"\\X\\", b"\\x2\\", b"\\X4\\", b"\\X2", b"0041", b"005C", b"002f", b"00", b"5C", b"2f", b"//", b"\\\\",
            b"/\\", b"://", b"a", b"ab+.-", b":", b"/", b"X", b"2", b"0", b"G", b"1A2b",
        ];
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        (0..count)
            .map(|i| {
                let len = (next() % (max_len as u64 + 1)) as usize;
                if i % 2 == 0 {
                    (0..len).map(|_| ALPHABET[(next() % ALPHABET.len() as u64) as usize]).collect()
                } else {
                    let mut s = Vec::new();
                    while s.len() < len {
                        s.extend_from_slice(PIECES[(next() % PIECES.len() as u64) as usize]);
                    }
                    s
                }
            })
            .collect()
    }

    #[test]
    fn the_scanner_answers_as_the_whole_buffer_search_did() {
        let mut matches = 0;
        for s in corpus(60_000, 24) {
            let want = reference(&s);
            matches += usize::from(want);
            assert_eq!(find_network_reference(&s), want, "{:?}", String::from_utf8_lossy(&s));
        }
        assert!((1000..55_000).contains(&matches), "the corpus must have matches and misses in it: {matches} of 60000");
    }

    #[test]
    fn a_match_is_found_wherever_the_pieces_are_cut() {
        // every cut of every string, and byte by byte
        for s in corpus(6_000, 20) {
            let want = reference(&s);
            for cut in 0..=s.len() {
                let mut sc = NetScanner::new();
                let got = sc.feed(&s[..cut]) || sc.feed(&s[cut..]);
                assert_eq!(got, want, "{:?} cut at {cut}", String::from_utf8_lossy(&s));
            }
            let mut sc = NetScanner::new();
            let got = s.iter().any(|b| sc.feed(&[*b]));
            assert_eq!(got, want, "{:?} byte by byte", String::from_utf8_lossy(&s));
        }
        // through the reader, with small pieces
        for s in corpus(1_500, 40) {
            let mut text = b"#5=DOCUMENT_FILE(".to_vec();
            text.extend_from_slice(&s);
            for piece in [1, 2, 3, 5, 8, 17, 64] {
                assert_eq!(check(&text, piece), reference(&text), "{:?} in pieces of {piece}", String::from_utf8_lossy(&text));
            }
        }
    }

    #[test]
    fn an_escape_that_starts_far_before_a_piece_boundary_does_not_hide_the_path_after_it() {
        // `\X2\` and a long run of harmless hex groups (more than the 256 bytes of overlap there used to be), then the group
        // 005C: a backslash. The run crosses many pieces.
        for lead in ["\\X2\\", "\\X4\\", "\\x2\\"] {
            let mut data = b"ISO-10303-21;\nDATA;\n#5=DOCUMENT_FILE('".to_vec();
            data.extend_from_slice(lead.as_bytes());
            for _ in 0..2000 {
                data.extend_from_slice(b"0041");
            }
            data.extend_from_slice(b"005C005C\\X0\\x.step');\n");
            for piece in [64, 100, 257, 1000, 4096] {
                assert!(check(&data, piece), "{lead} in pieces of {piece}");
            }
            assert!(check(&data, PIECE));
            // without the group the same file is clean
            let clean = String::from_utf8_lossy(&data).replace("005C005C", "00410041");
            for piece in [64, 257, 4096, PIECE] {
                assert!(!check(clean.as_bytes(), piece), "{lead} clean, pieces of {piece}");
            }
        }
        // a whole run of hex groups that ends exactly at a piece boundary, the match group right after it
        let mut data = b"EXTERNAL_SOURCE(\\X2\\".to_vec();
        data.extend(std::iter::repeat_n(*b"0041", 100).flatten());
        let boundary = data.len();
        data.extend_from_slice(b"002F");
        assert!(check(&data, boundary));
        assert!(check(&data, boundary + 1));
        assert!(check(&data, boundary - 1));
    }

    #[test]
    fn a_url_scheme_longer_than_the_overlap_is_still_found() {
        let mut data = b"#5=EXTERNAL_SOURCE('".to_vec();
        data.extend(std::iter::repeat_n(b'a', 1000));
        data.extend_from_slice(b"://host/x.step');");
        for piece in [16, 100, 500, 999, 1000, 1024, 8192] {
            assert!(check(&data, piece), "pieces of {piece}");
        }
        let clean = String::from_utf8_lossy(&data).replace("://", ":-/");
        assert!(!check(clean.as_bytes(), 100));
    }

    #[test]
    fn the_escape_patterns_may_start_inside_each_other() {
        assert!(find_network_reference(b"\\X\\X\\5C"));
        assert!(find_network_reference(b"\\X2\\0041\\X\\5C"));
        assert!(find_network_reference(b"\\X2\\0041\\X4\\002F"));
        assert!(find_network_reference(b"'\\X\\2F"));
        assert!(!find_network_reference(b"\\X2\\0041\\X\\5D"));
        assert!(!find_network_reference(b"\\X2\\00410\\005C"));
    }

    #[test]
    fn keyword_pass_is_case_insensitive() {
        assert!(has_keyword(b"..document_file.."));
        assert!(has_keyword(b"..External_Source.."));
        assert!(!has_keyword(b"..DOCUMENT_FIL.."));
    }
}
