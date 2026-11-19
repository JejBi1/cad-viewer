//! `cargo xtask linux-dist`: Linux packaging of the Rust program. Output (default `~/.local/cad-dist/linux[-no-dwg]/`):
//! * `cad-viewer-<ver>[-nodwg]/`                  the package folder: `install.sh`, `cad-viewer/` (program, third_party), licences
//! * `cad-viewer-<ver>[-nodwg]-linux-x86_64.tar.gz`
//! * `cad-viewer-<ver>[-nodwg]-x86_64.AppImage`   only when `appimagetool` is found (otherwise skipped, not an error)
//! * `size-report.txt`, `size-report.json`, `SHA256SUMS.txt`
//!
//! Verified on the Ubuntu 22.04 build host (`cadbuild`, glibc 2.35): `occt-static` with GCC (`-DUSE_XLIB=OFF`),
//! the Linux branch of `crates/occt-bridge/build.rs`, and `cargo xtask linux-dist` (`tar.gz` + `AppImage`).
//! Prerequisites on that host: Rust, cmake, ninja, gcc/g++, curl, tar, `libwayland-dev` (for `wayland-sys`),
//! `libvulkan1` / `mesa-vulkan-drivers` (for headless Vulkan tests); `cargo xtask occt-static`; for the full variant
//! the LibreDWG converter `dwg2dxf` (built by `linux/setup_build_env.sh` into `/opt/libredwg/bin`, or given with `--dwg2dxf FILE`).
//! The C/C++ side has no path remapping yet (GCC `-ffile-prefix-map` would need quoting, the repository path has spaces),
//! so build-machine paths may remain in the binary; the Rust side gets the same `--remap-path-prefix` as on Windows.
use crate::dist::{self, BuildFacts};
use crate::notices::{self, Platform};
use crate::occt;
use crate::sizes::{self, Component};
use crate::util::{self, Args, Toolchain, VERSION};
use anyhow::{Context, Result, anyhow, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

/// Size of the 1.9 AppImage (ARCHITECTURE.md, "Size budget").
const BASELINE_APPIMAGE_1_9: u64 = 100_317_688;

/// `cargo xtask linux-dist [--out DIR] [--target-dir DIR] [--no-dwg] [--no-build] [--dwg2dxf FILE]`
pub fn command(mut args: Args) -> Result<()> {
    if cfg!(windows) {
        bail!("linux-dist runs on a Linux build host (it is not run on Windows)");
    }
    let out = args.value("--out")?.map(PathBuf::from).unwrap_or_else(util::dist_root);
    let target_dir = args.value("--target-dir")?.map(PathBuf::from).unwrap_or_else(|| util::default_target_dir("linux"));
    let dwg2dxf_arg = args.value("--dwg2dxf")?.map(PathBuf::from);
    let no_dwg = args.flag("--no-dwg");
    let no_build = args.flag("--no-build");
    args.finish()?;
    util::forbid_synced(&out, "the output folder")?;
    util::forbid_synced(&target_dir, "the Cargo target folder")?;
    let started = Instant::now();

    let prefix = occt::default_prefix(occt::Crt::Dynamic);
    if !prefix.join("occt-static.txt").is_file() {
        bail!("static OCCT not found in {}: run `cargo xtask occt-static` first", prefix.display());
    }
    // looked for before the long build: the full variant cannot be made without the converter
    let converter = if no_dwg { None } else { Some(find_dwg2dxf(dwg2dxf_arg)?) };
    if !no_build {
        build_program(&target_dir, &prefix)?;
    }
    let bin = target_dir.join("dist").join("StepViewer");
    if !bin.is_file() {
        bail!("{} not found (the binary is named StepViewer by cad-app)", bin.display());
    }
    let tc = Toolchain::discover()?;
    let facts = dist::build_facts(&tc, &bin, Platform::Linux)?;

    let suffix = if no_dwg { "-nodwg" } else { "" };
    let root = out.join(if no_dwg { "linux-no-dwg" } else { "linux" });
    util::remove_tree(&root)?;
    let top = format!("cad-viewer-{VERSION}{suffix}");
    let pkg = root.join(&top);
    let app = pkg.join("cad-viewer");
    fs::create_dir_all(&app)?;
    util::copy_file(&bin, &app.join("cad-viewer"))?; // the name install.sh / the .desktop file expect
    set_exec_file(&app.join("cad-viewer"))?;
    let repo = util::repo_root();
    for f in ["install.sh", "cad-viewer.desktop", "cad-viewer-mime.xml"] {
        util::copy_file(&repo.join("linux").join(f), &pkg.join(f))?;
    }
    set_exec_file(&pkg.join("install.sh"))?;
    util::copy_file(&repo.join("stepview").join("stepview.png"), &pkg.join("cad-viewer.png"))?;
    for f in ["GPL-3.0-only.txt", "LGPL-2.1-only.txt"] {
        util::copy_file(&repo.join("installer").join("licenses").join(f), &pkg.join("licenses").join(f))?;
    }
    util::copy_file(
        &prefix.join("share").join("doc").join("opencascade").join("OCCT_LGPL_EXCEPTION.txt"),
        &pkg.join("licenses").join("OCCT_LGPL_EXCEPTION.txt"),
    )?;
    util::write_file(
        &pkg.join("licenses").join("THIRD_PARTY_NOTICES.txt"),
        notices::generate(&tc, !no_dwg, Platform::Linux)?,
    )?;
    if let Some(src) = &converter {
        let dir = app.join("third_party").join("libredwg-linux");
        util::copy_file(src, &dir.join("dwg2dxf"))?;
        set_exec_file(&dir.join("dwg2dxf"))?;
    }
    util::write_file(&pkg.join("README.txt"), readme(no_dwg))?;

    let tgz = root.join(format!("cad-viewer-{VERSION}{suffix}-linux-x86_64.tar.gz"));
    let mut t = Command::new("tar");
    t.arg("-C").arg(&root).arg("-czf").arg(&tgz).arg(&top);
    util::run(&mut t)?;
    println!("tar.gz: {} ({})", tgz.display(), util::mb(fs::metadata(&tgz)?.len()));

    let image = match find_appimagetool() {
        Some(tool) => {
            let file = format!("cad-viewer-{VERSION}{suffix}-x86_64.AppImage");
            let image = build_appimage(&tool, &root, &app, &pkg, &file)?;
            println!("AppImage: {} ({})", image.display(), util::mb(fs::metadata(&image)?.len()));
            Some(image)
        }
        None => {
            println!("appimagetool not found (linux/setup_build_env.sh installs it to /opt): no AppImage");
            None
        }
    };

    let report = size_report(&root, &pkg, &tgz, image.as_deref(), no_dwg, &facts)?;
    util::write_file(&root.join("size-report.txt"), &report)?;
    println!("{report}");

    let mut sums = String::new();
    for f in std::iter::once(&tgz).chain(image.iter()) {
        sums.push_str(&format!("{}  {}\n", util::sha256_file(f)?, f.file_name().unwrap_or_default().to_string_lossy()));
    }
    util::write_file(&root.join("SHA256SUMS.txt"), sums)?;
    eprintln!("linux-dist finished in {} s: {}", started.elapsed().as_secs(), root.display());
    Ok(())
}

/// The release build of cad-app for Linux: static OCCT, host target, the Rust path remapping, Drive variables forwarded.
fn build_program(target_dir: &Path, prefix: &Path) -> Result<()> {
    let cargo = std::env::var_os("CARGO").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("cargo"));
    let mut c = Command::new(cargo);
    c.current_dir(util::rust_dir())
        .args(["build", "--profile", "dist", "--package", "cad-app", "--features", "occt-bridge/static-occt"])
        .arg("--target-dir")
        .arg(target_dir)
        .env("OCCT_STATIC_PREFIX", prefix)
        .env_remove("RUSTFLAGS")
        .env("CARGO_ENCODED_RUSTFLAGS", dist::remap_path_flags().join("\x1f"));
    util::forward_drive_env(&mut c);
    util::run(&mut c)
}

