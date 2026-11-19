//! Settings persisted as JSON in the per-user config folder (`paths::settings_file`). The keys are the ones of SPEC
//! section 14 (QSettings in the Python version), plus the `spacemouse` group of SPEC section 11.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::paths;

/// Recent files kept: the list is cut to this many entries (main.py `add_recent`: `r[:11]` plus the new one).
pub const RECENT_MAX: usize = 12;

/// A value of the settings of the 1.9 version, as the registry holds it (`REG_SZ`, `REG_DWORD`, `REG_MULTI_SZ`).
#[derive(Clone, Debug, PartialEq)]
pub enum LegacyValue {
    Text(String),
    Number(u32),
    List(Vec<String>),
}

impl LegacyValue {
    pub fn into_text(self) -> Option<String> {
        match self {
            LegacyValue::Text(s) => Some(s),
            _ => None,
        }
    }

    /// A list of strings. A single string is a list of one (Qt gives a one-element list back as a string).
    pub fn into_list(self) -> Option<Vec<String>> {
        match self {
            LegacyValue::List(l) => Some(l),
            LegacyValue::Text(s) if !s.is_empty() => Some(vec![s]),
            _ => None,
        }
    }

    /// A boolean the way Qt stores it (`"true"` / `"false"`), also `1` / `yes` and a number.
    pub fn truthy(&self, default: bool) -> bool {
        match self {
            LegacyValue::Text(s) => match s.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" => true,
                "" => default,
                _ => false,
            },
            LegacyValue::Number(n) => *n != 0,
            LegacyValue::List(_) => default,
        }
    }

    pub fn number(&self) -> Option<f64> {
        match self {
            LegacyValue::Text(s) => s.trim().parse().ok().filter(|v: &f64| v.is_finite()),
            LegacyValue::Number(n) => Some(f64::from(*n)),
            LegacyValue::List(_) => None,
        }
    }
}

/// Read-only view of the settings store of the 1.9 version (`HKCU\Software\CadProhlizec\StepViewer`). A trait so the
/// import is tested against a fake and never touches the registry of the test machine.
pub trait LegacySettings {
    /// The value of a QSettings key (`"recent"`, `"spacemouse/speed"`), `None` when it is missing or unreadable.
    fn get(&self, name: &str) -> Option<LegacyValue>;
}

/// No old settings (a platform without them, portable mode).
pub struct NoLegacy;

