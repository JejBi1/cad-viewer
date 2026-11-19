//! Import checks of Windows binaries with `dumpbin` (from the portable MSVC, no admin rights needed).
use crate::util::{self, Toolchain};
use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// DLL names a binary imports (static and delay-load imports), lower case.
pub fn dependents(tc: &Toolchain, file: &Path) -> Result<Vec<String>> {
    let dumpbin = tc.require("dumpbin")?;
    let mut cmd = std::process::Command::new(dumpbin);
    tc.apply(&mut cmd);
    cmd.arg("/nologo").arg("/dependents").arg(file);
    let (ok, text) = util::capture(&mut cmd)?;
    if !ok {
        bail!("dumpbin /dependents {} failed:\n{text}", file.display());
    }
    let mut set = BTreeSet::new();
    for line in text.lines() {
        let t = line.trim();
        if t.to_lowercase().ends_with(".dll") && !t.contains(' ') && !t.contains('\\') {
            set.insert(t.to_lowercase());
        }
    }
    Ok(set.into_iter().collect())
}

fn system32() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32")
}

/// True for DLLs of Windows itself (System32 or an API-set name), which every machine has.
pub fn is_system_dll(name: &str) -> bool {
    name.starts_with("api-ms-win-") || name.starts_with("ext-ms-win-") || system32().join(name).is_file()
}

/// Names that must not appear in a self-contained executable: OCCT DLLs and the Visual C++ runtime (when the CRT is
/// linked statically).
pub fn forbidden(name: &str, crt_static: bool) -> bool {
    if name.starts_with("tk") {
        return true; // OCCT toolkit DLLs
    }
    crt_static
        && (name.starts_with("vcruntime")
            || name.starts_with("msvcp")
            || name.starts_with("concrt")
            || name.starts_with("vcomp")
            || name.starts_with("ucrtbase")
            || name.starts_with("api-ms-win-crt-"))
}

/// Imports of `file`, checked: no OCCT DLL, no VC runtime for a static CRT, nothing outside Windows itself.
/// Returns the import list (for the log).
pub fn check_self_contained(tc: &Toolchain, file: &Path, crt_static: bool) -> Result<Vec<String>> {
    let deps = dependents(tc, file).with_context(|| format!("imports of {}", file.display()))?;
    let mut bad = Vec::new();
    for d in &deps {
        if forbidden(d, crt_static) {
            bad.push(format!("{d} (forbidden)"));
        } else if !is_system_dll(d) {
            bad.push(format!("{d} (not a Windows system DLL)"));
        }
    }
    if !bad.is_empty() {
        bail!("{} imports: {}", file.display(), bad.join(", "));
    }
    Ok(deps)
}

/// DLLs of `dir` that `file` needs, directly or through other DLLs of `dir`: (names, total bytes).
/// The size of what the program loads at run time when OCCT is not linked statically.
pub fn dll_closure(tc: &Toolchain, file: &Path, dir: &Path) -> Result<(Vec<String>, u64)> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut todo = vec![file.to_path_buf()];
    let mut bytes = 0u64;
    while let Some(f) = todo.pop() {
        for d in dependents(tc, &f)? {
            let p = dir.join(&d);
            if p.is_file() && seen.insert(d.clone()) {
                bytes += std::fs::metadata(&p)?.len();
                todo.push(p);
            }
        }
    }
    Ok((seen.into_iter().collect(), bytes))
}

/// True when `needle` occurs in `hay` (byte search; the sizes here are megabytes).
fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// True when the file contains `needle` as raw bytes.
pub fn contains_bytes(file: &Path, needle: &[u8]) -> Result<bool> {
    let image = std::fs::read(file).with_context(|| file.display().to_string())?;
    Ok(contains(&image, needle))
}

/// The executable has a version resource: its UTF-16 data names `VS_VERSION_INFO` and the product name. A byte search
/// for the strings the resource compiler writes (cad-app's build script compiles the resource with rc.exe), not a parser.
pub fn has_version_resource(file: &Path) -> Result<bool> {
    let image = std::fs::read(file).with_context(|| file.display().to_string())?;
    let utf16 = |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(|c| c.to_le_bytes()).collect() };
    Ok(contains(&image, &utf16("VS_VERSION_INFO")) && contains(&image, &utf16("CAD prohlížeč")))
}

/// The executable carries the application manifest with long path awareness and the per-monitor DPI setting: the
/// manifest text is stored as it is in the resource, so a byte search finds its elements.
pub fn has_long_path_manifest(file: &Path) -> Result<bool> {
    let image = std::fs::read(file).with_context(|| file.display().to_string())?;
    Ok(contains(&image, b"longPathAware") && contains(&image, b"PerMonitorV2"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The manifest the build script compiles in (cad-app/app.manifest).
    const MANIFEST: &str = include_str!("../../crates/cad-app/app.manifest");

    #[test]
    fn the_manifest_asks_for_long_paths_and_the_dpi_setting_of_the_window_system() {
        assert!(MANIFEST.contains(">true</longPathAware>"));
        assert!(MANIFEST.contains(">PerMonitorV2, PerMonitor</dpiAwareness>"));
        // eframe and rfd need no common-controls version 6 here: the manifest declares no dependency on it
        assert!(!MANIFEST.contains("Microsoft.Windows.Common-Controls"));
    }

    #[test]
    fn a_built_executable_is_recognised_by_its_manifest() {
        let dir = std::env::temp_dir().join(format!("stepviewer-pe-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let with = dir.join("with.exe");
        let without = dir.join("without.exe");
        // the resource holds the manifest bytes next to other data
        std::fs::write(&with, [b"MZ\0\0".as_slice(), MANIFEST.as_bytes(), b"\0\0"].concat()).unwrap();
        std::fs::write(&without, b"MZ\0\0 no manifest here").unwrap();
        let found = (has_long_path_manifest(&with).unwrap(), has_long_path_manifest(&without).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(found, (true, false));
    }
}
