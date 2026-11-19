//! `cargo xtask dist`: the Windows distribution, built from the Rust sources without Python.
//!
//! Output (default `~/.local/cad-dist/<variant>/`, variant `full` or `no-dwg`):
//! * `stage/StepViewer/`                      the installer stage: StepViewer.exe, StepThumb.dll, licenses/, third_party/libredwg/
//! * `CadProhlizec_setup_<ver>[_nodwg].exe`   the Inno Setup installer (ISCC from the user's own installation)
//! * `StepViewer-portable-<ver>[-nodwg]/` and `.zip`  portable folder with `portable.flag` (Google Drive), no StepThumb.dll
//! * `StepViewer-dwg-addon-<ver>.zip`         only with `--no-dwg`: the LibreDWG converter as a separate add-on
//! * `size-report.txt`, `size-report.json`, `SHA256SUMS.txt`
use crate::notices;
use crate::occt::{self, Crt};
use crate::pe;
use crate::sizes::{self, Component};
use crate::util::{self, Args, Toolchain, VERSION, WINDOWS_TARGET};
use anyhow::{Context, Result, bail};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Sizes of the 1.9 releases (ARCHITECTURE.md, "Size budget").
const BASELINE_INSTALLER_1_9: u64 = 55_901_762;
const BASELINE_APPIMAGE_1_9: u64 = 100_317_688;

struct Opts {
    no_dwg: bool,
    out: PathBuf,
    target_dir: PathBuf,
    exe: Option<PathBuf>,
    no_build: bool,
    no_installer: bool,
    no_sizes: bool,
    build_occt: bool,
    crt: Crt,
    occt_opt: occt::Opt,
    profile: String,
}

/// `cargo xtask dist [--no-dwg] [--out DIR] [--target-dir DIR] [--exe FILE] [--no-build] [--no-installer] [--no-sizes]
/// [--build-occt] [--crt static|dynamic] [--occt-opt speed|size] [--profile NAME]`
pub fn command(mut args: Args) -> Result<()> {
    if !cfg!(windows) {
        bail!("`dist` builds the Windows distribution; on Linux use `cargo xtask linux-dist`");
    }
    let o = Opts {
        no_dwg: args.flag("--no-dwg"),
        out: args.value("--out")?.map(PathBuf::from).unwrap_or_else(util::dist_root),
        target_dir: args.value("--target-dir")?.map(PathBuf::from).unwrap_or_else(|| util::default_target_dir("dist")),
        exe: args.value("--exe")?.map(PathBuf::from),
        no_build: args.flag("--no-build"),
        no_installer: args.flag("--no-installer"),
        no_sizes: args.flag("--no-sizes"),
        build_occt: args.flag("--build-occt"),
        crt: match args.value("--crt")? {
            Some(v) => Crt::parse(&v)?,
            None => Crt::Static,
        },
        occt_opt: match args.value("--occt-opt")? {
            Some(v) => occt::Opt::parse(&v)?,
            None => occt::Opt::Speed,
        },
        profile: args.value("--profile")?.unwrap_or_else(|| "dist".to_string()),
    };
    args.finish()?;
    run(&o)
}