/// The LibreDWG converter for the package: `--dwg2dxf FILE`, else `/opt/libredwg/bin/dwg2dxf` (built by
/// `linux/setup_build_env.sh`), else `third_party/libredwg-linux/dwg2dxf` in the repository. Without one the full
/// variant is not made: an error, not a warning.
fn find_dwg2dxf(arg: Option<PathBuf>) -> Result<PathBuf> {
    let candidates = match arg {
        Some(p) => vec![p],
        None => vec![
            PathBuf::from("/opt/libredwg/bin/dwg2dxf"),
            util::repo_root().join("third_party").join("libredwg-linux").join("dwg2dxf"),
        ],
    };
    candidates.iter().find(|p| p.is_file()).cloned().ok_or_else(|| {
        anyhow!(
            "no DWG converter (dwg2dxf) found, looked in {}: build LibreDWG with linux/setup_build_env.sh, pass \
             --dwg2dxf FILE, or make the base package with --no-dwg",
            candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
        )
    })
}

/// `appimagetool`: `/opt/appimagetool` (linux/setup_build_env.sh) or the one on the PATH; `None` when there is none.
fn find_appimagetool() -> Option<PathBuf> {
    let opt = PathBuf::from("/opt/appimagetool");
    if opt.is_file() {
        return Some(opt);
    }
    Toolchain::discover().ok()?.find("appimagetool")
}

