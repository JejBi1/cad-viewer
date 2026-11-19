//! Windows: per-format on/off switch for the Explorer thumbnail handler (current user, HKCU). Port of
//! `stepview/thumbsettings.py`. The registry code is Windows only; the rules (which keys, which folders count as
//! installed, the texts) are plain functions with tests.
use std::path::{Path, PathBuf};

pub const CLSID: &str = "{7A1F3C52-8E0B-4C6D-9F2A-5B3E1D4C6A70}";
pub const IID_THUMB: &str = "{e357fccd-a995-4576-b01f-234630154e96}";
/// `FORMATS`: name and extensions.
pub const FORMATS: [(&str, &[&str]); 5] = [
    ("STEP", &[".step", ".stp"]),
    ("IGES", &[".iges", ".igs"]),
    ("STL", &[".stl"]),
    ("DXF", &[".dxf"]),
    ("DWG", &[".dwg"]),
];

pub const TITLE: &str = "Náhledy v Průzkumníku";
pub const INTRO: &str = "Zobrazovat v Průzkumníku Windows náhled modelu / výkresu místo ikony u souborů:";
pub const DWG_SUFFIX: &str = "   – méně bezpečné (převodník LibreDWG)";
pub const DWG_TIP: &str = "Náhled spouští převodník LibreDWG na každý zobrazený soubor. Ten je méně odolný vůči poškozeným nebo podvrženým souborům – zapínej jen, když pracuješ s DWG z důvěryhodných zdrojů. Vykreslování náhledů běží v omezeném procesu (nízká integrita, limit paměti).";
pub const MACHINE_TIP: &str = "Zapnuto instalací pro všechny uživatele – změnit jde jen přeinstalací";
pub const INFO: &str = "Pozn.: ve složce SynologyDrive náhledy nefungují (Synology tam používá vlastní).\nUž vytvořené náhledy si Windows pamatují – po vypnutí je smaž tlačítkem níže (Vyčištění disku › Miniatury).";
pub const CLEAN_BUTTON: &str = "Vymazat uložené náhledy…";
pub const NOT_INSTALLED: &str = "Náhledy v Průzkumníku jde zapnout jen z nainstalované verze (instalátor CadProhlizec_setup).\n\nTento program neběží ze složky, kam instalátor program ukládá (Program Files nebo %LOCALAPPDATA%\\Programs), a Průzkumník by načítal knihovnu z umístění, do kterého může zapisovat i někdo jiný. Nainstaluj program a náhledy zapni v něm.";
pub const NO_DLL: &str = "Chybí StepThumb.dll vedle programu – náhledy nejde zapnout.";
pub const COM_NAME: &str = "CAD prohlížeč – náhledy";
pub const ASSEMBLY: &str = "StepThumb, Version=1.3.0.0, Culture=neutral, PublicKeyToken=null";

/// Label of the check box of a format: `"STEP  (.step, .stp)"`, DWG with the warning suffix.
pub fn format_label(name: &str, exts: &[&str]) -> String {
    format!("{name}  ({}){}", exts.join(", "), if name == "DWG" { DWG_SUFFIX } else { "" })
}

/// Registry key (below HKCU or HKLM) of the handler of an extension.
pub fn key_path(ext: &str) -> String {
    format!(r"Software\Classes\SystemFileAssociations\{ext}\ShellEx\{IID_THUMB}")
}

pub fn clsid_key() -> String {
    format!(r"Software\Classes\CLSID\{CLSID}")
}

/// `CodeBase` value: the DLL path with forward slashes behind `file:///`.
pub fn code_base(dll: &Path) -> String {
    format!("file:///{}", dll.to_string_lossy().replace('\\', "/"))
}

/// Folders where the installer puts the program: `%LOCALAPPDATA%\Programs` and the Program Files folders.
pub fn install_roots(env: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(l) = env("LOCALAPPDATA").filter(|v| !v.is_empty()) {
        roots.push(PathBuf::from(l).join("Programs"));
    }
    for v in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Some(p) = env(v).filter(|v| !v.is_empty()) {
            roots.push(PathBuf::from(p));
        }
    }
    roots
}

fn norm(p: &Path) -> String {
    p.to_string_lossy().replace('/', "\\").to_lowercase().trim_end_matches('\\').to_string()
}

/// True when `exe_dir` is one of the roots or below one. A copy that runs from a downloaded, synced or shared folder
/// is not installed: Explorer would load a library from a place somebody else may write to.
pub fn is_installed_dir(exe_dir: &Path, roots: &[PathBuf]) -> bool {
    let here = norm(exe_dir);
    roots.iter().any(|r| {
        let r = norm(r);
        !r.is_empty() && (here == r || here.starts_with(&format!("{r}\\")))
    })
}

#[cfg(windows)]
mod imp {
    use super::*;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;

    fn handler_is(root: winreg::HKEY, ext: &str) -> bool {
        RegKey::predef(root)
            .open_subkey(key_path(ext))
            .and_then(|k| k.get_value::<String, _>(""))
            .map(|v| v.eq_ignore_ascii_case(CLSID))
            .unwrap_or(false)
    }

    /// True if our handler is registered for `ext` (current user or machine-wide).
    pub fn is_on(ext: &str) -> bool {
        handler_is(HKEY_CURRENT_USER, ext) || handler_is(HKEY_LOCAL_MACHINE, ext)
    }

    pub fn machine_wide(ext: &str) -> bool {
        handler_is(HKEY_LOCAL_MACHINE, ext)
    }