fn run(o: &Opts) -> Result<()> {
    let started = Instant::now();
    util::forbid_synced(&o.out, "the output folder")?;
    util::forbid_synced(&o.target_dir, "the Cargo target folder")?;
    let tc = Toolchain::discover()?;
    let variant = if o.no_dwg { "no-dwg" } else { "full" };
    let root = o.out.join(variant);
    util::remove_tree(&root)?;
    fs::create_dir_all(&root)?;
    eprintln!("dist {VERSION} ({variant}) -> {}", root.display());

    // 1. the executable
    let exe = match &o.exe {
        Some(e) => e.clone(),
        None => build_app(&tc, o)?,
    };
    if !exe.is_file() {
        bail!("{} not found", exe.display());
    }
    let imports = pe::check_self_contained(&tc, &exe, o.crt == Crt::Static)?;
    eprintln!("imports of {}: {}", exe.file_name().unwrap_or_default().to_string_lossy(), imports.join(", "));
    let stamp = zip_time(&exe);
    let facts = build_facts(&tc, &exe, notices::Platform::Windows)?;

    // 2. pieces that are not Rust
    let work = root.join("work");
    let thumb = build_thumb_dll(&work)?;
    let dwg = if o.no_dwg { Vec::new() } else { dwg_files(&tc)? };
    let dwg_all = if o.no_dwg { dwg_files(&tc)? } else { dwg.clone() }; // the add-on needs them as well
    let notices_text = notices::generate(&tc, !o.no_dwg, notices::Platform::Windows)?;
    let prefix = occt::default_prefix_opt(o.crt, o.occt_opt);
    let licenses = license_files(&prefix, &notices_text)?;

    // 3. installer stage
    let stage = root.join("stage").join("StepViewer");
    fs::create_dir_all(&stage)?;
    util::copy_file(&exe, &stage.join("StepViewer.exe"))?;
    if let Some(t) = &thumb {
        util::copy_file(t, &stage.join("StepThumb.dll"))?;
    }
    put_licenses(&stage, &licenses)?;
    if o.no_dwg {
        // the installer copies it only where the converter is installed (a full installation or the add-on), see the
        // [Files] section of StepViewer.iss
        util::write_file(&stage.join("licenses").join("THIRD_PARTY_NOTICES_DWG.txt"), notices::dwg_addon_notice())?;
    }
    put_dwg(&stage, &dwg)?;

    // 4. portable folder + zip
    // the two variants must not share a zip name (the same file name on a shared Drive would overwrite the other)
    let pname = format!("StepViewer-portable-{VERSION}{}", variant_suffix(o.no_dwg));
    let pdir = root.join(&pname);
    fs::create_dir_all(&pdir)?;
    util::copy_file(&exe, &pdir.join("StepViewer.exe"))?;
    put_licenses(&pdir, &licenses)?;
    put_dwg(&pdir, &dwg)?;
    util::write_file(&pdir.join("portable.flag"), "CAD prohlížeč – přenosný režim (viz PRECTI_ME.txt)\r\n")?;
    util::write_file(&pdir.join("PRECTI_ME.txt"), portable_readme(!o.no_dwg))?;
    let portable_zip = root.join(format!("{pname}.zip"));
    write_zip(&portable_zip, &pname, &util::list_files(&pdir)?, stamp)?;

    // 5. the DWG add-on (base build only)
    let mut addon_zip = None;
    let mut addon_files: Vec<(PathBuf, String)> = Vec::new();
    if o.no_dwg {
        let adir = root.join("work").join("addon");
        util::remove_tree(&adir)?;
        put_dwg(&adir, &dwg_all)?;
        let lic = adir.join("licenses");
        util::copy_file(&repo_license("GPL-3.0-only.txt"), &lic.join("GPL-3.0-only.txt"))?;
        util::copy_file(&repo_license("LGPL-2.1-only.txt"), &lic.join("LGPL-2.1-only.txt"))?;
        util::write_file(&lic.join("THIRD_PARTY_NOTICES_DWG.txt"), notices::dwg_addon_notice())?;
        util::write_file(&adir.join("PRECTI_ME_DWG.txt"), addon_readme())?;
        addon_files = util::list_files(&adir)?;
        let z = root.join(format!("StepViewer-dwg-addon-{VERSION}.zip"));
        write_zip(&z, "", &addon_files, stamp)?;
        addon_zip = Some(z);
    }

    // 6. installer
    let mut installer = None;
    if o.no_installer {
        eprintln!("installer: skipped (--no-installer); the script is rust/xtask/installer/StepViewer.iss");
    } else {
        installer = compile_installer(&tc, &stage, &root, o.no_dwg)?;
    }

    // 7. size report
    let stage_files = util::list_files(&stage)?;
    let report = if o.no_sizes {
        String::new()
    } else {
        size_report(&root, &stage_files, &addon_files, &exe, &prefix, &[
            ("installer", installer.clone()),
            ("portable zip", Some(portable_zip.clone())),
            ("DWG add-on zip", addon_zip.clone()),
        ], o, &imports, &facts)?
    };
    if !report.is_empty() {
        util::write_file(&root.join("size-report.txt"), &report)?;
        println!("{report}");
    }

    // 8. checksums
    let mut sums = String::new();
    for f in [installer.as_ref(), Some(&portable_zip), addon_zip.as_ref()].into_iter().flatten() {
        sums.push_str(&format!("{}  {}\n", util::sha256_file(f)?, f.file_name().unwrap_or_default().to_string_lossy()));
    }
    util::write_file(&root.join("SHA256SUMS.txt"), sums)?;
    eprintln!("dist finished in {} s: {}", started.elapsed().as_secs(), root.display());
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// build

pub fn cargo(tc: &Toolchain) -> std::process::Command {
    let mut c = match std::env::var_os("CARGO").map(PathBuf::from).filter(|p| p.is_file()) {
        Some(p) => std::process::Command::new(p),
        None => tc.command("cargo"),
    };
    tc.apply(&mut c);
    c
}

/// Rust flags of the release build, `\x1f`-separated for CARGO_ENCODED_RUSTFLAGS (the repository path has spaces).
pub fn rustflags(crt: Crt) -> String {
    let mut f: Vec<String> = Vec::new();
    if crt == Crt::Static {
        f.push("-C".into());
        f.push("target-feature=+crt-static".into());
    }
    // deterministic PE header and no build-machine paths in panic messages
    f.push("-C".into());
    f.push("link-arg=/Brepro".into());
    f.extend(remap_path_flags());
    f.join("\x1f")
}

/// `--remap-path-prefix` of the repository (`repo`) and of the user's folder (`~`): no build-machine paths in the
/// executable (the C and C++ side is `/d1trimpath`, see `build_app`).
pub fn remap_path_flags() -> Vec<String> {
    vec![
        format!("--remap-path-prefix={}=repo", util::repo_root().display()),
        format!("--remap-path-prefix={}=~", util::home().display()),
    ]
}

fn profile_dir(profile: &str) -> &str {
    match profile {
        "dev" | "test" => "debug",
        "release" | "bench" => "release",
        other => other,
    }
}

fn build_app(tc: &Toolchain, o: &Opts) -> Result<PathBuf> {
    let exe = o.target_dir.join(WINDOWS_TARGET).join(profile_dir(&o.profile)).join("StepViewer.exe");
    if o.no_build {
        return Ok(exe);
    }
    let prefix = occt::default_prefix_opt(o.crt, o.occt_opt);
    if !prefix.join("occt-static.txt").is_file() {
        if o.build_occt {
            occt::build(&util::occt_root(), o.crt, o.occt_opt, occt::default_jobs(), false, false)?;
        } else {
            bail!(
                "static OCCT not found in {}: run `cargo xtask occt-static{}` (or pass --build-occt)",
                prefix.display(),
                if o.crt == Crt::Dynamic { " --crt dynamic" } else { "" }
            );
        }
    }
    let mut c = cargo(tc);
    c.current_dir(util::rust_dir())
        .arg("build")
        .args(["--profile", &o.profile, "--package", "cad-app", "--target", WINDOWS_TARGET])
        .arg("--target-dir")
        .arg(&o.target_dir)
        // works whether or not cad-app forwards `static-occt` itself
        .args(["--features", "occt-bridge/static-occt"])
        .env_remove("RUSTFLAGS")
        .env("CARGO_ENCODED_RUSTFLAGS", rustflags(o.crt))
        .env("OCCT_STATIC_PREFIX", &prefix);
    util::forward_drive_env(&mut c);
    // __FILE__ of the C++ code (cxx, occt-bridge) would carry the user's folder into the executable. `/d1trimfile` is the
    // option this MSVC (19.44) has; it goes to C++ only and needs a path without spaces.
    let home = util::home().display().to_string();
    if !home.contains(' ') {
        c.env("CXXFLAGS_x86_64_pc_windows_msvc", format!("/d1trimfile:{}\\", home.trim_end_matches('\\')));
    }
    util::run(&mut c)?;
    Ok(exe)
}

/// `-nodwg` in the names of the base variant (portable folder and zip, Linux archives).
fn variant_suffix(no_dwg: bool) -> &'static str {
    if no_dwg { "-nodwg" } else { "" }
}

