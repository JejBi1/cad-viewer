//! Registration of the `cadviewer://` link handler, on explicit user action only.
//!
//! * Windows: values under `HKEY_CURRENT_USER\Software\Classes\cadviewer` and nothing else.
//!   The command is `"<exe>" --open-link "%1"`.
//! * Linux: a desktop entry in `$XDG_DATA_HOME/applications` and the default handler in
//!   `$XDG_CONFIG_HOME/mimeapps.list`, both in the user's own folders.
//!
//! The registry access sits behind [`RegistryBackend`] and the folders behind [`XdgDirs`], so the
//! tests run against a fake registry and a temporary folder and never touch the real ones.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::DriveError;

/// Key below HKCU that holds everything this crate writes.
pub const REGISTRY_ROOT: &str = r"Software\Classes\cadviewer";
/// Default value of the scheme key.
const REG_DESCRIPTION: &str = "URL:CAD prohlížeč";
/// Argument of the registered command.
pub const OPEN_LINK_ARG: &str = "--open-link";
/// File name of the Linux desktop entry.
pub const DESKTOP_FILE_NAME: &str = "cadviewer-link.desktop";
/// Marker line inside the desktop entry, so only our own file is ever removed.
const DESKTOP_MARKER: &str = "X-CadViewer-Link=1";
const MIME_KEY: &str = "x-scheme-handler/cadviewer";

fn reg_err(msg: impl Into<String>) -> DriveError {
    DriveError::Registration(msg.into())
}

/// Characters that never belong in the executable path put into a command line.
fn check_exe_text(exe: &str) -> Result<(), DriveError> {
    if exe.is_empty() || exe.chars().any(|c| c.is_control()) || exe.contains('"') || exe.contains('%') {
        return Err(reg_err("cesta ke spustitelnému souboru obsahuje nepovolené znaky"));
    }
    Ok(())
}

fn exe_str(exe: &Path) -> Result<&str, DriveError> {
    exe.to_str()
        .ok_or_else(|| reg_err("cesta ke spustitelnému souboru není platný text"))
}

/// The command stored in the registry: `"<exe>" --open-link "%1"`.
pub fn windows_command(exe: &Path) -> Result<String, DriveError> {
    let exe = exe_str(exe)?;
    check_exe_text(exe)?;
    Ok(format!("\"{exe}\" {OPEN_LINK_ARG} \"%1\""))
}

/// Minimal registry operations, all relative to HKEY_CURRENT_USER.
pub trait RegistryBackend {
    /// Creates the key if needed and sets a string value. `name` is empty for the default value.
    fn set_string(&mut self, key: &str, name: &str, data: &str) -> Result<(), DriveError>;
    /// Reads a string value; `None` if the key or the value does not exist.
    fn get_string(&self, key: &str, name: &str) -> Result<Option<String>, DriveError>;
    /// Deletes the key and everything below it. A missing key is not an error.
    fn delete_tree(&mut self, key: &str) -> Result<(), DriveError>;
}

