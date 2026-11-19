//! Launcher page of the CAD viewer for Google Drive ("Open with").
//! See docs/rust-port/ARCHITECTURE.md, section "Google Drive launcher (all Rust)".
//!
//! Drive opens the page registered as "Open URL" of the Drive UI integration and adds the
//! query parameter `state={"ids":["<fileId>"],"action":"open",...}`. The page checks the value
//! and redirects to `cadviewer://open?fileId=<fileId>`, which starts the installed viewer. If
//! the viewer does not start, a Czech help text with the download link appears after a short
//! delay. Input that does not pass the checks shows a Czech error and never redirects.
//!
//! The logic is plain Rust and tested natively ([`plan`] and the functions below it). Only
//! the small DOM code at the end is compiled for `wasm32`.
//!
//! Build of the static site (from the repository root, after `. .\rust\tools\devenv.ps1 web`):
//!
//! ```text
//! cargo build --manifest-path rust\Cargo.toml -p cad-drive-web --release --target wasm32-unknown-unknown
//! wasm-bindgen --target web --no-typescript --out-dir <site>\pkg `
//!     <target dir>\wasm32-unknown-unknown\release\cad_drive_web.wasm
//! copy rust\crates\cad-drive-web\{index.html,style.css,loader.js} <site>
//! ```
//!
//! `wasm-bindgen` is the command line tool whose version matches the `wasm-bindgen` crate in
//! `Cargo.lock`. The download link shown in the help text is build-time configuration
//! (`CADVIEWER_DOWNLOAD_URL`); nothing is committed. Serve the site over HTTPS. If the host
//! can send headers, add `Content-Security-Policy: frame-ancestors 'none'` as well, because
//! that directive is ignored inside a `<meta>` tag.

use serde::de::{Error as _, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// Scheme and command of the link that starts the viewer.
pub const LINK_PREFIX: &str = "cadviewer://open?fileId=";
/// How long the page waits after the redirect before it shows the help text.
pub const HELP_DELAY_MS: i32 = 2500;
/// Longest accepted query string of the page (encoded).
pub const MAX_SEARCH_LEN: usize = 4096;
/// Longest accepted `state` value after decoding.
pub const MAX_STATE_LEN: usize = 2048;
/// Download link of the viewer, baked in at build time if provided.
pub const DOWNLOAD_URL: Option<&str> = option_env!("CADVIEWER_DOWNLOAD_URL");

/// Ids of the elements of `index.html` that the page script changes.
pub const ID_STATUS: &str = "status";
pub const ID_ERROR: &str = "error";
pub const ID_ERROR_TEXT: &str = "error-text";
pub const ID_HELP: &str = "help";
pub const ID_OPEN_LINK: &str = "open-link";
pub const ID_DOWNLOAD_LINK: &str = "download-link";
pub const ID_DOWNLOAD_MISSING: &str = "download-missing";
pub const DOM_IDS: [&str; 7] = [
    ID_STATUS,
    ID_ERROR,
    ID_ERROR_TEXT,
    ID_HELP,
    ID_OPEN_LINK,
    ID_DOWNLOAD_LINK,
    ID_DOWNLOAD_MISSING,
];

/// Czech texts that the script writes into the page.
pub mod text {
    pub const OPENING: &str =
        "Otevírám soubor v CAD prohlížeči. Pokud se zobrazí dotaz, potvrďte spuštění aplikace.";
    pub const SCRIPT_FAILED: &str =
        "Stránku se nepodařilo spustit. Povolte JavaScript a WebAssembly, nebo zkuste jiný prohlížeč.";
}

/// Why the `state` parameter was rejected. `Display` is the Czech message for the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateError {
    Missing,
    Duplicate,
    TooLong,
    BadEncoding,
    BadJson,
    WrongAction,
    NoId,
    MultipleIds,
    BadId,
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            StateError::Missing => "Odkaz z Google Drive neobsahuje údaje o souboru (chybí parametr state).",
            StateError::Duplicate => "Odkaz z Google Drive obsahuje údaje o souboru vícekrát.",
            StateError::TooLong => "Odkaz z Google Drive je příliš dlouhý.",
            StateError::BadEncoding => "Údaje o souboru v odkazu jsou poškozené (nesprávné kódování).",
            StateError::BadJson => "Údaje o souboru v odkazu nelze přečíst.",
            StateError::WrongAction => "Odkaz z Google Drive nepožaduje otevření souboru (akce není „open“).",
            StateError::NoId => "Odkaz z Google Drive neobsahuje žádný soubor.",
            StateError::MultipleIds => "Najednou lze otevřít jen jeden soubor, odkaz jich obsahuje více.",
            StateError::BadId => "Identifikátor souboru v odkazu není platný.",
        })
    }
}