/// What the executable really contains, checked after the build; recorded in the size report. The Drive client id is
/// only ever reported as yes or no, its value is never printed.
pub struct BuildFacts {
    /// Windows only (`None` on Linux): the executable has the version resource.
    pub version_resource: Option<bool>,
    pub fonts: Vec<&'static str>,
    /// `CADVIEWER_DRIVE_CLIENT_ID` is set in the environment of this run.
    pub drive_id_env: bool,
    /// The configured client id's bytes are in the executable (`None`: not configured).
    pub drive_id_in_exe: Option<bool>,
}

pub fn build_facts(tc: &Toolchain, exe: &Path, platform: notices::Platform) -> Result<BuildFacts> {
    let version_resource = match platform {
        notices::Platform::Windows => {
            if !pe::has_version_resource(exe)? {
                bail!(
                    "{} has no version resource (product name, version): rc.exe was not found when cad-app was built; \
                     check the build warnings and run the build from devenv.ps1",
                    exe.display()
                );
            }
            // without it, paths over 260 characters fail to load (the program reports them as unreadable)
            if !pe::has_long_path_manifest(exe)? {
                bail!(
                    "{} has no application manifest with longPathAware: rc.exe was not found when cad-app was built; \
                     check the build warnings and run the build from devenv.ps1",
                    exe.display()
                );
            }
            Some(true)
        }
        notices::Platform::Linux => None,
    };
    let fonts = notices::verify_fonts_in_exe(tc, exe, platform)?;
    let drive_id = std::env::var(util::DRIVE_CLIENT_ID_ENV).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let drive_id_in_exe = match &drive_id {
        Some(id) => Some(pe::contains_bytes(exe, id.as_bytes())?),
        None => None,
    };
    Ok(BuildFacts { version_resource, fonts, drive_id_env: drive_id.is_some(), drive_id_in_exe })
}

