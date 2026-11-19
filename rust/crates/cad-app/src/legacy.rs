//! The settings of the 1.9 version, read once when there is no settings file yet. The Python version kept them with
//! `QSettings("CadProhlizec", "StepViewer")`, which on Windows is the registry key `HKCU\Software\CadProhlizec\StepViewer`
//! (the installer removes it on uninstall). The key is only ever read here, never written; the import itself and its
//! tests are `cad_core::settings::Settings::load_or_import`.
use cad_core::settings::{LegacySettings, LegacyValue, NoLegacy};

/// The key of the 1.9 version below HKCU.
pub const KEY: &str = r"Software\CadProhlizec\StepViewer";

/// Registry subkey and value name of a QSettings key: `spacemouse/speed` is the value `speed` of the subkey
/// `spacemouse`; a name without a group is a value of the key itself.
pub fn registry_location(name: &str) -> (String, String) {
    match name.rsplit_once('/') {
        Some((group, value)) => (format!(r"{KEY}\{}", group.replace('/', r"\")), value.to_string()),
        None => (KEY.to_string(), name.to_string()),
    }
}

/// The registry of the current user.
#[cfg(windows)]
pub struct Registry;

#[cfg(windows)]
impl LegacySettings for Registry {
    fn get(&self, name: &str) -> Option<LegacyValue> {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
        use winreg::RegKey;
        let (key, value) = registry_location(name);
        let k = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(&key, KEY_READ).ok()?;
        // Qt writes a list as REG_MULTI_SZ, numbers as REG_DWORD and everything else (booleans, floats, text) as REG_SZ
        if let Ok(list) = k.get_value::<Vec<String>, _>(&value) {
            return Some(LegacyValue::List(list));
        }
        if let Ok(text) = k.get_value::<String, _>(&value) {
            return Some(LegacyValue::Text(text));
        }
        k.get_value::<u32, _>(&value).ok().map(LegacyValue::Number)
    }
}

/// Where the old settings come from: the registry on Windows, nowhere elsewhere and in portable mode (which never
/// touches the registry).
pub fn source() -> Box<dyn LegacySettings> {
    #[cfg(windows)]
    {
        if !cad_core::paths::is_portable() {
            return Box::new(Registry);
        }
    }
    Box::new(NoLegacy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qsettings_names_map_to_registry_values() {
        assert_eq!(registry_location("recent"), (KEY.to_string(), "recent".to_string()));
        assert_eq!(
            registry_location("spacemouse/speed"),
            (r"Software\CadProhlizec\StepViewer\spacemouse".to_string(), "speed".to_string())
        );
        assert_eq!(
            registry_location("spacemouse/invert3").0,
            r"Software\CadProhlizec\StepViewer\spacemouse"
        );
        assert_eq!(registry_location("a/b/c"), (format!(r"{KEY}\a\b"), "c".to_string()));
    }

    #[test]
    fn the_key_is_the_one_of_the_python_version() {
        // SPEC section 14: QSettings("CadProhlizec", "StepViewer")
        assert_eq!(KEY, r"Software\CadProhlizec\StepViewer");
    }
}
