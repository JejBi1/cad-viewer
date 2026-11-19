//! Windows link settings and resources of `StepViewer.exe`:
//! * a main-thread stack of 64 MiB: Open CASCADE recurses deeply on nested assemblies and the window thread also calls
//!   it (mass, bounds, measuring), while Windows gives a thread 1 MiB by default;
//! * one resource script, compiled with `rc.exe` when it can be found: the program icon (`assets/stepview.ico`), the
//!   application manifest (`app.manifest`: long path awareness and DPI awareness) and the version resource (Explorer
//!   "Properties", installer and uninstaller). The version comes from the workspace version (`CARGO_PKG_VERSION`), so
//!   `2.0.0` gives FileVersion 2.0.0.0. Without `rc.exe` the program works, with the default icon, no version
//!   information and no manifest (so paths over 260 characters fail); `cargo xtask dist` checks the version resource
//!   and the manifest.
use std::path::{Path, PathBuf};
use std::process::Command;

const STACK: u64 = 64 * 1024 * 1024;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|v| v == "windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|v| v == "msvc");
    if !(windows && msvc) {
        return;
    }
    println!("cargo:rustc-link-arg-bins=/STACK:{STACK}");
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let icon = manifest_dir.join("..").join("..").join("..").join("assets").join("stepview.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let app_manifest = manifest_dir.join("app.manifest");
    println!("cargo:rerun-if-changed={}", app_manifest.display());
    let Ok(out_dir) = std::env::var("OUT_DIR") else { return };
    let out_dir = PathBuf::from(out_dir);
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let icon = icon.is_file().then_some(icon);
    let app_manifest = app_manifest.is_file().then_some(app_manifest);
    let resources = Resources { icon: icon.as_deref(), manifest: app_manifest.as_deref(), version: &version };
    match compile_resources(&resources, &out_dir) {
        Some(res) => println!("cargo:rustc-link-arg-bins={}", res.display()),
        None => println!(
            "cargo:warning=program icon, version information and application manifest not embedded (rc.exe not found or failed)"
        ),
    }
}

/// What the resource script of the executable holds.
struct Resources<'a> {
    icon: Option<&'a Path>,
    manifest: Option<&'a Path>,
    version: &'a str,
}

/// `rc.exe` from the PATH (the build environment of `devenv.ps1` and `cargo xtask dist` puts the Windows SDK there).
fn find_rc() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("rc.exe")).find(|p| p.is_file())
}

/// `1.2.3` -> `(1, 2, 3, 0)`; a pre-release suffix is ignored. Missing parts are zero.
fn version_parts(version: &str) -> [u16; 4] {
    let core = version.split(['-', '+']).next().unwrap_or("0");
    let mut out = [0u16; 4];
    for (i, part) in core.split('.').take(4).enumerate() {
        out[i] = part.parse().unwrap_or(0);
    }
    out
}

/// A path as the resource compiler takes it in a string: absolute, forward slashes, no `\\?\` prefix.
fn rc_path(p: &Path) -> String {
    std::fs::canonicalize(p)
        .map(|c| c.to_string_lossy().trim_start_matches(r"\\?\").replace('\\', "/"))
        .unwrap_or_else(|_| p.to_string_lossy().replace('\\', "/"))
}

/// The resource script: icon (optional), application manifest (optional, RT_MANIFEST = 24, id 1 = the program's own
/// manifest) and version information. UTF-8 source (`code_page(65001)`) for the Czech name.
fn resource_script(res: &Resources<'_>) -> String {
    let [a, b, c, d] = version_parts(res.version);
    let core = format!("{a}.{b}.{c}");
    let mut s = String::from("#pragma code_page(65001)\r\n");
    if let Some(icon) = res.icon {
        s.push_str(&format!("1 ICON \"{}\"\r\n", rc_path(icon)));
    }
    if let Some(manifest) = res.manifest {
        s.push_str(&format!("1 24 \"{}\"\r\n", rc_path(manifest)));
    }
    s.push_str(&format!(
        "1 VERSIONINFO\r\n\
         FILEVERSION {a},{b},{c},{d}\r\n\
         PRODUCTVERSION {a},{b},{c},{d}\r\n\
         FILEFLAGSMASK 0x3fL\r\n\
         FILEFLAGS 0x0L\r\n\
         FILEOS 0x40004L\r\n\
         FILETYPE 0x1L\r\n\
         FILESUBTYPE 0x0L\r\n\
         BEGIN\r\n\
         \x20   BLOCK \"StringFileInfo\"\r\n\
         \x20   BEGIN\r\n\
         \x20       BLOCK \"040504b0\"\r\n\
         \x20       BEGIN\r\n\
         \x20           VALUE \"CompanyName\", \"CadProhlizec\"\r\n\
         \x20           VALUE \"FileDescription\", \"CAD prohlížeč\"\r\n\
         \x20           VALUE \"FileVersion\", \"{core}.{d}\"\r\n\
         \x20           VALUE \"InternalName\", \"StepViewer\"\r\n\
         \x20           VALUE \"OriginalFilename\", \"StepViewer.exe\"\r\n\
         \x20           VALUE \"ProductName\", \"CAD prohlížeč\"\r\n\
         \x20           VALUE \"ProductVersion\", \"{core}\"\r\n\
         \x20       END\r\n\
         \x20   END\r\n\
         \x20   BLOCK \"VarFileInfo\"\r\n\
         \x20   BEGIN\r\n\
         \x20       VALUE \"Translation\", 0x405, 1200\r\n\
         \x20   END\r\n\
         END\r\n"
    ));
    s
}

/// Compiles the resource script to `stepviewer.res` in `out_dir`; `None` when there is no `rc.exe` or it fails.
fn compile_resources(res: &Resources<'_>, out_dir: &Path) -> Option<PathBuf> {
    let rc = find_rc()?;
    let script = out_dir.join("stepviewer.rc");
    std::fs::write(&script, resource_script(res)).ok()?;
    let res = out_dir.join("stepviewer.res");
    let status = Command::new(rc).arg("/nologo").arg("/fo").arg(&res).arg(&script).status().ok()?;
    (status.success() && res.is_file()).then_some(res)
}