/// The Explorer thumbnail handler (.NET Framework 4 COM server), compiled with the csc.exe that ships with Windows,
/// like build.py did.
fn build_thumb_dll(work: &Path) -> Result<Option<PathBuf>> {
    let win = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let csc = win.join("Microsoft.NET").join("Framework64").join("v4.0.30319").join("csc.exe");
    if !csc.is_file() {
        eprintln!("warning: {} not found, the installer gets no StepThumb.dll", csc.display());
        return Ok(None);
    }
    fs::create_dir_all(work)?;
    let out = work.join("StepThumb.dll");
    let src = util::repo_root().join("shellext");
    let mut c = std::process::Command::new(csc);
    c.args(["/nologo", "/target:library", "/platform:x64", "/optimize"])
        .arg(format!("/out:{}", out.display()))
        .arg(src.join("StepThumb.cs"))
        .arg(src.join("Sandbox.cs"))
        .arg("/r:System.Drawing.dll");
    util::run(&mut c)?;
    Ok(Some(out))
}

// ---------------------------------------------------------------------------------------------------------------
// content

/// dwg2dxf.exe and the DLLs it really loads (computed from the import tables; 1.9 also shipped the two PCRE DLLs and
/// dwgbmp.exe, which nothing loads).
fn dwg_files(tc: &Toolchain) -> Result<Vec<(PathBuf, String)>> {
    let dir = util::repo_root().join("third_party").join("libredwg");
    let exe = dir.join("dwg2dxf.exe");
    if !exe.is_file() {
        bail!("{} not found", exe.display());
    }
    let (dlls, _) = pe::dll_closure(tc, &exe, &dir)?;
    let mut files = vec![(exe, "dwg2dxf.exe".to_string())];
    for d in dlls {
        files.push((dir.join(&d), d));
    }
    Ok(files)
}

fn repo_license(name: &str) -> PathBuf {
    util::rust_dir().join("xtask").join("installer").join("licenses").join(name)
}