fn build_appimage(tool: &Path, root: &Path, app: &Path, pkg: &Path, file: &str) -> Result<PathBuf> {
    let ad = root.join("AppDir");
    util::remove_tree(&ad)?;
    let lib = ad.join("usr").join("lib").join("cad-viewer");
    copy_dir(app, &lib)?;
    copy_dir(&pkg.join("licenses"), &lib.join("licenses"))?;
    util::copy_file(&pkg.join("cad-viewer.desktop"), &ad.join("cad-viewer.desktop"))?;
    util::copy_file(&pkg.join("cad-viewer.png"), &ad.join("cad-viewer.png"))?;
    util::write_file(
        &ad.join("AppRun"),
        "#!/bin/bash\nHERE=\"$(dirname \"$(readlink -f \"$0\")\")\"\nexec \"$HERE/usr/lib/cad-viewer/cad-viewer\" \"$@\"\n",
    )?;
    set_exec_file(&ad.join("AppRun"))?;
    let image = root.join(file);
    let mut a = Command::new(tool);
    a.env("ARCH", "x86_64").args(["--appimage-extract-and-run", "--no-appstream"]).arg(&ad).arg(&image);
    util::run(&mut a).context("appimagetool")?;
    Ok(image)
}

/// `size-report.txt` and `size-report.json`: bytes per part of the package, the archives, the build checks, and the 1.9
/// AppImage for comparison. Returns the text.
fn size_report(root: &Path, pkg: &Path, tgz: &Path, image: Option<&Path>, no_dwg: bool, facts: &BuildFacts) -> Result<String> {
    let files = util::list_files(pkg)?;
    let part = |rel: &str| -> &'static str {
        if rel == "cad-viewer/cad-viewer" {
            "cad-viewer (program)"
        } else if rel.starts_with("cad-viewer/third_party/") {
            "DWG converter"
        } else if rel.starts_with("licenses/") {
            "licences"
        } else {
            "other"
        }
    };
    let mut comps: Vec<Component> = Vec::new();
    for name in ["cad-viewer (program)", "DWG converter", "licences", "other"] {
        let f: Vec<_> = files.iter().filter(|(_, r)| part(r.as_str()) == name).cloned().collect();
        if !f.is_empty() {
            comps.push(Component { name: name.to_string(), note: String::new(), files: f });
        }
    }
    let rows = sizes::measure(&comps)?;
    let total = sizes::measure_total(&comps)?;

    let mut s = format!(
        "CAD prohlížeč {VERSION} – size report (Linux x86_64, {})\n\n",
        if no_dwg { "base without the DWG converter" } else { "full, with the DWG converter" }
    );
    s.push_str("Package content (xz = LZMA2 preset 9e, one solid stream per row; the archives are tar.gz and AppImage):\n");
    s.push_str(&sizes::render(&rows, &total));
    s.push_str(&dist::build_checks_text(facts));
    s.push_str("\nArtifacts:\n");
    let tgz_n = fs::metadata(tgz).map(|m| m.len()).unwrap_or(0);
    s.push_str(&format!("  {:<22} {:>14} bytes  {}\n", "tar.gz", sizes::group(tgz_n), util::mb(tgz_n)));
    match image {
        Some(p) => {
            let n = fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            s.push_str(&format!(
                "  {:<22} {:>14} bytes  {}  ({:+.1}% vs 1.9 AppImage {})\n",
                "AppImage",
                sizes::group(n),
                util::mb(n),
                100.0 * (n as f64 / BASELINE_APPIMAGE_1_9 as f64 - 1.0),
                util::mb(BASELINE_APPIMAGE_1_9)
            ));
        }
        None => s.push_str("  AppImage               not built (appimagetool not found)\n"),
    }
    s.push_str(&format!("\nBaseline 1.9: Linux AppImage {}.\n", util::mb(BASELINE_APPIMAGE_1_9)));

    let json = serde_json::json!({
        "version": VERSION,
        "platform": "linux-x86_64",
        "variant": if no_dwg { "no-dwg" } else { "full" },
        "components": rows,
        "total": total,
        "artifacts": [
            { "name": "tar.gz", "file": tgz.file_name().unwrap_or_default().to_string_lossy(), "bytes": tgz_n },
            {
                "name": "AppImage",
                "file": image.and_then(|p| p.file_name()).map(|f| f.to_string_lossy().into_owned()),
                "bytes": image.and_then(|p| fs::metadata(p).ok()).map(|m| m.len()),
            },
        ],
        "build_checks": {
            "fonts_in_exe": facts.fonts,
            "drive_client_id_configured": facts.drive_id_env,
            "drive_client_id_in_exe": facts.drive_id_in_exe,
        },
        "baseline_1_9": { "appimage_bytes": BASELINE_APPIMAGE_1_9 },
    });
    util::write_file(&root.join("size-report.json"), serde_json::to_vec_pretty(&json)?)?;
    Ok(s)
}