/// True if `key` is [`REGISTRY_ROOT`] or lies below it (case-insensitive, as the registry is).
/// Empty segments and `.` or `..` segments are refused.
pub fn is_under_root(key: &str) -> bool {
    let key = key.trim_matches('\\');
    let root = REGISTRY_ROOT;
    let prefix_ok = key.len() >= root.len()
        && key.is_char_boundary(root.len())
        && key[..root.len()].eq_ignore_ascii_case(root)
        && (key.len() == root.len() || key.as_bytes()[root.len()] == b'\\');
    prefix_ok && key.split('\\').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// Writes the scheme registration through `backend`.
pub fn register_link_with(backend: &mut dyn RegistryBackend, exe: &Path) -> Result<(), DriveError> {
    let command = windows_command(exe)?;
    let icon = format!("\"{}\",0", exe_str(exe)?);
    backend.set_string(REGISTRY_ROOT, "", REG_DESCRIPTION)?;
    backend.set_string(REGISTRY_ROOT, "URL Protocol", "")?;
    backend.set_string(&format!(r"{REGISTRY_ROOT}\DefaultIcon"), "", &icon)?;
    backend.set_string(&format!(r"{REGISTRY_ROOT}\shell\open\command"), "", &command)?;
    Ok(())
}

/// Removes the registration, but only if it was made for `exe` (the command is exactly [`windows_command`] of `exe`).
/// A registration of another copy of the program (installed or portable) or of another program is left alone and
/// reported.
pub fn unregister_link_with(backend: &mut dyn RegistryBackend, exe: &Path) -> Result<(), DriveError> {
    let command_key = format!(r"{REGISTRY_ROOT}\shell\open\command");
    match backend.get_string(&command_key, "")? {
        Some(command) if command.eq_ignore_ascii_case(&windows_command(exe)?) => backend.delete_tree(REGISTRY_ROOT),
        Some(command) if command.contains(OPEN_LINK_ARG) => {
            Err(reg_err("odkazy cadviewer:// jsou zaregistrované pro jinou kopii CAD prohlížeče, registrace nebyla odstraněna"))
        }
        Some(_) => Err(reg_err("klíč cadviewer v registru nepatří CAD prohlížeči, nebyl odstraněn")),
        // A registration that was interrupted before the command was written: remove it if the
        // description is ours, otherwise the key is not ours and stays.
        None => match backend.get_string(REGISTRY_ROOT, "")? {
            Some(description) if description == REG_DESCRIPTION => backend.delete_tree(REGISTRY_ROOT),
            _ => Ok(()),
        },
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};

    /// The real registry, restricted to the cadviewer key.
    pub struct HkcuRegistry;

    fn guard(key: &str) -> Result<(), DriveError> {
        if is_under_root(key) { Ok(()) } else { Err(reg_err("zápis mimo klíč cadviewer není povolen")) }
    }

    impl RegistryBackend for HkcuRegistry {
        fn set_string(&mut self, key: &str, name: &str, data: &str) -> Result<(), DriveError> {
            guard(key)?;
            let hkcu = RegKey::predef(HKEY_CURRENT_USER);
            let (k, _) = hkcu.create_subkey(key).map_err(|e| reg_err(e.to_string()))?;
            k.set_value(name, &data).map_err(|e| reg_err(e.to_string()))
        }

        fn get_string(&self, key: &str, name: &str) -> Result<Option<String>, DriveError> {
            guard(key)?;
            let hkcu = RegKey::predef(HKEY_CURRENT_USER);
            let k = match hkcu.open_subkey_with_flags(key, KEY_READ) {
                Ok(k) => k,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(reg_err(e.to_string())),
            };
            match k.get_value::<String, _>(name) {
                Ok(v) => Ok(Some(v)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(reg_err(e.to_string())),
            }
        }

        fn delete_tree(&mut self, key: &str) -> Result<(), DriveError> {
            guard(key)?;
            let hkcu = RegKey::predef(HKEY_CURRENT_USER);
            match hkcu.delete_subkey_all(key) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(reg_err(e.to_string())),
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Linux (XDG). The functions work on any OS; only the entry points below choose by platform.
// ---------------------------------------------------------------------------------------------

/// The user's XDG folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XdgDirs {
    /// `$XDG_DATA_HOME`, default `~/.local/share`.
    pub data_home: PathBuf,
    /// `$XDG_CONFIG_HOME`, default `~/.config`.
    pub config_home: PathBuf,
}

impl XdgDirs {
    pub fn from_env() -> Result<Self, DriveError> {
        let home = std::env::var_os("HOME").map(PathBuf::from).filter(|p| p.is_absolute());
        let pick = |var: &str, fallback: &str| -> Result<PathBuf, DriveError> {
            match std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()) {
                Some(p) => Ok(p),
                None => home
                    .as_ref()
                    .map(|h| h.join(fallback))
                    .ok_or_else(|| reg_err("nelze určit domovskou složku uživatele")),
            }
        };
        Ok(XdgDirs {
            data_home: pick("XDG_DATA_HOME", ".local/share")?,
            config_home: pick("XDG_CONFIG_HOME", ".config")?,
        })
    }

    pub fn desktop_file(&self) -> PathBuf {
        self.data_home.join("applications").join(DESKTOP_FILE_NAME)
    }

    pub fn mimeapps_list(&self) -> PathBuf {
        self.config_home.join("mimeapps.list")
    }
}

/// Quotes one argument for the `Exec` key of a desktop entry (Desktop Entry Specification).
/// `%` is doubled, `"`, `` ` ``, `$` and `\` are escaped, and the backslash is escaped once
/// more because of the string escaping rule, which makes `\` four characters long.
pub fn desktop_exec_quote(arg: &str) -> String {
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    for c in arg.chars() {
        match c {
            '\\' => out.push_str("\\\\\\\\"),
            '"' => out.push_str("\\\""),
            '`' => out.push_str("\\`"),
            '$' => out.push_str("\\$"),
            '%' => out.push_str("%%"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Content of the desktop entry for `exe`.
pub fn desktop_entry(exe: &Path) -> Result<String, DriveError> {
    let exe = exe_str(exe)?;
    if exe.is_empty() || exe.chars().any(|c| c.is_control()) {
        return Err(reg_err("cesta ke spustitelnému souboru obsahuje nepovolené znaky"));
    }
    Ok(format!(
        "[Desktop Entry]\n\
Type=Application\n\
Name=CAD prohlížeč (odkazy Google Drive)\n\
Comment=Otevře soubor z Google Drive v CAD prohlížeči\n\
Exec={} {OPEN_LINK_ARG} %u\n\
Terminal=false\n\
NoDisplay=true\n\
MimeType={MIME_KEY};\n\
{DESKTOP_MARKER}\n",
        desktop_exec_quote(exe)
    ))
}

fn write_atomic(path: &Path, content: &str) -> Result<(), DriveError> {
    let dir = path.parent().ok_or_else(|| reg_err("neplatná cesta"))?;
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("cadviewer")
    ));
    fs::write(&tmp, content)?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Sets `x-scheme-handler/cadviewer=<desktop id>;` in the `[Default Applications]` section of
/// a `mimeapps.list` text, keeping everything else.
pub fn mimeapps_set_default(text: &str) -> String {
    let entry = format!("{MIME_KEY}={DESKTOP_FILE_NAME};");
    let mut out: Vec<String> = Vec::new();
    let mut in_defaults = false;
    let mut section_seen = false;
    let mut done = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_defaults && !done {
                out.push(entry.clone());
                done = true;
            }
            in_defaults = trimmed == "[Default Applications]";
            section_seen |= in_defaults;
            out.push(line.to_owned());
            continue;
        }
        if in_defaults && trimmed.split('=').next().map(str::trim) == Some(MIME_KEY) {
            if !done {
                out.push(entry.clone());
                done = true;
            }
            continue;
        }
        out.push(line.to_owned());
    }
    if !done {
        if in_defaults {
            out.push(entry);
        } else if !section_seen {
            if out.last().is_some_and(|l| !l.is_empty()) {
                out.push(String::new());
            }
            out.push("[Default Applications]".to_owned());
            out.push(entry);
        }
    }
    let mut result = out.join("\n");
    result.push('\n');
    result
}

/// Removes our desktop id from the `x-scheme-handler/cadviewer` line of a `mimeapps.list` text.
/// Other handlers on the same line are kept.
pub fn mimeapps_remove_default(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_defaults = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_defaults = trimmed == "[Default Applications]";
            out.push(line.to_owned());
            continue;
        }
        if in_defaults
            && let Some((key, value)) = trimmed.split_once('=')
            && key.trim() == MIME_KEY
        {
            let kept: Vec<&str> = value
                .split(';')
                .map(str::trim)
                .filter(|v| !v.is_empty() && *v != DESKTOP_FILE_NAME)
                .collect();
            if !kept.is_empty() {
                out.push(format!("{MIME_KEY}={};", kept.join(";")));
            }
            continue;
        }
        out.push(line.to_owned());
    }
    let mut result = out.join("\n");
    if !result.is_empty() {
        result.push('\n');
    }
    result
}

/// Writes the desktop entry and the default handler below `dirs`.
pub fn register_link_xdg(dirs: &XdgDirs, exe: &Path) -> Result<(), DriveError> {
    let desktop = desktop_entry(exe)?;
    write_atomic(&dirs.desktop_file(), &desktop)?;
    let list_path = dirs.mimeapps_list();
    let current = match fs::read_to_string(&list_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    write_atomic(&list_path, &mimeapps_set_default(&current))
}

/// Removes what [`register_link_xdg`] wrote. A desktop file without our marker is left alone.
pub fn unregister_link_xdg(dirs: &XdgDirs) -> Result<(), DriveError> {
    let desktop_path = dirs.desktop_file();
    match fs::read_to_string(&desktop_path) {
        Ok(text) if text.lines().any(|l| l.trim() == DESKTOP_MARKER) => fs::remove_file(&desktop_path)?,
        Ok(_) => return Err(reg_err("soubor cadviewer-link.desktop nepatří CAD prohlížeči, nebyl odstraněn")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let list_path = dirs.mimeapps_list();
    match fs::read_to_string(&list_path) {
        Ok(text) => write_atomic(&list_path, &mimeapps_remove_default(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

// ---------------------------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------------------------

fn check_exe_for_registration(exe: &Path) -> Result<(), DriveError> {
    if !exe.is_absolute() {
        return Err(reg_err("cesta ke spustitelnému souboru musí být úplná"));
    }
    if !exe.is_file() {
        return Err(reg_err("spustitelný soubor neexistuje"));
    }
    Ok(())
}

/// Registers `cadviewer://` links for the current user, so a link opens `exe --open-link <url>`.
/// Call it only because the user asked for it.
pub fn register_link(exe: &Path) -> Result<(), DriveError> {
    check_exe_for_registration(exe)?;
    #[cfg(windows)]
    {
        register_link_with(&mut win::HkcuRegistry, exe)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        register_link_xdg(&XdgDirs::from_env()?, exe)
    }
    #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
    {
        Err(DriveError::Unsupported)
    }
}

/// Removes the registration made by [`register_link`] for the running executable.
pub fn unregister_link() -> Result<(), DriveError> {
    #[cfg(windows)]
    {
        let exe = std::env::current_exe().map_err(|_| reg_err("cestu ke spustitelnému souboru nelze zjistit"))?;
        unregister_link_with(&mut win::HkcuRegistry, &exe)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        unregister_link_xdg(&XdgDirs::from_env()?)
    }
    #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
    {
        Err(DriveError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Fake registry: (lower-case key, value name) -> data. Records every write.
    #[derive(Default)]
    struct FakeRegistry {
        values: BTreeMap<(String, String), String>,
        touched: Vec<String>,
    }

    impl RegistryBackend for FakeRegistry {
        fn set_string(&mut self, key: &str, name: &str, data: &str) -> Result<(), DriveError> {
            self.touched.push(key.to_owned());
            self.values.insert((key.to_ascii_lowercase(), name.to_owned()), data.to_owned());
            Ok(())
        }
        fn get_string(&self, key: &str, name: &str) -> Result<Option<String>, DriveError> {
            Ok(self.values.get(&(key.to_ascii_lowercase(), name.to_owned())).cloned())
        }
        fn delete_tree(&mut self, key: &str) -> Result<(), DriveError> {
            self.touched.push(key.to_owned());
            let prefix = key.to_ascii_lowercase();
            self.values.retain(|(k, _), _| !(k == &prefix || k.starts_with(&format!("{prefix}\\"))));
            Ok(())
        }
    }

    #[test]
    fn windows_command_format() {
        let cmd = windows_command(Path::new(r"C:\Program Files\CAD\StepViewer.exe")).unwrap();
        assert_eq!(cmd, r#""C:\Program Files\CAD\StepViewer.exe" --open-link "%1""#);
    }

    #[test]
    fn windows_command_rejects_dangerous_paths() {
        for p in [r#"C:\a"b\x.exe"#, r"C:\a%1\x.exe", "C:\\a\nb.exe", ""] {
            assert!(windows_command(Path::new(p)).is_err(), "{p:?}");
        }
    }

    #[test]
    fn registers_only_below_root() {
        let mut reg = FakeRegistry::default();
        register_link_with(&mut reg, Path::new(r"C:\Apps\Viewer\StepViewer.exe")).unwrap();
        assert!(!reg.touched.is_empty());
        for key in &reg.touched {
            assert!(is_under_root(key), "{key}");
        }
        let get = |k: &str, n: &str| reg.get_string(k, n).unwrap();
        assert_eq!(get(REGISTRY_ROOT, "").as_deref(), Some("URL:CAD prohlížeč"));
        assert_eq!(get(REGISTRY_ROOT, "URL Protocol").as_deref(), Some(""));
        assert_eq!(
            get(r"Software\Classes\cadviewer\shell\open\command", "").as_deref(),
            Some(r#""C:\Apps\Viewer\StepViewer.exe" --open-link "%1""#)
        );
        assert_eq!(
            get(r"Software\Classes\cadviewer\DefaultIcon", "").as_deref(),
            Some(r#""C:\Apps\Viewer\StepViewer.exe",0"#)
        );
    }

    #[test]
    fn unregister_removes_ours_and_is_idempotent() {
        let mut reg = FakeRegistry::default();
        register_link_with(&mut reg, Path::new(r"C:\Apps\StepViewer.exe")).unwrap();
        unregister_link_with(&mut reg, Path::new(r"c:\apps\stepviewer.exe")).unwrap();
        assert!(reg.values.is_empty());
        unregister_link_with(&mut reg, Path::new(r"C:\Apps\StepViewer.exe")).unwrap();
    }

    #[test]
    fn unregister_leaves_registration_of_another_copy_alone() {
        let mut reg = FakeRegistry::default();
        register_link_with(&mut reg, Path::new(r"C:\Program Files\StepViewer\StepViewer.exe")).unwrap();
        let before = reg.values.clone();
        reg.touched.clear();
        let err = unregister_link_with(&mut reg, Path::new(r"G:\Můj disk\StepViewer\StepViewer.exe")).unwrap_err();
        assert!(err.to_string().contains("jinou kopii"));
        assert!(reg.touched.is_empty());
        assert_eq!(reg.values, before);
    }

    #[test]
    fn unregister_leaves_foreign_key_alone() {
        let mut reg = FakeRegistry::default();
        reg.set_string(&format!(r"{REGISTRY_ROOT}\shell\open\command"), "", r#""C:\Other\app.exe" "%1""#)
            .unwrap();
        reg.touched.clear();
        let err = unregister_link_with(&mut reg, Path::new(r"C:\Apps\StepViewer.exe")).unwrap_err();
        assert!(matches!(err, DriveError::Registration(_)));
        assert!(reg.touched.is_empty());
        assert_eq!(reg.values.len(), 1);
    }

    #[test]
    fn root_check() {
        assert!(is_under_root(r"Software\Classes\cadviewer"));
        assert!(is_under_root(r"software\classes\CADVIEWER\shell"));
        assert!(is_under_root(r"\Software\Classes\cadviewer\"));
        assert!(!is_under_root(r"Software\Classes\cadviewer2"));
        assert!(!is_under_root(r"Software\Classes\cadviewerX\shell"));
        assert!(!is_under_root(r"Software\Classes"));
        assert!(!is_under_root(r"Software\Classes\http"));
        assert!(!is_under_root(r"Software\Microsoft\Windows"));
        assert!(!is_under_root(""));
        assert!(!is_under_root(r"Software\Classes\cadviewer\..\http"));
        assert!(!is_under_root(r"Software\Classes\cadviewer\.\x"));
        assert!(!is_under_root(r"Software\Classes\cadviewer\\x"));
    }

    #[test]
    fn unregister_cleans_partial_registration_of_ours() {
        let mut reg = FakeRegistry::default();
        let exe = Path::new(r"C:\Apps\StepViewer.exe");
        reg.set_string(REGISTRY_ROOT, "", REG_DESCRIPTION).unwrap();
        unregister_link_with(&mut reg, exe).unwrap();
        assert!(reg.values.is_empty());
        // A root key with another description and no command is not ours.
        reg.set_string(REGISTRY_ROOT, "", "něco jiného").unwrap();
        unregister_link_with(&mut reg, exe).unwrap();
        assert_eq!(reg.values.len(), 1);
    }

    #[test]
    fn exec_quoting() {
        assert_eq!(desktop_exec_quote("/usr/bin/viewer"), "\"/usr/bin/viewer\"");
        assert_eq!(desktop_exec_quote("/opt/my app/v"), "\"/opt/my app/v\"");
        assert_eq!(desktop_exec_quote("/a\"b$c`d\\e%f"), "\"/a\\\"b\\$c\\`d\\\\\\\\e%%f\"");
    }

    #[test]
    fn desktop_entry_content() {
        let text = desktop_entry(Path::new("/opt/Step Viewer/StepViewer")).unwrap();
        assert!(text.starts_with("[Desktop Entry]\n"));
        assert!(text.contains("Exec=\"/opt/Step Viewer/StepViewer\" --open-link %u\n"));
        assert!(text.contains("MimeType=x-scheme-handler/cadviewer;\n"));
        assert!(text.contains("NoDisplay=true\n"));
        assert!(text.contains(DESKTOP_MARKER));
        assert!(desktop_entry(Path::new("/opt/a\nb")).is_err());
    }

    #[test]
    fn mimeapps_set_into_empty_and_existing() {
        assert_eq!(
            mimeapps_set_default(""),
            "[Default Applications]\nx-scheme-handler/cadviewer=cadviewer-link.desktop;\n"
        );
        let existing = "[Added Associations]\ntext/plain=a.desktop;\n\n[Default Applications]\nx-scheme-handler/http=b.desktop;\n";
        let got = mimeapps_set_default(existing);
        assert_eq!(
            got,
            "[Added Associations]\ntext/plain=a.desktop;\n\n[Default Applications]\nx-scheme-handler/http=b.desktop;\nx-scheme-handler/cadviewer=cadviewer-link.desktop;\n"
        );
        // Idempotent.
        assert_eq!(mimeapps_set_default(&got), got);
        // Replaces a foreign handler for our scheme and keeps later sections.
        let foreign = "[Default Applications]\nx-scheme-handler/cadviewer=other.desktop;\n[Removed Associations]\nfoo/bar=x.desktop;\n";
        assert_eq!(
            mimeapps_set_default(foreign),
            "[Default Applications]\nx-scheme-handler/cadviewer=cadviewer-link.desktop;\n[Removed Associations]\nfoo/bar=x.desktop;\n"
        );
        // Section in the middle gets the entry before the next section.
        let middle = "[Default Applications]\na/b=c.desktop;\n[Added Associations]\nd/e=f.desktop;\n";
        assert_eq!(
            mimeapps_set_default(middle),
            "[Default Applications]\na/b=c.desktop;\nx-scheme-handler/cadviewer=cadviewer-link.desktop;\n[Added Associations]\nd/e=f.desktop;\n"
        );
    }

    #[test]
    fn mimeapps_remove() {
        let text = "[Default Applications]\na/b=c.desktop;\nx-scheme-handler/cadviewer=cadviewer-link.desktop;\n";
        assert_eq!(mimeapps_remove_default(text), "[Default Applications]\na/b=c.desktop;\n");
        let shared = "[Default Applications]\nx-scheme-handler/cadviewer=other.desktop;cadviewer-link.desktop;\n";
        assert_eq!(
            mimeapps_remove_default(shared),
            "[Default Applications]\nx-scheme-handler/cadviewer=other.desktop;\n"
        );
        assert_eq!(mimeapps_remove_default(""), "");
        // Same key in another section is not ours.
        let other = "[Added Associations]\nx-scheme-handler/cadviewer=cadviewer-link.desktop;\n";
        assert_eq!(mimeapps_remove_default(other), other);
    }

    #[test]
    fn xdg_register_and_unregister_in_temp_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = XdgDirs {
            data_home: tmp.path().join("data"),
            config_home: tmp.path().join("config"),
        };
        let exe = Path::new("/opt/viewer/StepViewer");
        // Existing user file is kept.
        fs::create_dir_all(&dirs.config_home).unwrap();
        fs::write(dirs.mimeapps_list(), "[Default Applications]\ntext/html=firefox.desktop;\n").unwrap();

        register_link_xdg(&dirs, exe).unwrap();
        let desktop = fs::read_to_string(dirs.desktop_file()).unwrap();
        assert!(desktop.contains("Exec=\"/opt/viewer/StepViewer\" --open-link %u"));
        let list = fs::read_to_string(dirs.mimeapps_list()).unwrap();
        assert!(list.contains("text/html=firefox.desktop;"));
        assert!(list.contains("x-scheme-handler/cadviewer=cadviewer-link.desktop;"));
        // Registering again changes nothing.
        register_link_xdg(&dirs, exe).unwrap();
        assert_eq!(fs::read_to_string(dirs.mimeapps_list()).unwrap(), list);

        unregister_link_xdg(&dirs).unwrap();
        assert!(!dirs.desktop_file().exists());
        let list = fs::read_to_string(dirs.mimeapps_list()).unwrap();
        assert_eq!(list, "[Default Applications]\ntext/html=firefox.desktop;\n");
        unregister_link_xdg(&dirs).unwrap(); // nothing left to do
        // Only the two expected files exist, no leftovers.
        let names: Vec<_> = fs::read_dir(dirs.data_home.join("applications"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(names.is_empty(), "{names:?}");
    }

    #[test]
    fn xdg_unregister_keeps_foreign_desktop_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = XdgDirs { data_home: tmp.path().join("d"), config_home: tmp.path().join("c") };
        fs::create_dir_all(dirs.data_home.join("applications")).unwrap();
        fs::write(dirs.desktop_file(), "[Desktop Entry]\nName=Cizí\n").unwrap();
        assert!(matches!(unregister_link_xdg(&dirs), Err(DriveError::Registration(_))));
        assert!(dirs.desktop_file().exists());
    }

    #[test]
    fn public_register_checks_the_exe() {
        assert!(register_link(Path::new("relative/StepViewer.exe")).is_err());
        assert!(register_link(Path::new(if cfg!(windows) { r"C:\neexistuje\nic.exe" } else { "/neexistuje/nic" })).is_err());
    }
}