/// (file name in `licenses/`, content source)
fn license_files(prefix: &Path, notices_text: &str) -> Result<Vec<(String, LicenseSrc)>> {
    let exception = prefix.join("share").join("doc").join("opencascade").join("OCCT_LGPL_EXCEPTION.txt");
    if !exception.is_file() {
        bail!("{} missing: the static OCCT in {} is incomplete, run `cargo xtask occt-static`", exception.display(), prefix.display());
    }
    Ok(vec![
        ("GPL-3.0-only.txt".into(), LicenseSrc::File(repo_license("GPL-3.0-only.txt"))),
        ("LGPL-2.1-only.txt".into(), LicenseSrc::File(repo_license("LGPL-2.1-only.txt"))),
        ("OCCT_LGPL_EXCEPTION.txt".into(), LicenseSrc::File(exception)),
        ("THIRD_PARTY_NOTICES.txt".into(), LicenseSrc::Text(notices_text.to_string())),
    ])
}

enum LicenseSrc {
    File(PathBuf),
    Text(String),
}

fn put_licenses(dir: &Path, lic: &[(String, LicenseSrc)]) -> Result<()> {
    for (name, src) in lic {
        let to = dir.join("licenses").join(name);
        match src {
            LicenseSrc::File(p) => util::copy_file(p, &to)?,
            LicenseSrc::Text(t) => util::write_file(&to, t.as_bytes())?,
        }
    }
    Ok(())
}

/// The converter goes to `third_party/libredwg` next to the executable, where `cad-2d` looks for it.
fn put_dwg(dir: &Path, files: &[(PathBuf, String)]) -> Result<()> {
    for (p, name) in files {
        util::copy_file(p, &dir.join("third_party").join("libredwg").join(name))?;
    }
    Ok(())
}

fn portable_readme(with_dwg: bool) -> String {
    let dwg = if with_dwg {
        "Výkresy DWG: převodník LibreDWG je ve složce third_party\\libredwg.\r\n"
    } else {
        "Výkresy DWG: tento balíček neobsahuje převodník. Doplněk StepViewer-dwg-addon-*.zip rozbalte do této složky\r\n(vznikne third_party\\libredwg); bez něj se soubory DWG neotevřou, DXF a 3D modely fungují.\r\n"
    };
    format!(
        "CAD prohlížeč {VERSION} – přenosná verze (např. složka na Google Drive)\r\n\
         ====================================================================\r\n\r\n\
         Spuštění: StepViewer.exe. Žádná instalace, žádné zápisy do registru, nejsou potřeba práva správce.\r\n\r\n\
         Soubor portable.flag vedle StepViewer.exe zapíná přenosný režim:\r\n\
         - složka programu se považuje za jen pro čtení; nastavení, mezipaměť, protokoly a dočasné soubory jsou vždy\r\n\
         \x20 v místních složkách uživatele (%APPDATA%\\StepViewer, %LOCALAPPDATA%\\StepViewer), nikdy ve složce, kterou\r\n\
         \x20 synchronizuje Disk Google;\r\n\
         - modely a výkresy otevřené z Disku se nejdřív zkopírují do místní mezipaměti; zdrojové soubory program nikdy\r\n\
         \x20 nemění ani vedle nich nic nezapisuje;\r\n\
         - přenosná verze neregistruje přípony souborů ani náhledy v Průzkumníku. Odkazy z Disku Google („Otevřít v aplikaci“)\r\n\
         \x20 lze zapnout na vyžádání příkazem StepViewer.exe --register-link (zapisuje jen do HKCU\\Software\\Classes\\cadviewer,\r\n\
         \x20 vypnutí: --unregister-link).\r\n\r\n\
         {dwg}\r\n\
         Licence a oznámení: složka licenses. Tento program používá a je založen na technologii Open CASCADE Technology\r\n\
         (LGPL 2.1 s výjimkou Open CASCADE). Program je svobodný software, licence GPL-3.0; zdrojový kód viz\r\n\
         licenses\\THIRD_PARTY_NOTICES.txt.\r\n"
    )
}