impl std::error::Error for StateError {}

/// `^[A-Za-z0-9_-]{10,128}$`, the same rule as `cad_drive::is_valid_file_id` (a test compares
/// the two).
pub fn is_valid_file_id(id: &str) -> bool {
    (10..=128).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `cadviewer://open?fileId=<id>` for a valid id.
pub fn build_link(file_id: &str) -> Result<String, StateError> {
    if !is_valid_file_id(file_id) {
        return Err(StateError::BadId);
    }
    Ok(format!("{LINK_PREFIX}{file_id}"))
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Strict form decoding: `+` is a space, a bad escape or a result that is not UTF-8 is an error.
fn decode_component(s: &str) -> Option<String> {
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

/// Takes the value of the one `state` parameter from `location.search` (with or without the
/// leading `?`). Other parameters are ignored. Zero or several `state` parameters are errors.
pub fn state_from_search(search: &str) -> Result<String, StateError> {
    if search.len() > MAX_SEARCH_LEN {
        return Err(StateError::TooLong);
    }
    let query = search.strip_prefix('?').unwrap_or(search);
    let mut found: Option<&str> = None;
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode_component(key).ok_or(StateError::BadEncoding)?;
        if key == "state" {
            if found.is_some() {
                return Err(StateError::Duplicate);
            }
            found = Some(value);
        }
    }
    let raw = found.ok_or(StateError::Missing)?;
    let decoded = decode_component(raw).ok_or(StateError::BadEncoding)?;
    if decoded.len() > MAX_STATE_LEN {
        return Err(StateError::TooLong);
    }
    Ok(decoded)
}

/// The two fields of Drive's `state` that matter. Other fields (`userId`, `resourceKeys`, ...)
/// are ignored. It must be a JSON object (the sequence form that derived code would also
/// accept is refused), and a repeated `ids` or `action` key is an error.
struct DriveState {
    action: Option<Value>,
    ids: Option<Value>,
}

impl<'de> Deserialize<'de> for DriveState {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StateVisitor;

        impl<'de> Visitor<'de> for StateVisitor {
            type Value = DriveState;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<DriveState, A::Error> {
                let (mut action, mut ids) = (None, None);
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "action" if action.is_some() => return Err(A::Error::duplicate_field("action")),
                        "action" => action = Some(map.next_value::<Value>()?),
                        "ids" if ids.is_some() => return Err(A::Error::duplicate_field("ids")),
                        "ids" => ids = Some(map.next_value::<Value>()?),
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(DriveState { action, ids })
            }
        }

        deserializer.deserialize_map(StateVisitor)
    }
}

/// Parses `{"ids":["<id>"],"action":"open", ...}` and returns the one file id.
pub fn parse_state(state_json: &str) -> Result<String, StateError> {
    if state_json.len() > MAX_STATE_LEN {
        return Err(StateError::TooLong);
    }
    let state: DriveState = serde_json::from_str(state_json).map_err(|_| StateError::BadJson)?;
    match state.action {
        Some(Value::String(a)) if a == "open" => {}
        _ => return Err(StateError::WrongAction),
    }
    let ids = match state.ids {
        Some(Value::Array(ids)) => ids,
        _ => return Err(StateError::NoId),
    };
    match ids.as_slice() {
        [] => Err(StateError::NoId),
        [Value::String(id)] if is_valid_file_id(id) => Ok(id.clone()),
        [_] => Err(StateError::BadId),
        _ => Err(StateError::MultipleIds),
    }
}

/// The whole check: query string of the page in, `cadviewer://open?fileId=<id>` out.
pub fn link_from_search(search: &str) -> Result<String, StateError> {
    let state = state_from_search(search)?;
    let id = parse_state(&state)?;
    build_link(&id)
}

/// What the page does with its address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageAction {
    /// Redirect to `link`, then show the help text after [`HELP_DELAY_MS`].
    Redirect { link: String },
    /// Show `message` and do nothing else.
    Error { message: String },
}