impl LegacySettings for NoLegacy {
    fn get(&self, _name: &str) -> Option<LegacyValue> {
        None
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpaceMouse {
    pub enabled: bool,
    pub rotate: bool,
    pub pan: bool,
    pub dominant: bool,
    /// Clamped to 0.1..=10 when loaded.
    pub speed: f64,
    /// `invert0` .. `invert5`.
    pub invert: [bool; 6],
}

impl Default for SpaceMouse {
    fn default() -> Self {
        SpaceMouse {
            enabled: true,
            rotate: true,
            pan: true,
            dominant: false,
            speed: 1.0,
            invert: [false; 6],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Window geometry, opaque layout state owned by the UI (SPEC key `geometry`).
    pub geometry: Option<String>,
    /// Splitter state, opaque layout state owned by the UI (SPEC key `split`).
    pub split: Option<String>,
    /// Recent model paths, newest first (SPEC key `recent`).
    pub recent: Vec<String>,
    /// Folder of the last opened file (SPEC key `lastdir`).
    pub lastdir: String,
    /// Scale bar in the 2D view / 3D orthographic view (SPEC key `scale_bar`, default on).
    pub scale_bar: bool,
    /// White 2D background (SPEC key `light_bg`, default off).
    pub light_bg: bool,
    /// Rulers in the 2D view (SPEC key `rulers`, default on).
    pub rulers: bool,
    /// Multisampling of the 3D view (SPEC key `msaa`, read only by the Python viewer).
    pub msaa: Option<u32>,
    pub spacemouse: SpaceMouse,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            geometry: None,
            split: None,
            recent: Vec::new(),
            lastdir: String::new(),
            scale_bar: true,
            light_bg: false,
            rulers: true,
            msaa: None,
            spacemouse: SpaceMouse::default(),
        }
    }
}

/// The value of `key` when it is there and has the type `T` wants; `None` otherwise (the caller keeps the default).
fn pick<T: DeserializeOwned>(obj: &Map<String, Value>, key: &str) -> Option<T> {
    obj.get(key).and_then(|v| serde_json::from_value(v.clone()).ok())
}

/// UTF-8 byte order mark: some editors write it at the start of the file; it is not part of the JSON.
const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// Most damaged copies kept as `<file>.bad`, `<file>.bad.1`, ... (more than that: nothing more is kept).
const DAMAGED_KEPT_MAX: u32 = 20;

/// The JSON document of a settings file. A UTF-8 byte order mark is ignored. `None` when the bytes are not one
/// document of JSON text.
fn parse_document(bytes: &[u8]) -> Option<Value> {
    let bytes = bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).ok()?;
    serde_json::from_str::<Value>(text).ok()
}

/// Keeps the bytes of a damaged settings file as `<file>.bad`. An existing `.bad` file is never overwritten: the
/// damaged file goes to the next free `.bad.<n>` name, and nothing is written when a kept copy already has these
/// bytes (a file that stays damaged until the next save is not copied again on every start).
fn keep_damaged(path: &Path, bytes: &[u8]) {
    for n in 0..=DAMAGED_KEPT_MAX {
        let name = if n == 0 { format!("{}.bad", path.display()) } else { format!("{}.bad.{n}", path.display()) };
        let target = PathBuf::from(name);
        match fs::OpenOptions::new().write(true).create_new(true).open(&target) {
            Ok(mut f) => {
                use std::io::Write;
                let _ = f.write_all(bytes);
                return;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                if fs::read(&target).is_ok_and(|old| old == bytes) {
                    return;
                }
            }
            // no folder, no rights: nothing can be kept, and nothing that exists is touched
            Err(_) => return,
        }
    }
}

/// The temp file of one save: `<file>.<pid>.<seq>.tmp`, so two processes or two attempts never share it.
fn temp_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.{}.{}.tmp", path.display(), std::process::id(), paths::temp_seq()))
}

impl SpaceMouse {
    /// Every key on its own: a value of the wrong type resets that key to its default and nothing else.
    fn from_value(v: Option<&Value>) -> SpaceMouse {
        let mut s = SpaceMouse::default();
        let Some(o) = v.and_then(Value::as_object) else { return s };
        s.enabled = pick(o, "enabled").unwrap_or(s.enabled);
        s.rotate = pick(o, "rotate").unwrap_or(s.rotate);
        s.pan = pick(o, "pan").unwrap_or(s.pan);
        s.dominant = pick(o, "dominant").unwrap_or(s.dominant);
        s.speed = pick(o, "speed").unwrap_or(s.speed);
        s.invert = pick(o, "invert").unwrap_or(s.invert);
        s
    }
}

impl Settings {
    /// The settings of a parsed JSON text. Every key is taken on its own: one value of the wrong type (a hand edit,
    /// or a file of another version) resets that key to its default and leaves all the others as they are. A
    /// document that is not an object gives the defaults.
    pub fn from_json(v: &Value) -> Settings {
        let mut s = Settings::default();
        let Some(o) = v.as_object() else { return s };
        s.geometry = pick(o, "geometry").unwrap_or(s.geometry);
        s.split = pick(o, "split").unwrap_or(s.split);
        s.recent = pick(o, "recent").unwrap_or(s.recent);
        s.lastdir = pick(o, "lastdir").unwrap_or(s.lastdir);
        s.scale_bar = pick(o, "scale_bar").unwrap_or(s.scale_bar);
        s.light_bg = pick(o, "light_bg").unwrap_or(s.light_bg);
        s.rulers = pick(o, "rulers").unwrap_or(s.rulers);
        s.msaa = pick(o, "msaa").unwrap_or(s.msaa);
        s.spacemouse = SpaceMouse::from_value(o.get("spacemouse"));
        s.sanitize();
        s
    }