fn addon_readme() -> String {
    format!(
        "CAD prohlížeč {VERSION} – převodník DWG (doplněk)\r\n\
         ===========================================\r\n\r\n\
         Obsah: složka third_party\\libredwg (dwg2dxf.exe z LibreDWG a knihovny, které potřebuje) a licenses\\.\r\n\r\n\
         Instalace: rozbalte celý archiv do složky programu (vedle StepViewer.exe), tedy do složky, kde už je\r\n\
         StepViewer.exe – instalované verze (výchozí C:\\Program Files\\StepViewer nebo %LOCALAPPDATA%\\Programs\\StepViewer)\r\n\
         i přenosné verze. Složky third_party a licenses se doplní; texty GPL-3.0-only.txt a LGPL-2.1-only.txt už ve složce\r\n\
         licenses jsou, archiv obsahuje totožné kopie (přepsání nevadí). Do C:\\Program Files je potřeba právo správce.\r\n\
         Odinstalace: smažte složku third_party\\libredwg.\r\n\r\n\
         LibreDWG je samostatný program (GPL-3.0 nebo novější), který CAD prohlížeč spouští jako externí proces;\r\n\
         podrobnosti jsou v licenses\\THIRD_PARTY_NOTICES_DWG.txt.\r\n"
    )
}

// ---------------------------------------------------------------------------------------------------------------
// archives

fn zip_time(reference: &Path) -> zip::DateTime {
    let secs = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .or_else(|| {
            fs::metadata(reference)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
        })
        .or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs()))
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = civil(secs);
    zip::DateTime::from_date_and_time(y, mo, d, h, mi, s).unwrap_or_default()
}

/// Unix seconds -> (year, month, day, hour, minute, second), UTC (Hinnant's days-to-civil).
fn civil(secs: u64) -> (u16, u8, u8, u8, u8, u8) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let y = (yoe + era * 400 + if m <= 2 { 1 } else { 0 }) as u16;
    (y, m, d, (rem / 3600) as u8, (rem % 3600 / 60) as u8, (rem % 60) as u8)
}

/// Deflate level 9 zip; entries below `top/` (no folder when `top` is empty), sorted, one timestamp.
fn write_zip(path: &Path, top: &str, files: &[(PathBuf, String)], stamp: zip::DateTime) -> Result<()> {
    let f = fs::File::create(path).with_context(|| path.display().to_string())?;
    let mut z = zip::ZipWriter::new(std::io::BufWriter::new(f));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(9))
        .last_modified_time(stamp)
        .unix_permissions(0o644);
    for (p, rel) in files {
        let name = if top.is_empty() { rel.clone() } else { format!("{top}/{rel}") };
        z.start_file(name, opts)?;
        let mut src = fs::File::open(p).with_context(|| p.display().to_string())?;
        std::io::copy(&mut src, &mut z)?;
    }
    z.finish()?.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// installer

/// ISCC.exe without admin rights: `ISCC` variable, the per-user Inno Setup 6, the machine-wide one, PATH.
pub fn find_iscc(tc: &Toolchain) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("ISCC") {
        candidates.push(PathBuf::from(p));
    }
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(l).join("Programs").join("Inno Setup 6").join("ISCC.exe"));
    }
    for v in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(p) = std::env::var_os(v) {
            candidates.push(PathBuf::from(p).join("Inno Setup 6").join("ISCC.exe"));
        }
    }
    candidates.into_iter().find(|p| p.is_file()).or_else(|| tc.find("ISCC"))
}

fn compile_installer(tc: &Toolchain, stage: &Path, out: &Path, no_dwg: bool) -> Result<Option<PathBuf>> {
    let iss = util::rust_dir().join("xtask").join("installer").join("StepViewer.iss");
    let Some(iscc) = find_iscc(tc) else {
        eprintln!(
            "installer: ISCC.exe not found (looked in %LOCALAPPDATA%\\Programs\\Inno Setup 6 and Program Files). \
             The ready script is {}; compile it with: ISCC.exe /O<dir> /DStageDir={} /DRepoDir={}",
            iss.display(),
            stage.display(),
            util::repo_root().display()
        );
        return Ok(None);
    };
    let mut c = std::process::Command::new(&iscc);
    c.arg("/Q")
        .arg(format!("/O{}", out.display()))
        .arg(format!("/DAppVersion={VERSION}"))
        .arg(format!("/DStageDir={}", stage.display()))
        .arg(format!("/DRepoDir={}", util::repo_root().display()));
    if no_dwg {
        c.arg("/DNoDwg=1");
    }
    c.arg(&iss);
    util::run(&mut c)?;
    let name = format!("CadProhlizec_setup_{VERSION}{}.exe", if no_dwg { "_nodwg" } else { "" });
    let p = out.join(name);
    if !p.is_file() {
        bail!("ISCC finished but {} was not written", p.display());
    }
    Ok(Some(p))
}