    /// Real folder of the running program when it is an installed copy, else `None`.
    pub fn installed_dir() -> Option<PathBuf> {
        if cad_core::paths::is_portable() {
            return None;
        }
        let exe = std::env::current_exe().ok()?;
        let dir = cad_core::paths::real_path(exe.parent()?);
        let roots: Vec<PathBuf> =
            install_roots(&|v| std::env::var(v).ok()).iter().map(|r| cad_core::paths::real_path(r)).collect();
        is_installed_dir(&dir, &roots).then_some(dir)
    }

    /// Registers the COM server for the current user if the installer did not. Explorer loads the registered DLL into
    /// its own process, so this is done only for the copy the installer put into Program Files / Local Programs.
    /// Returns the Czech reason why thumbnails cannot be enabled.
    pub fn ensure_clsid() -> Result<(), String> {
        let sub = format!(r"{}\InprocServer32", clsid_key());
        for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            if RegKey::predef(root).open_subkey(&sub).is_ok() {
                return Ok(());
            }
        }
        let folder = installed_dir().ok_or_else(|| NOT_INSTALLED.to_string())?;
        let dll = folder.join("StepThumb.dll");
        if !dll.exists() {
            return Err(NO_DLL.to_string());
        }
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (k, _) = hkcu.create_subkey(clsid_key()).map_err(|e| e.to_string())?;
        k.set_value("", &COM_NAME).map_err(|e| e.to_string())?;
        let (k, _) = hkcu.create_subkey(&sub).map_err(|e| e.to_string())?;
        for (name, val) in [
            ("", "mscoree.dll".to_string()),
            ("ThreadingModel", "Both".to_string()),
            ("Class", "StepThumb.StepThumbnailProvider".to_string()),
            ("Assembly", ASSEMBLY.to_string()),
            ("RuntimeVersion", "v4.0.30319".to_string()),
            ("CodeBase", code_base(&dll)),
        ] {
            k.set_value(name, &val).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn set_on(ext: &str, on: bool) -> std::io::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if on {
            let (k, _) = hkcu.create_subkey(key_path(ext))?;
            k.set_value("", &CLSID)?;
        } else {
            // removing a key that is not there is not an error
            let _ = hkcu.delete_subkey(key_path(ext));
        }
        Ok(())
    }

    /// `SHChangeNotify(SHCNE_ASSOCCHANGED)`: Explorer re-reads the associations.
    pub fn notify_shell() {
        // SAFETY: documented call with no item identifiers
        unsafe {
            windows_sys::Win32::UI::Shell::SHChangeNotify(0x0800_0000, 0, std::ptr::null(), std::ptr::null());
        }
    }

    /// `cleanmgr.exe` by full path: a bare name is searched in the program's / current folder first (exe planting).
    pub fn open_disk_cleanup() {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let exe = Path::new(&root).join("System32").join("cleanmgr.exe");
        let _ = std::process::Command::new(exe).spawn();
    }
}

#[cfg(windows)]
pub use imp::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_key_is_the_thumbnail_handler_of_the_extension() {
        assert_eq!(
            key_path(".step"),
            r"Software\Classes\SystemFileAssociations\.step\ShellEx\{e357fccd-a995-4576-b01f-234630154e96}"
        );
        assert_eq!(clsid_key(), r"Software\Classes\CLSID\{7A1F3C52-8E0B-4C6D-9F2A-5B3E1D4C6A70}");
    }

    #[test]
    fn formats_and_labels() {
        assert_eq!(FORMATS.len(), 5);
        assert_eq!(format_label("STEP", FORMATS[0].1), "STEP  (.step, .stp)");
        assert_eq!(format_label("STL", FORMATS[2].1), "STL  (.stl)");
        assert_eq!(format_label("DWG", FORMATS[4].1), "DWG  (.dwg)   – méně bezpečné (převodník LibreDWG)");
    }

    #[test]
    fn code_base_uses_forward_slashes() {
        assert_eq!(
            code_base(Path::new(r"C:\Users\Jan\AppData\Local\Programs\StepViewer\StepThumb.dll")),
            "file:///C:/Users/Jan/AppData/Local/Programs/StepViewer/StepThumb.dll"
        );
    }

    #[test]
    fn install_folders() {
        let env = |v: &str| match v {
            "LOCALAPPDATA" => Some(r"C:\Users\Jan\AppData\Local".to_string()),
            "ProgramFiles" => Some(r"C:\Program Files".to_string()),
            "ProgramFiles(x86)" => Some(r"C:\Program Files (x86)".to_string()),
            _ => None,
        };
        let roots = install_roots(&env);
        assert_eq!(roots.len(), 3);
        assert!(is_installed_dir(Path::new(r"C:\Program Files\StepViewer"), &roots));
        assert!(is_installed_dir(Path::new(r"c:\program files"), &roots));
        assert!(is_installed_dir(Path::new(r"C:\Users\Jan\AppData\Local\Programs\StepViewer"), &roots));
        // a sibling folder whose name only starts like a root is not below it
        assert!(!is_installed_dir(Path::new(r"C:\Program Files Extra\x"), &roots));
        // downloaded, synced or portable copies are not installed
        assert!(!is_installed_dir(Path::new(r"C:\Users\Jan\Downloads\StepViewer"), &roots));
        assert!(!is_installed_dir(Path::new(r"G:\My Drive\CAD\StepViewer"), &roots));
        assert!(!is_installed_dir(Path::new(r"C:\Users\Jan\AppData\Local\Other"), &roots));
        assert!(!is_installed_dir(Path::new(r"C:\Users\Jan\AppData\Local"), &roots));
    }

    #[test]
    fn missing_variables_give_no_roots() {
        assert!(install_roots(&|_| None).is_empty());
        assert!(!is_installed_dir(Path::new(r"C:\Program Files\x"), &[]));
    }
}