/// Decides what the page does for a given `location.search`. An error never leads to a redirect.
pub fn plan(search: &str) -> PageAction {
    match link_from_search(search) {
        Ok(link) => PageAction::Redirect { link },
        Err(e) => PageAction::Error { message: e.to_string() },
    }
}

/// Accepts only an absolute `https://` address without user info, white space, quotes or
/// angle brackets. Anything else is not shown as a link.
pub fn safe_download_url(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let ok_chars = url.len() <= 2048 && url.bytes().all(|b| (0x21..=0x7E).contains(&b) && !b"\"'<>`\\{}|^".contains(&b));
    let host_ok = !authority.is_empty()
        && !authority.contains('@')
        && authority
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'));
    (ok_chars && host_ok).then_some(url)
}

/// The configured download link if it passes [`safe_download_url`].
pub fn configured_download_url() -> Option<&'static str> {
    DOWNLOAD_URL.and_then(safe_download_url)
}

#[cfg(target_arch = "wasm32")]
mod page {
    //! DOM code. It only shows or hides elements and writes text and two `href` values that
    //! come from the checked functions above. No HTML is ever built from input.

    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;
    use web_sys::{Document, Element, Window};

    use super::*;

    fn element(doc: &Document, id: &str) -> Result<Element, JsValue> {
        doc.get_element_by_id(id)
            .ok_or_else(|| JsValue::from_str(&format!("missing element #{id}")))
    }

    fn set_hidden(el: &Element, hidden: bool) -> Result<(), JsValue> {
        if hidden { el.set_attribute("hidden", "") } else { el.remove_attribute("hidden") }
    }

    fn show_error(doc: &Document, message: &str) -> Result<(), JsValue> {
        element(doc, ID_STATUS).and_then(|e| set_hidden(&e, true))?;
        element(doc, ID_ERROR_TEXT)?.set_text_content(Some(message));
        set_hidden(&element(doc, ID_ERROR)?, false)
    }

    fn show_opening(doc: &Document, link: &str) -> Result<(), JsValue> {
        element(doc, ID_STATUS)?.set_text_content(Some(text::OPENING));
        // The manual link works even if the browser blocks the automatic redirect.
        element(doc, ID_OPEN_LINK)?.set_attribute("href", link)
    }

    fn show_help(doc: &Document) -> Result<(), JsValue> {
        match configured_download_url() {
            Some(url) => {
                let a = element(doc, ID_DOWNLOAD_LINK)?;
                a.set_attribute("href", url)?;
                set_hidden(&a, false)?;
                set_hidden(&element(doc, ID_DOWNLOAD_MISSING)?, true)?;
            }
            None => {
                set_hidden(&element(doc, ID_DOWNLOAD_LINK)?, true)?;
                set_hidden(&element(doc, ID_DOWNLOAD_MISSING)?, false)?;
            }
        }
        set_hidden(&element(doc, ID_HELP)?, false)
    }

    fn run(window: &Window) -> Result<(), JsValue> {
        let doc = window.document().ok_or_else(|| JsValue::from_str("no document"))?;
        let search = window.location().search()?;
        match plan(&search) {
            PageAction::Error { message } => show_error(&doc, &message),
            PageAction::Redirect { link } => {
                show_opening(&doc, &link)?;
                let help_doc = doc.clone();
                let timer = Closure::once_into_js(move || {
                    // Nothing sensible can be done about a failure here.
                    let _ = show_help(&help_doc);
                });
                window.set_timeout_with_callback_and_timeout_and_arguments_0(
                    timer.unchecked_ref::<js_sys::Function>(),
                    HELP_DELAY_MS,
                )?;
                window.location().set_href(&link)
            }
        }
    }