// ---------------------------------------------------------------------------------------------------------------
// report

fn yes_no(b: bool) -> String {
    if b { "yes" } else { "no" }.to_string()
}

/// The "Build checks" block of the size report. The Drive client id is reported as yes or no only.
pub fn build_checks_text(facts: &BuildFacts) -> String {
    let mut s = String::from("\nBuild checks:\n");
    if let Some(v) = facts.version_resource {
        s.push_str(&format!("  version resource (product CAD prohlížeč, version {VERSION}): {}\n", yes_no(v)));
    }
    s.push_str(&format!(
        "  embedded fonts found in the executable: {} of {} ({})\n",
        facts.fonts.len(),
        notices::EMBEDDED_FONTS.len(),
        facts.fonts.join("; ")
    ));
    s.push_str(&format!(
        "  Google Drive client id (CADVIEWER_DRIVE_CLIENT_ID) set in the build environment: {}\n",
        yes_no(facts.drive_id_env)
    ));
    s.push_str(&format!(
        "  Google Drive client id compiled into the executable: {}\n",
        facts.drive_id_in_exe.map_or("not configured (Drive sign-in is off in this build)".to_string(), yes_no)
    ));
    s
}

fn component_of(rel: &str) -> &'static str {
    if rel == "StepViewer.exe" {
        "StepViewer.exe"
    } else if rel == "StepThumb.dll" {
        "StepThumb.dll"
    } else if rel.starts_with("third_party/libredwg/") {
        "DWG converter"
    } else if rel.starts_with("licenses/") {
        "licenses"
    } else {
        "other"
    }
}