    /// Reads the settings file. A missing file gives the defaults. A file that is not one JSON object (empty,
    /// truncated, a trailing comma, another text encoding, or valid JSON such as `[1, 2]` or `null`) is kept as
    /// `<file>.bad` before the defaults are used: the next save writes the settings file again, and the damaged
    /// original must not be lost with it. A value of the wrong type resets only its own key.
    pub fn load_from(path: &Path) -> Settings {
        let Ok(bytes) = fs::read(path) else { return Settings::default() };
        match parse_document(&bytes) {
            Some(v) if v.is_object() => Settings::from_json(&v),
            _ => {
                keep_damaged(path, &bytes);
                Settings::default()
            }
        }
    }

    /// The settings file when there is one. When there is none, the settings of the 1.9 version are imported once
    /// from `legacy` and written to the new file; nothing is written when there is nothing to import. The old store
    /// is only read.
    pub fn load_or_import(path: &Path, legacy: &dyn LegacySettings) -> Settings {
        if path.exists() {
            return Settings::load_from(path);
        }
        match Settings::from_legacy(legacy) {
            Some(s) => {
                let _ = s.save_to(path);
                s
            }
            None => Settings::default(),
        }
    }

    /// The 1.9 settings (`QSettings("CadProhlizec", "StepViewer")`): `recent`, `lastdir`, `scale_bar`, `light_bg`,
    /// `rulers` and the `spacemouse/*` keys. `None` when the store has none of them. The window geometry and the
    /// splitter state are Qt binary blobs and are not taken over.
    pub fn from_legacy(legacy: &dyn LegacySettings) -> Option<Settings> {
        let mut s = Settings::default();
        let mut found = false;
        let mut take = |name: &str| {
            let v = legacy.get(name);
            found |= v.is_some();
            v
        };
        if let Some(list) = take("recent").and_then(|v| v.into_list()) {
            s.recent = list.into_iter().filter(|p| !p.is_empty()).take(RECENT_MAX).collect();
        }
        if let Some(d) = take("lastdir").and_then(|v| v.into_text()) {
            s.lastdir = d;
        }
        // main.py: `self.settings.value(key, default) in (True, "true")`
        if let Some(b) = take("scale_bar").map(|v| v.truthy(false)) {
            s.scale_bar = b;
        }
        if let Some(b) = take("light_bg").map(|v| v.truthy(false)) {
            s.light_bg = b;
        }
        if let Some(b) = take("rulers").map(|v| v.truthy(false)) {
            s.rulers = b;
        }
        // spacemouse.py `load`: booleans from "1", "true" or "yes", the speed a float within 0.1..=10
        let sm = &mut s.spacemouse;
        for (key, slot) in [("enabled", &mut sm.enabled), ("rotate", &mut sm.rotate), ("pan", &mut sm.pan), ("dominant", &mut sm.dominant)] {
            if let Some(v) = take(&format!("spacemouse/{key}")) {
                *slot = v.truthy(*slot);
            }
        }
        for (i, slot) in sm.invert.iter_mut().enumerate() {
            if let Some(v) = take(&format!("spacemouse/invert{i}")) {
                *slot = v.truthy(*slot);
            }
        }
        if let Some(v) = take("spacemouse/speed") {
            sm.speed = v.number().unwrap_or(1.0);
        }
        s.sanitize();
        found.then_some(s)
    }