#[cfg(unix)]
fn set_exec(p: &mut fs::Permissions) {
    use std::os::unix::fs::PermissionsExt;
    p.set_mode(0o755);
}

#[cfg(not(unix))]
fn set_exec(_p: &mut fs::Permissions) {}

fn set_exec_file(p: &Path) -> Result<()> {
    let mut perm = fs::metadata(p).with_context(|| p.display().to_string())?.permissions();
    set_exec(&mut perm);
    fs::set_permissions(p, perm).with_context(|| p.display().to_string())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    for (p, rel) in util::list_files(from)? {
        util::copy_file(&p, &to.join(rel))?;
    }
    Ok(())
}

fn readme(no_dwg: bool) -> String {
    let (title_dwg, dwg_line) = if no_dwg {
        ("", "Výkresy DWG: tato verze nemá převodník DWG, výkresy DWG se neotevřou (STEP, IGES, STL a DXF fungují).\n")
    } else {
        (", DWG (2D)", "Výkresy DWG: převodník LibreDWG je v cad-viewer/third_party/libredwg-linux.\n")
    };
    format!(
        "CAD prohlížeč {VERSION} pro Linux (x86_64) – STEP, IGES, STL (3D), DXF{title_dwg}\n\n\
         Spuštění bez instalace:  ./cad-viewer/cad-viewer [soubor]\n\
         Instalace pro uživatele: ./install.sh   (sudo ./install.sh --system pro všechny; --default, --uninstall)\n\n\
         {dwg_line}\
         Potřebuje glibc 2.35+ a ovladač Vulkan (Mesa / GPU). Open CASCADE je přilinkováno staticky.\n\
         Tento program používá a je založen na technologii Open CASCADE Technology (licence v licenses/).\n"
    )
}