    /// Runs when the module is loaded.
    #[wasm_bindgen(start)]
    pub fn start() -> Result<(), JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
        run(&window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "1AbCdEfGhIjKlMnOpQrStUvWxYz_0123456-";
    const INDEX_HTML: &str = include_str!("../index.html");
    const LOADER_JS: &str = include_str!("../loader.js");
    const STYLE_CSS: &str = include_str!("../style.css");

    fn enc(s: &str) -> String {
        s.bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect()
    }

    fn search_for(state: &str) -> String {
        format!("?state={}", enc(state))
    }

    // ----- state parsing -------------------------------------------------------------------

    #[test]
    fn accepts_drive_open_state() {
        let state = format!(r#"{{"ids":["{ID}"],"action":"open"}}"#);
        assert_eq!(parse_state(&state).unwrap(), ID);
        assert_eq!(link_from_search(&search_for(&state)).unwrap(), format!("cadviewer://open?fileId={ID}"));
    }

    #[test]
    fn accepts_the_extra_fields_drive_really_sends() {
        let state = format!(r#"{{"ids":["{ID}"],"resourceKeys":{{}},"action":"open","userId":"117001234567890123456"}}"#);
        assert_eq!(link_from_search(&search_for(&state)).unwrap(), format!("cadviewer://open?fileId={ID}"));
        // Key order and white space do not matter.
        let state = format!("{{ \"action\" : \"open\" ,\n \"ids\" : [ \"{ID}\" ] }}");
        assert_eq!(parse_state(&state).unwrap(), ID);
    }

    #[test]
    fn rejects_wrong_action() {
        for action in [r#""create""#, r#""Open""#, r#""OPEN""#, r#""open ""#, r#""""#, "null", "true", "1", r#"["open"]"#] {
            let state = format!(r#"{{"ids":["{ID}"],"action":{action}}}"#);
            assert_eq!(parse_state(&state), Err(StateError::WrongAction), "{action}");
        }
        assert_eq!(parse_state(&format!(r#"{{"ids":["{ID}"]}}"#)), Err(StateError::WrongAction));
    }

    #[test]
    fn requires_exactly_one_id() {
        assert_eq!(parse_state(r#"{"ids":[],"action":"open"}"#), Err(StateError::NoId));
        assert_eq!(parse_state(r#"{"action":"open"}"#), Err(StateError::NoId));
        assert_eq!(parse_state(r#"{"ids":null,"action":"open"}"#), Err(StateError::NoId));
        assert_eq!(parse_state(&format!(r#"{{"ids":"{ID}","action":"open"}}"#)), Err(StateError::NoId));
        assert_eq!(parse_state(&format!(r#"{{"ids":{{"0":"{ID}"}},"action":"open"}}"#)), Err(StateError::NoId));
        let two = format!(r#"{{"ids":["{ID}","{ID}"],"action":"open"}}"#);
        assert_eq!(parse_state(&two), Err(StateError::MultipleIds));
        let two_other = format!(r#"{{"ids":["{ID}","ZZZZZZZZZZZZZZZZ"],"action":"open"}}"#);
        assert_eq!(parse_state(&two_other), Err(StateError::MultipleIds));
    }

    #[test]
    fn rejects_bad_ids() {
        for id in [
            "short", "a b c d e f g h i j", "../../etc/passwd", "C:\\x\\y\\z\\q", "https://evil.example/x", "id;rm -rf /xxx",
            "ídčko-s-diakritikou", "a=b&c=d&e=f&g", "1234567890\n", "%41%42%43%44%45%46%47", &"a".repeat(129),
        ] {
            let state = format!(r#"{{"ids":[{}],"action":"open"}}"#, serde_json::to_string(id).unwrap());
            assert_eq!(parse_state(&state), Err(StateError::BadId), "{id:?}");
        }
        for not_a_string in ["5", "null", "true", r#"["x"]"#, r#"{"a":"b"}"#] {
            let state = format!(r#"{{"ids":[{not_a_string}],"action":"open"}}"#);
            assert_eq!(parse_state(&state), Err(StateError::BadId), "{not_a_string}");
        }
        assert!(is_valid_file_id(&"a".repeat(128)));
        assert!(is_valid_file_id("abcdefghij"));
        assert!(!is_valid_file_id("abcdefghi"));
    }

    #[test]
    fn rejects_broken_json() {
        for s in ["", "not json", "{", "[]", "null", "\"open\"", "42", r#"{"ids":["#, "{'ids':['x'],'action':'open'}"] {
            let r = parse_state(s);
            assert!(matches!(r, Err(StateError::BadJson) | Err(StateError::WrongAction)), "{s:?}: {r:?}");
        }
        assert_eq!(parse_state(""), Err(StateError::BadJson));
        assert_eq!(parse_state("not json"), Err(StateError::BadJson));
        // Deep nesting is refused by the parser instead of overflowing the stack.
        let deep = format!(r#"{{"ids":{}{},"action":"open"}}"#, "[".repeat(900), "]".repeat(900));
        assert!(deep.len() < MAX_STATE_LEN);
        assert_eq!(parse_state(&deep), Err(StateError::BadJson));
        // The array form that a derived parser would accept is refused too.
        assert_eq!(parse_state(&format!(r#"["open",["{ID}"]]"#)), Err(StateError::BadJson));
        assert_eq!(parse_state(&format!(r#"[["{ID}"],"open"]"#)), Err(StateError::BadJson));
    }

    #[test]
    fn repeated_keys_are_an_error() {
        let dup_ids = format!(r#"{{"ids":["{ID}"],"ids":["ZZZZZZZZZZZZZZZZ"],"action":"open"}}"#);
        assert_eq!(parse_state(&dup_ids), Err(StateError::BadJson));
        let dup_action = format!(r#"{{"ids":["{ID}"],"action":"open","action":"open"}}"#);
        assert_eq!(parse_state(&dup_action), Err(StateError::BadJson));
    }

    #[test]
    fn state_length_is_limited() {
        let padding = "x".repeat(MAX_STATE_LEN);
        let state = format!(r#"{{"ids":["{ID}"],"action":"open","pad":"{padding}"}}"#);
        assert_eq!(parse_state(&state), Err(StateError::TooLong));
        assert_eq!(link_from_search(&search_for(&state)), Err(StateError::TooLong));
        let huge = format!("?state={}", "a".repeat(MAX_SEARCH_LEN + 1));
        assert_eq!(state_from_search(&huge), Err(StateError::TooLong));
    }

    // ----- query string --------------------------------------------------------------------

    #[test]
    fn state_parameter_extraction() {
        let state = format!(r#"{{"ids":["{ID}"],"action":"open"}}"#);
        let enc_state = enc(&state);
        assert_eq!(state_from_search(&format!("?state={enc_state}")).unwrap(), state);
        assert_eq!(state_from_search(&format!("state={enc_state}")).unwrap(), state);
        // Other parameters are ignored, in any position.
        assert_eq!(state_from_search(&format!("?a=1&state={enc_state}&b=2")).unwrap(), state);
        assert_eq!(state_from_search(&format!("?authuser=0&&state={enc_state}")).unwrap(), state);
        // Spaces may come as plus signs.
        assert_eq!(state_from_search("?state=%7B%22a%22%3A+1%7D").unwrap(), r#"{"a": 1}"#);
        // The key may be percent-encoded.
        assert_eq!(state_from_search("?%73tate=x").unwrap(), "x");
    }

    #[test]
    fn state_parameter_must_appear_once() {
        assert_eq!(state_from_search(""), Err(StateError::Missing));
        assert_eq!(state_from_search("?"), Err(StateError::Missing));
        assert_eq!(state_from_search("?a=b"), Err(StateError::Missing));
        assert_eq!(state_from_search("?State=x"), Err(StateError::Missing));
        assert_eq!(state_from_search("?state=a&state=b"), Err(StateError::Duplicate));
        assert_eq!(state_from_search("?state=a&%73tate=b"), Err(StateError::Duplicate));
        assert_eq!(state_from_search("?state=a&state="), Err(StateError::Duplicate));
        assert_eq!(state_from_search("?state=%zz"), Err(StateError::BadEncoding));
        assert_eq!(state_from_search("?state=%C3"), Err(StateError::BadEncoding));
        assert_eq!(state_from_search("?%zz=1&state=x"), Err(StateError::BadEncoding));
        // An empty value is a value; the JSON check rejects it afterwards.
        assert_eq!(state_from_search("?state=").unwrap(), "");
        assert_eq!(link_from_search("?state="), Err(StateError::BadJson));
    }

    #[test]
    fn plan_redirects_only_for_valid_input() {
        let good = format!(r#"{{"ids":["{ID}"],"action":"open"}}"#);
        assert_eq!(
            plan(&search_for(&good)),
            PageAction::Redirect { link: format!("cadviewer://open?fileId={ID}") }
        );
        let bad_inputs = [
            String::new(),
            "?state=".to_owned(),
            "?state=%7B%7D".to_owned(),
            "?id=abc".to_owned(),
            "?fileId=1AbCdEfGhIjKlMnOpQrStUvWxYz".to_owned(),
            search_for(r#"{"ids":["short"],"action":"open"}"#),
            search_for(r#"{"ids":["a","b"],"action":"open"}"#),
            search_for(&format!(r#"{{"ids":["{ID}"],"action":"create"}}"#)),
            search_for(&format!(r#"{{"ids":["{ID}/../x"],"action":"open"}}"#)),
            format!("?state={}&state={}", enc(&good), enc(&good)),
            format!("?state={}", "%7B".repeat(5000)),
            "?state=%00".to_owned(),
        ];
        for input in &bad_inputs {
            match plan(input) {
                PageAction::Error { message } => {
                    assert!(!message.is_empty(), "{input}");
                    assert!(message.ends_with('.'), "{message}");
                }
                other => panic!("{input:?} gave {other:?}"),
            }
        }
        // A link is only ever produced from a valid id.
        for input in &bad_inputs {
            assert!(link_from_search(input).is_err(), "{input}");
        }
    }

    #[test]
    fn built_link_never_contains_anything_but_the_id() {
        let link = build_link(ID).unwrap();
        assert_eq!(link, format!("cadviewer://open?fileId={ID}"));
        assert!(link.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-:/?=".contains(&b)));
        for bad in ["", "x", "a b", "a&b=c", "https://x", "../x"] {
            assert_eq!(build_link(bad), Err(StateError::BadId), "{bad}");
        }
    }

    #[test]
    fn id_rule_matches_the_native_crate() {
        let samples: Vec<String> = vec![
            ID.into(), "abcdefghij".into(), "abcdefghi".into(), "a".repeat(128), "a".repeat(129), "".into(),
            "id with space".into(), "id.with.dots.x".into(), "ídčko-diakritika".into(), "ABC_def-123456".into(),
            "1234567890\n".into(), "a/b/c/d/e/f/g/h".into(), "-_-_-_-_-_-_".into(),
        ];
        for s in &samples {
            assert_eq!(is_valid_file_id(s), cad_drive::is_valid_file_id(s), "{s:?}");
        }
        // Both crates build the same link text.
        assert_eq!(build_link(ID).unwrap(), cad_drive::DriveLink::new(ID).unwrap().to_url());
        assert_eq!(LINK_PREFIX, format!("{}://{}?{}=", cad_drive::LINK_SCHEME, cad_drive::LINK_COMMAND, cad_drive::LINK_PARAM));
        // And the native parser accepts what this page produces.
        let link = link_from_search(&search_for(&format!(r#"{{"ids":["{ID}"],"action":"open"}}"#))).unwrap();
        assert_eq!(cad_drive::parse_link(&link).unwrap().file_id(), ID);
    }

    // ----- messages and download link ------------------------------------------------------

    #[test]
    fn every_error_has_a_czech_message() {
        for e in [
            StateError::Missing, StateError::Duplicate, StateError::TooLong, StateError::BadEncoding,
            StateError::BadJson, StateError::WrongAction, StateError::NoId, StateError::MultipleIds, StateError::BadId,
        ] {
            let m = e.to_string();
            assert!(m.starts_with(|c: char| c.is_uppercase()) && m.ends_with('.'), "{m}");
        }
        assert_eq!(StateError::BadId.to_string(), "Identifikátor souboru v odkazu není platný.");
        assert!(text::OPENING.contains("CAD prohlížeči"));
    }

    #[test]
    fn download_url_must_be_plain_https() {
        for ok in [
            "https://example.com/cadviewer/setup.exe",
            "https://example.com",
            "https://example.com:8443/a?b=c#d",
            "https://objects.example.org/dl/CAD-prohlizec-2.0.0.zip",
        ] {
            assert_eq!(safe_download_url(ok), Some(ok), "{ok}");
        }
        for bad in [
            "", "http://example.com/x", "ftp://example.com/x", "javascript:alert(1)", "data:text/html,x", "//example.com/x",
            "https://", "https:///x", "https://user:pw@example.com/x", "https://exa mple.com/x", "https://example.com/\"onclick=x",
            "https://example.com/<script>", "https://example.com/x y", "https://example.com/\u{e1}", "https://example.com/'x",
            "https://exam_ple.com/", "HTTPS://example.com/",
        ] {
            assert_eq!(safe_download_url(bad), None, "{bad:?}");
        }
        assert_eq!(safe_download_url(&format!("https://example.com/{}", "a".repeat(2100))), None);
    }

    #[test]
    fn configured_url_follows_the_build_setting() {
        // The variable is not set in the test build, so no link is offered.
        if DOWNLOAD_URL.is_none() {
            assert_eq!(configured_download_url(), None);
        } else {
            assert_eq!(configured_download_url(), DOWNLOAD_URL.and_then(safe_download_url));
        }
    }

    // ----- static page ---------------------------------------------------------------------

    fn csp() -> String {
        let marker = "http-equiv=\"Content-Security-Policy\"";
        let tag_start = INDEX_HTML.find(marker).expect("CSP meta tag");
        let tag = &INDEX_HTML[INDEX_HTML[..tag_start].rfind('<').unwrap()..];
        let tag = &tag[..tag.find('>').unwrap()];
        let content = tag.split("content=\"").nth(1).expect("content attribute");
        content[..content.find('"').unwrap()].to_owned()
    }

    #[test]
    fn page_has_a_strict_content_security_policy() {
        let csp = csp();
        let directives: Vec<(&str, &str)> = csp
            .split(';')
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(|d| d.split_once(' ').unwrap_or((d, "")))
            .collect();
        let get = |name: &str| directives.iter().find(|(n, _)| *n == name).map(|(_, v)| *v);
        assert_eq!(get("default-src"), Some("'none'"));
        assert_eq!(get("script-src"), Some("'self' 'wasm-unsafe-eval'"));
        assert_eq!(get("style-src"), Some("'self'"));
        assert_eq!(get("connect-src"), Some("'self'"));
        assert_eq!(get("base-uri"), Some("'none'"));
        assert_eq!(get("form-action"), Some("'none'"));
        assert_eq!(get("object-src"), Some("'none'"));
        // No loophole: no inline code, no eval, no wildcards, no remote hosts, no data/blob.
        for forbidden in ["'unsafe-inline'", "'unsafe-eval'", "*", "http:", "https:", "data:", "blob:", "ws:", "wss:"] {
            assert!(!csp.contains(forbidden), "CSP contains {forbidden}: {csp}");
        }
        // frame-ancestors would be ignored in a meta tag, so it must not pretend to protect.
        assert!(get("frame-ancestors").is_none());
        // Every directive that is present uses only 'none' or 'self' sources.
        for (name, value) in &directives {
            assert!(
                value.split(' ').all(|s| matches!(s, "'none'" | "'self'" | "'wasm-unsafe-eval'")),
                "{name}: {value}"
            );
        }
    }

    #[test]
    fn page_loads_no_external_or_inline_code() {
        let html = INDEX_HTML;
        assert!(html.contains("<html lang=\"cs\">"));
        assert!(html.contains("<meta charset=\"utf-8\">"));
        assert!(html.contains("name=\"referrer\" content=\"no-referrer\""));
        // Exactly one script, a same-origin module.
        assert_eq!(html.matches("<script").count(), 1);
        assert!(html.contains("<script type=\"module\" src=\"loader.js\"></script>"));
        // No inline handlers, inline styles or javascript: URLs.
        let lower = html.to_ascii_lowercase();
        for forbidden in ["javascript:", " onclick", " onload", " onerror", " style=", "<style", "<iframe", "<object", "<embed", "<form", "<base"] {
            assert!(!lower.contains(forbidden), "page contains {forbidden}");
        }
        // No absolute address anywhere in the markup. (The download link is added by the script.)
        assert!(!lower.contains("http://") && !lower.contains("https://"), "absolute URL in index.html");
        // Only relative same-folder resources.
        for attr in ["src=\"", "href=\""] {
            for part in html.split(attr).skip(1) {
                let value = &part[..part.find('"').unwrap()];
                assert!(
                    !value.contains("://") && !value.starts_with("//") && !value.contains(".."),
                    "{attr}{value}"
                );
            }
        }
        // Czech text is present before any script runs.
        assert!(html.contains("CAD prohlížeč"));
        assert!(html.contains("<noscript>"));
    }

    #[test]
    fn page_has_every_element_the_script_uses() {
        for id in DOM_IDS {
            assert!(INDEX_HTML.contains(&format!("id=\"{id}\"")), "missing #{id}");
        }
        // The hidden parts start hidden, so a failed script never shows half a page.
        for id in [ID_ERROR, ID_HELP, ID_DOWNLOAD_LINK, ID_DOWNLOAD_MISSING] {
            let at = INDEX_HTML.find(&format!("id=\"{id}\"")).unwrap();
            let tag = &INDEX_HTML[INDEX_HTML[..at].rfind('<').unwrap()..];
            let tag = &tag[..tag.find('>').unwrap()];
            assert!(tag.split_whitespace().any(|t| t == "hidden"), "#{id} is not hidden at the start");
        }
        // Text is written with textContent only: no markup is built from input.
        let wasm_src = include_str!("lib.rs");
        let code = wasm_src.split("#[cfg(test)]").next().unwrap();
        for forbidden in ["inner_html", "set_outer_html", "insert_adjacent_html", "document.write", "eval(", "Function::new"] {
            let used = code
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .any(|l| l.contains(forbidden));
            assert!(!used, "script code uses {forbidden}");
        }
    }

    #[test]
    fn help_page_names_menu_entries_that_the_application_has() {
        // The page used to send people to "Nastavení → Zaregistrovat odkazy Google Drive", a menu that does not exist.
        // The registration, the removal and the sign-out are entries of the menu behind the arrow of "Otevřít".
        assert!(!INDEX_HTML.contains("Nastavení"), "there is no Nastavení menu in the application");
        assert!(INDEX_HTML.contains("šipka vedle tlačítka „Otevřít“ → Zaregistrovat odkazy Google Drive"));
        for entry in ["Zaregistrovat odkazy Google Drive", "Odregistrovat odkazy Google Drive", "Odhlásit se z Google Drive"] {
            assert!(INDEX_HTML.contains(entry), "{entry}");
        }
        // the help part is hidden until the script needs it, like the rest of the optional parts
        let at = INDEX_HTML.find("id=\"help\"").unwrap();
        let tag = &INDEX_HTML[INDEX_HTML[..at].rfind('<').unwrap()..];
        assert!(tag[..tag.find('>').unwrap()].split_whitespace().any(|t| t == "hidden"));
    }

    #[test]
    fn loader_and_style_are_self_contained() {
        assert!(LOADER_JS.contains("import init from \"./pkg/cad_drive_web.js\""));
        assert!(!LOADER_JS.contains("http"));
        assert!(!LOADER_JS.contains("eval("));
        assert!(!LOADER_JS.contains("innerHTML"));
        assert!(LOADER_JS.contains(text::SCRIPT_FAILED), "loader error text must match the Rust constant");
        assert!(!STYLE_CSS.contains("@import"));
        assert!(!STYLE_CSS.contains("url("));
        assert!(!STYLE_CSS.contains("http"));
    }
}