    /// Writes the settings file atomically (temp file, then rename). The temp file name is unique to this attempt.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        let tmp = temp_path(path);
        let result = fs::write(&tmp, text).and_then(|_| fs::rename(&tmp, path));
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }

    /// `load_from` on the per-user settings file.
    pub fn load() -> Settings {
        Self::load_from(&paths::settings_file())
    }

    /// `save_to` on the per-user settings file.
    pub fn save(&self) -> io::Result<()> {
        self.save_to(&paths::settings_file())
    }

    /// Keeps values in their documented ranges (speed 0.1..=10, NaN gives 1.0).
    fn sanitize(&mut self) {
        let sp = &mut self.spacemouse;
        sp.speed = if sp.speed.is_nan() {
            1.0
        } else {
            sp.speed.clamp(0.1, 10.0)
        };
    }

    /// Recent entries whose files still exist (`MainWindow.recent`).
    pub fn existing_recent(&self) -> Vec<String> {
        self.recent
            .iter()
            .filter(|p| Path::new(p).exists())
            .cloned()
            .collect()
    }

    /// Moves `path` to the front of the recent list and sets `lastdir` (`MainWindow.add_recent`). The list is built
    /// from the existing entries; an earlier entry for the same file (compared case-insensitively on Windows) is
    /// dropped, and the list keeps at most `RECENT_MAX` entries.
    pub fn add_recent(&mut self, path: &str) {
        let key = paths::normcase_str(path);
        let mut rest: Vec<String> = self
            .existing_recent()
            .into_iter()
            .filter(|p| paths::normcase_str(p) != key)
            .collect();
        rest.truncate(RECENT_MAX - 1);
        let mut list = Vec::with_capacity(RECENT_MAX);
        list.push(path.to_string());
        list.extend(rest);
        self.recent = list;
        self.lastdir = Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec_section_14() {
        let s = Settings::default();
        assert!(s.scale_bar);
        assert!(!s.light_bg);
        assert!(s.rulers);
        assert!(s.recent.is_empty());
        assert_eq!(s.lastdir, "");
        assert!(s.spacemouse.enabled && s.spacemouse.rotate && s.spacemouse.pan);
        assert!(!s.spacemouse.dominant);
        assert_eq!(s.spacemouse.speed, 1.0);
        assert_eq!(s.spacemouse.invert, [false; 6]);
    }

    #[test]
    fn round_trip_and_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg").join("settings.json");
        assert_eq!(Settings::load_from(&path), Settings::default());
        let mut s = Settings::default();
        s.light_bg = true;
        s.spacemouse.invert[3] = true;
        s.spacemouse.speed = 2.5;
        s.msaa = Some(4);
        s.save_to(&path).unwrap();
        assert!(!PathBuf::from(format!("{}.tmp", path.display())).exists());
        assert_eq!(Settings::load_from(&path), s);
    }

    #[test]
    fn json_uses_documented_key_names() {
        let v: serde_json::Value = serde_json::to_value(Settings::default()).unwrap();
        for key in ["recent", "lastdir", "scale_bar", "light_bg", "rulers", "spacemouse"] {
            assert!(v.get(key).is_some(), "missing key {key}");
        }
        let sp = &v["spacemouse"];
        for key in ["enabled", "rotate", "pan", "dominant", "speed", "invert"] {
            assert!(sp.get(key).is_some(), "missing spacemouse key {key}");
        }
    }

    #[test]
    fn damaged_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, "{ not json").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
    }

    #[test]
    fn a_byte_order_mark_is_not_a_damaged_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut bytes = UTF8_BOM.to_vec();
        bytes.extend_from_slice(br#"{"rulers": false, "lastdir": "C:\\modely"}"#);
        fs::write(&path, &bytes).unwrap();
        let s = Settings::load_from(&path);
        assert!(!s.rulers);
        assert_eq!(s.lastdir, r"C:\modely");
        assert!(!dir.path().join("settings.json.bad").exists());
    }

    /// Damaged texts a hand edit or an interrupted save leaves behind.
    fn damaged_texts() -> Vec<Vec<u8>> {
        let mut bom_only = UTF8_BOM.to_vec();
        bom_only.extend_from_slice(b"{\"rulers\": fal");
        vec![
            b"".to_vec(),
            b"   \n".to_vec(),
            br#"{"rulers": false,}"#.to_vec(),
            br#"{"rulers": fal"#.to_vec(),
            bom_only,
            // UTF-16 with its byte order mark, as Notepad "Unicode" writes it
            vec![0xFF, 0xFE, b'{', 0, b'}', 0],
        ]
    }

    #[test]
    fn a_damaged_file_is_kept_as_bad_before_the_defaults_are_used() {
        for (i, text) in damaged_texts().into_iter().enumerate() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.json");
            fs::write(&path, &text).unwrap();
            assert_eq!(Settings::load_from(&path), Settings::default(), "case {i}");
            // the original is there, byte for byte, as the .bad copy; the settings file itself is left for the next save
            assert_eq!(fs::read(dir.path().join("settings.json.bad")).unwrap(), text, "case {i}");
            assert_eq!(fs::read(&path).unwrap(), text, "case {i}");
        }
    }

    #[test]
    fn an_existing_bad_file_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let bad = dir.path().join("settings.json.bad");
        fs::write(&bad, b"earlier damage").unwrap();
        fs::write(&path, b"{ broken").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
        assert_eq!(fs::read(&bad).unwrap(), b"earlier damage");
        assert_eq!(fs::read(dir.path().join("settings.json.bad.1")).unwrap(), b"{ broken");
        // the same damage again is already kept: no third copy
        assert_eq!(Settings::load_from(&path), Settings::default());
        assert!(!dir.path().join("settings.json.bad.2").exists());
    }

    #[test]
    fn a_valid_file_keeps_no_bad_copy_and_a_save_replaces_the_damaged_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, b"[1, 2").unwrap();
        let s = Settings::load_from(&path);
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), s);
        assert_eq!(fs::read(dir.path().join("settings.json.bad")).unwrap(), b"[1, 2");
        // a valid file is never copied
        let other = dir.path().join("ok.json");
        fs::write(&other, br#"{"rulers": false}"#).unwrap();
        Settings::load_from(&other);
        assert!(!dir.path().join("ok.json.bad").exists());
    }

    #[test]
    fn temp_files_of_two_saves_never_share_a_name() {
        let path = Path::new("cfg").join("settings.json");
        let (a, b) = (temp_path(&path), temp_path(&path));
        assert_ne!(a, b);
        assert_ne!(a, PathBuf::from(format!("{}.tmp", path.display())));
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        Settings::default().save_to(&file).unwrap();
        let mut s = Settings::default();
        s.rulers = false;
        s.save_to(&file).unwrap();
        assert_eq!(Settings::load_from(&file), s);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1, "no temp file is left behind");
    }

    #[test]
    fn partial_file_fills_missing_keys_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, r#"{"rulers": false, "spacemouse": {"speed": 50}}"#).unwrap();
        let s = Settings::load_from(&path);
        assert!(!s.rulers);
        assert!(s.scale_bar);
        assert!(s.spacemouse.enabled);
        assert_eq!(s.spacemouse.speed, 10.0, "speed is clamped to 10");
    }

    #[test]
    fn one_wrong_typed_value_resets_only_its_own_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(
            &path,
            r#"{"recent": ["a.step", "b.step"], "lastdir": 5, "scale_bar": "yes", "light_bg": true, "rulers": false,
                "msaa": "four", "geometry": 7, "split": "340",
                "spacemouse": {"enabled": false, "speed": "fast", "pan": 0, "invert": [true, false, true, false, true, false], "dominant": true}}"#,
        )
        .unwrap();
        let s = Settings::load_from(&path);
        // the wrong ones are back at their defaults ...
        assert_eq!(s.lastdir, "");
        assert!(s.scale_bar);
        assert_eq!(s.msaa, None);
        assert_eq!(s.geometry, None);
        assert_eq!(s.spacemouse.speed, 1.0);
        assert!(s.spacemouse.pan);
        // ... and everything else is kept
        assert_eq!(s.recent, vec!["a.step".to_string(), "b.step".to_string()]);
        assert!(s.light_bg);
        assert!(!s.rulers);
        assert_eq!(s.split.as_deref(), Some("340"));
        assert!(!s.spacemouse.enabled);
        assert!(s.spacemouse.dominant);
        assert_eq!(s.spacemouse.invert, [true, false, true, false, true, false]);
        // a wrong list resets the whole list key only
        fs::write(&path, r#"{"recent": ["a.step", 3], "rulers": false}"#).unwrap();
        let s = Settings::load_from(&path);
        assert!(s.recent.is_empty());
        assert!(!s.rulers);
        // a group of the wrong type resets the group only
        fs::write(&path, r#"{"spacemouse": 5, "light_bg": true}"#).unwrap();
        let s = Settings::load_from(&path);
        assert_eq!(s.spacemouse, SpaceMouse::default());
        assert!(s.light_bg);
        // a document that is not an object gives the defaults (and is kept as .bad, see below)
        fs::write(&path, "[1, 2]").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
    }

    #[test]
    fn valid_json_that_is_not_an_object_is_kept_as_bad_before_the_defaults() {
        // JSON texts that parse but are not a settings object: the same treatment as an unparsable file
        let texts: [&[u8]; 6] = [b"[1, 2]", b"null", b"42", br#""rulers""#, b"true", b"[]"];
        for (i, text) in texts.into_iter().enumerate() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.json");
            fs::write(&path, text).unwrap();
            assert_eq!(Settings::load_from(&path), Settings::default(), "case {i}");
            assert_eq!(fs::read(dir.path().join("settings.json.bad")).unwrap(), text, "case {i}");
            // the original stays until the next save
            assert_eq!(fs::read(&path).unwrap(), text, "case {i}");
        }
    }

    #[test]
    fn a_non_object_with_a_byte_order_mark_is_kept_as_bad() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut bytes = UTF8_BOM.to_vec();
        bytes.extend_from_slice(b"[1, 2]");
        fs::write(&path, &bytes).unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
        assert_eq!(fs::read(dir.path().join("settings.json.bad")).unwrap(), bytes);
    }

    #[test]
    fn a_non_object_keeps_the_numbered_bad_copies_too() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(dir.path().join("settings.json.bad"), b"earlier damage").unwrap();
        fs::write(&path, b"null").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
        assert_eq!(fs::read(dir.path().join("settings.json.bad")).unwrap(), b"earlier damage");
        assert_eq!(fs::read(dir.path().join("settings.json.bad.1")).unwrap(), b"null");
        // a save then replaces the non-object file; the next start keeps nothing new
        Settings::default().save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
        assert!(!dir.path().join("settings.json.bad.2").exists());
    }

    #[test]
    fn a_valid_object_is_never_kept_as_bad() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, b"{}").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
        assert!(!dir.path().join("settings.json.bad").exists());
    }

    #[test]
    fn speed_is_clamped_low_and_nan_becomes_one() {
        let mut s = Settings::default();
        s.spacemouse.speed = 0.01;
        s.sanitize();
        assert_eq!(s.spacemouse.speed, 0.1);
        s.spacemouse.speed = f64::NAN;
        s.sanitize();
        assert_eq!(s.spacemouse.speed, 1.0);
    }

    /// A fake of the registry key of the 1.9 version.
    struct Fake(std::collections::HashMap<&'static str, LegacyValue>);

    impl LegacySettings for Fake {
        fn get(&self, name: &str) -> Option<LegacyValue> {
            self.0.get(name).cloned()
        }
    }

    fn text(s: &str) -> LegacyValue {
        LegacyValue::Text(s.to_string())
    }

    #[test]
    fn the_19_settings_are_imported_once_when_there_is_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg").join("settings.json");
        let fake = Fake(
            [
                ("recent", LegacyValue::List(vec![r"C:\modely\a.step".into(), "".into(), r"C:\modely\b.stl".into()])),
                ("lastdir", text(r"C:\modely")),
                ("scale_bar", text("false")),
                ("light_bg", text("true")),
                ("rulers", text("false")),
                ("spacemouse/enabled", text("false")),
                ("spacemouse/dominant", text("true")),
                ("spacemouse/speed", text("2.5")),
                ("spacemouse/invert2", text("true")),
                ("spacemouse/invert3", LegacyValue::Number(1)),
                // a Qt binary blob is never imported
                ("geometry", text("@ByteArray(...)")),
            ]
            .into_iter()
            .collect(),
        );
        let s = Settings::load_or_import(&path, &fake);
        assert_eq!(s.recent, vec![r"C:\modely\a.step".to_string(), r"C:\modely\b.stl".to_string()]);
        assert_eq!(s.lastdir, r"C:\modely");
        assert!(!s.scale_bar && s.light_bg && !s.rulers);
        assert!(!s.spacemouse.enabled && s.spacemouse.dominant);
        assert!(s.spacemouse.rotate && s.spacemouse.pan, "keys that are missing keep their defaults");
        assert_eq!(s.spacemouse.speed, 2.5);
        assert_eq!(s.spacemouse.invert, [false, false, true, true, false, false]);
        assert_eq!(s.geometry, None);
        // written to the new file, so the next start reads that and never the old store again
        assert_eq!(Settings::load_from(&path), s);
        struct Forbidden;
        impl LegacySettings for Forbidden {
            fn get(&self, name: &str) -> Option<LegacyValue> {
                panic!("the old store was read again for {name}");
            }
        }
        assert_eq!(Settings::load_or_import(&path, &Forbidden), s);
    }

    #[test]
    fn nothing_to_import_writes_no_file_and_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(Settings::load_or_import(&path, &NoLegacy), Settings::default());
        assert!(!path.exists());
        assert!(Settings::from_legacy(&Fake(Default::default())).is_none());
    }

    #[test]
    fn an_existing_file_is_never_overwritten_by_an_import() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, r#"{"rulers": false}"#).unwrap();
        let fake = Fake([("rulers", text("true")), ("lastdir", text("x"))].into_iter().collect());
        let s = Settings::load_or_import(&path, &fake);
        assert!(!s.rulers);
        assert_eq!(s.lastdir, "");
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"rulers": false}"#);
    }

    #[test]
    fn legacy_values_follow_the_python_readers() {
        // main.py: `value(key, default) in (True, "true")`; spacemouse.py: "1", "true", "yes", else False
        assert!(text("true").truthy(false) && text("TRUE").truthy(false) && text("yes").truthy(false) && text("1").truthy(false));
        assert!(!text("false").truthy(true) && !text("0").truthy(true) && !text("nope").truthy(true));
        assert!(text("").truthy(true) && !text("").truthy(false), "an empty text keeps the default");
        assert!(LegacyValue::Number(2).truthy(false) && !LegacyValue::Number(0).truthy(true));
        // speed: a number, else 1.0, then clamped to 0.1..=10
        let f = |v: LegacyValue| Settings::from_legacy(&Fake([("spacemouse/speed", v)].into_iter().collect())).unwrap().spacemouse.speed;
        assert_eq!(f(text("50")), 10.0);
        assert_eq!(f(text("0.001")), 0.1);
        assert_eq!(f(text("rychle")), 1.0);
        assert_eq!(f(text("nan")), 1.0);
        assert_eq!(f(LegacyValue::Number(3)), 3.0);
        // one path stored as a string is a list of one
        let s = Settings::from_legacy(&Fake([("recent", text(r"C:\a.step"))].into_iter().collect())).unwrap();
        assert_eq!(s.recent, vec![r"C:\a.step".to_string()]);
    }

    #[test]
    fn an_imported_recent_list_is_cut_to_the_maximum() {
        let many: Vec<String> = (0..20).map(|i| format!(r"C:\m{i}.step")).collect();
        let s = Settings::from_legacy(&Fake([("recent", LegacyValue::List(many))].into_iter().collect())).unwrap();
        assert_eq!(s.recent.len(), RECENT_MAX);
        assert_eq!(s.recent[0], r"C:\m0.step");
    }

    #[test]
    fn add_recent_moves_to_front_dedupes_and_caps_at_12() {
        let dir = tempfile::tempdir().unwrap();
        let mut files = Vec::new();
        for i in 0..14 {
            let p = dir.path().join(format!("m{i}.step"));
            fs::write(&p, b"x").unwrap();
            files.push(p.to_string_lossy().into_owned());
        }
        let mut s = Settings::default();
        for f in &files {
            s.add_recent(f);
        }
        assert_eq!(s.recent.len(), RECENT_MAX);
        assert_eq!(s.recent[0], files[13]);
        assert_eq!(s.recent[11], files[2]);
        // re-opening an entry moves it to the front without duplicating it
        s.add_recent(&files[5]);
        assert_eq!(s.recent.len(), RECENT_MAX);
        assert_eq!(s.recent[0], files[5]);
        assert_eq!(s.recent.iter().filter(|p| **p == files[5]).count(), 1);
        assert_eq!(s.lastdir, dir.path().to_string_lossy());
    }

    #[test]
    fn add_recent_drops_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let keep = dir.path().join("keep.step");
        fs::write(&keep, b"x").unwrap();
        let mut s = Settings::default();
        s.recent = vec![
            dir.path().join("gone.step").to_string_lossy().into_owned(),
            keep.to_string_lossy().into_owned(),
        ];
        s.add_recent(&dir.path().join("new.step").to_string_lossy());
        assert_eq!(s.recent.len(), 2);
        assert!(s.recent[1].ends_with("keep.step"));
    }
}