#[allow(clippy::too_many_arguments)]
fn size_report(
    root: &Path,
    stage_files: &[(PathBuf, String)],
    addon_files: &[(PathBuf, String)],
    exe: &Path,
    prefix: &Path,
    artifacts: &[(&str, Option<PathBuf>)],
    o: &Opts,
    imports: &[String],
    facts: &BuildFacts,
) -> Result<String> {
    let note = |n: &str| match n {
        "StepViewer.exe" => "app + egui/wgpu + OCCT (static) + C runtime",
        "StepThumb.dll" => "Explorer thumbnails (.NET COM server)",
        "DWG converter" => "dwg2dxf.exe + libredwg-0.dll + libiconv-2.dll",
        "licenses" => "licence texts, third-party notices",
        _ => "",
    };
    let mut comps: Vec<Component> = Vec::new();
    for name in ["StepViewer.exe", "StepThumb.dll", "DWG converter", "licenses", "other"] {
        let files: Vec<_> = stage_files.iter().filter(|(_, r)| component_of(r) == name).cloned().collect();
        if !files.is_empty() {
            comps.push(Component { name: name.to_string(), note: note(name).to_string(), files });
        }
    }
    let rows = sizes::measure(&comps)?;
    let total = sizes::measure_total(&comps)?;

    let mut s = String::new();
    s.push_str(&format!(
        "CAD prohlížeč {VERSION} – size report ({})  profile `{}`, CRT {}\n\n",
        if o.no_dwg { "base without the DWG converter" } else { "full, with the DWG converter" },
        o.profile,
        o.crt.name()
    ));
    s.push_str("Installer content (xz = LZMA2 preset 9e, one solid stream per row; the installer adds Inno's own overhead):\n");
    s.push_str(&sizes::render(&rows, &total));
    for r in &rows {
        if !r.note.is_empty() {
            s.push_str(&format!("  {:<18} {}\n", r.name, r.note));
        }
    }
    if !addon_files.is_empty() {
        let addon = sizes::measure(&[Component { name: "DWG add-on (separate download)".into(), note: String::new(), files: addon_files.to_vec() }])?;
        s.push_str("\nSeparate add-on (not in the figures above):\n");
        s.push_str(&sizes::render(&[], &addon[0]));
    }
    s.push_str(&format!("\nStepViewer.exe imports: {}\n", imports.join(", ")));
    s.push_str(&build_checks_text(facts));
    if let Ok(tk) = occt::toolkit_sizes(prefix) {
        let sum: u64 = tk.iter().map(|(_, b)| b).sum();
        s.push_str(&format!(
            "Static OCCT libraries linked from (build input, not shipped): {} files, {} in {}\n",
            tk.len(),
            util::mb(sum),
            prefix.join("lib").display()
        ));
    }
    s.push_str("\nArtifacts:\n");
    let exe_size = fs::metadata(exe).map(|m| m.len()).unwrap_or(0);
    s.push_str(&format!("  {:<22} {:>14} bytes  {}\n", "StepViewer.exe", sizes::group(exe_size), util::mb(exe_size)));
    for (label, path) in artifacts {
        if let Some(p) = path {
            let n = fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            let vs = if *label == "installer" {
                format!("  ({:+.1}% vs 1.9 installer {})", 100.0 * (n as f64 / BASELINE_INSTALLER_1_9 as f64 - 1.0), util::mb(BASELINE_INSTALLER_1_9))
            } else {
                String::new()
            };
            s.push_str(&format!("  {:<22} {:>14} bytes  {}{}\n", label, sizes::group(n), util::mb(n), vs));
        }
    }
    s.push_str(&format!(
        "\nBaseline 1.9: Windows installer {}, Linux AppImage {}.\n",
        util::mb(BASELINE_INSTALLER_1_9),
        util::mb(BASELINE_APPIMAGE_1_9)
    ));

    let json = serde_json::json!({
        "version": VERSION,
        "variant": if o.no_dwg { "no-dwg" } else { "full" },
        "profile": o.profile,
        "crt": o.crt.name(),
        "components": rows,
        "total": total,
        "artifacts": artifacts.iter().filter_map(|(l, p)| {
            let p = p.as_ref()?;
            Some(serde_json::json!({ "name": l, "file": p.file_name()?.to_string_lossy(), "bytes": fs::metadata(p).ok()?.len() }))
        }).collect::<Vec<_>>(),
        "baseline_1_9": { "installer_bytes": BASELINE_INSTALLER_1_9, "appimage_bytes": BASELINE_APPIMAGE_1_9 },
        "build_checks": {
            "version_resource": facts.version_resource,
            "fonts_in_exe": facts.fonts,
            "drive_client_id_configured": facts.drive_id_env,
            "drive_client_id_in_exe": facts.drive_id_in_exe,
        },
    });
    util::write_file(&root.join("size-report.json"), serde_json::to_vec_pretty(&json)?)?;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil(1_700_000_000), (2023, 11, 14, 22, 13, 20));
        assert_eq!(civil(951_782_400), (2000, 2, 29, 0, 0, 0)); // leap day
    }

    #[test]
    fn components_by_path() {
        assert_eq!(component_of("StepViewer.exe"), "StepViewer.exe");
        assert_eq!(component_of("third_party/libredwg/dwg2dxf.exe"), "DWG converter");
        assert_eq!(component_of("licenses/GPL-3.0-only.txt"), "licenses");
        assert_eq!(component_of("portable.flag"), "other");
    }

    #[test]
    fn profile_folders() {
        assert_eq!(profile_dir("dist"), "dist");
        assert_eq!(profile_dir("dev"), "debug");
        assert_eq!(profile_dir("release"), "release");
    }

    #[test]
    fn rustflags_keep_paths_with_spaces_in_one_flag() {
        let f = rustflags(Crt::Static);
        assert!(f.split('\x1f').any(|x| x == "target-feature=+crt-static"));
        assert!(f.split('\x1f').any(|x| x == "link-arg=/Brepro"));
        assert!(f.split('\x1f').filter(|x| x.starts_with("--remap-path-prefix=")).count() == 2);
        assert!(!rustflags(Crt::Dynamic).contains("crt-static"));
    }
}
