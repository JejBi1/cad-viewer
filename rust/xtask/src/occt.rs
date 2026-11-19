//! `cargo xtask occt-static`: builds Open CASCADE Technology 8.0.1 from the official source archive as static
//! libraries, with only the toolkits `occt-bridge` needs and no optional third-party library.
//!
//! Everything lives outside the repository, by default in `~/.local/occt-static`:
//! `dl/` (the pinned archive), `src/OCCT-8_0_1/` (extracted source), `build-<flavour>/` (CMake + Ninja) and
//! `prefix-<flavour>/` (install: `include/opencascade`, `lib/TK*.lib`, `occt-static.txt`, `share/doc/...`).
//! The flavour is `mt` (static CRT, /MT) or `md` (dynamic CRT, /MD) on Windows and `linux` elsewhere; the flavour
//! must match the CRT of the Rust build (`-C target-feature=+crt-static`), `occt-bridge/build.rs` checks that.
use crate::util::{self, Args, Toolchain};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const OCCT_VERSION: &str = "8.0.1";
pub const ARCHIVE_URL: &str = "https://github.com/Open-Cascade-SAS/OCCT/archive/refs/tags/V8_0_1.tar.gz";
pub const ARCHIVE_NAME: &str = "OCCT-V8_0_1.tar.gz";
/// SHA-256 of the archive (the same value the conda-forge recipe of occt 8.0.1 pins).
pub const ARCHIVE_SHA256: &str = "0d6913eae4bcc09a3653ceced6dda1aec11c35a1513d4c06762c9b002092c68a";
pub const ARCHIVE_BYTES: u64 = 45_130_339;
/// Commit the tag V8_0_1 points to (informational; the archive hash is what is enforced).
pub const SOURCE_COMMIT: &str = "b8f597c677811d1f9f4d8a97f5ae2825c0353a42";
const SRC_NAME: &str = "OCCT-8_0_1";

/// Toolkits `occt-bridge` calls directly (STEP / IGES / STL readers, XCAF, binary XCAF documents, Boolean common,
/// meshing, shape processing). CMake adds every toolkit they depend on (TKService, TKV3d and TKHLR come in through
/// TKXCAF); nothing else is built: no TKOpenGl, no Draw, no TKRWMesh / glTF, no TKXMesh, no TKFillet / TKOffset.
pub const ROOT_TOOLKITS: &[&str] = &[
    "TKDESTEP", "TKDEIGES", "TKDESTL", "TKBinXCAF", "TKBO", "TKMesh", "TKXCAF", "TKXSBase",
];

const MODULES: &[&str] = &[
    "FoundationClasses",
    "ModelingData",
    "ModelingAlgorithms",
    "Visualization",
    "ApplicationFramework",
    "DataExchange",
    "Draw",
];

/// Which C runtime the libraries are compiled for (Windows only).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Crt {
    /// `/MT`: the CRT is linked into the executable, no vcruntime140.dll / msvcp140.dll needed.
    Static,
    /// `/MD`: the Visual C++ redistributable DLLs are needed.
    Dynamic,
}

impl Crt {
    pub fn parse(s: &str) -> Result<Crt> {
        match s {
            "static" | "mt" => Ok(Crt::Static),
            "dynamic" | "md" => Ok(Crt::Dynamic),
            _ => bail!("--crt must be `static` or `dynamic`"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Crt::Static => "static",
            Crt::Dynamic => "dynamic",
        }
    }
}

/// What the OCCT code is optimised for. `Speed` (/O2) is the default; `Size` (/O1, MinSizeRel) is an experiment that
/// trades some speed of the geometry kernel for a smaller executable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Opt {
    Speed,
    Size,
}

impl Opt {
    pub fn parse(s: &str) -> Result<Opt> {
        match s {
            "speed" => Ok(Opt::Speed),
            "size" => Ok(Opt::Size),
            _ => bail!("--occt-opt / --opt must be `speed` or `size`"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Opt::Speed => "speed",
            Opt::Size => "size",
        }
    }
}

/// Folders of one flavour below the OCCT root.
pub struct Layout {
    pub root: PathBuf,
    pub download: PathBuf,
    pub archive: PathBuf,
    pub src_parent: PathBuf,
    pub src: PathBuf,
    pub build: PathBuf,
    pub prefix: PathBuf,
}

pub fn layout(root: &Path, crt: Crt, opt: Opt) -> Layout {
    let mut flavour = String::from(if cfg!(windows) {
        match crt {
            Crt::Static => "mt",
            Crt::Dynamic => "md",
        }
    } else {
        "linux"
    });
    if opt == Opt::Size {
        flavour.push_str("-os");
    }
    Layout {
        root: root.to_path_buf(),
        download: root.join("dl"),
        archive: root.join("dl").join(ARCHIVE_NAME),
        src_parent: root.join("src"),
        src: root.join("src").join(SRC_NAME),
        build: root.join(format!("build-{flavour}")),
        prefix: root.join(format!("prefix-{flavour}")),
    }
}

/// Install prefix of the static libraries for `crt` (default root), the value for `OCCT_STATIC_PREFIX`.
pub fn default_prefix(crt: Crt) -> PathBuf {
    default_prefix_opt(crt, Opt::Speed)
}

pub fn default_prefix_opt(crt: Crt, opt: Opt) -> PathBuf {
    layout(&util::occt_root(), crt, opt).prefix
}

/// `cargo xtask occt-static [--crt static|dynamic] [--jobs N] [--root DIR] [--reconfigure] [--clean]`
pub fn command(mut args: Args) -> Result<()> {
    let crt = match args.value("--crt")? {
        Some(v) => Crt::parse(&v)?,
        None => Crt::Static,
    };
    let root = args.value("--root")?.map(PathBuf::from).unwrap_or_else(util::occt_root);
    let jobs = match args.value("--jobs")? {
        Some(v) => v.parse::<usize>().context("--jobs")?.max(1),
        None => default_jobs(),
    };
    let opt = match args.value("--opt")? {
        Some(v) => Opt::parse(&v)?,
        None => Opt::Speed,
    };
    let reconfigure = args.flag("--reconfigure");
    let clean = args.flag("--clean");
    args.finish()?;
    build(&root, crt, opt, jobs, reconfigure, clean)
}

pub fn default_jobs() -> usize {
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    (n * 3 / 4).max(2)
}

pub fn build(root: &Path, crt: Crt, opt: Opt, jobs: usize, reconfigure: bool, clean: bool) -> Result<()> {
    util::forbid_synced(root, "the OCCT build folder")?;
    let mut tc = Toolchain::discover()?;
    let lay = layout(root, crt, opt);
    let started = Instant::now();
    supply_missing_headers(&lay, &mut tc)?;
    eprintln!("OCCT {OCCT_VERSION} static build, CRT {}, root {}", if cfg!(windows) { crt.name() } else { "system" }, root.display());

    fetch(&lay)?;
    extract(&lay)?;
    if clean {
        util::remove_tree(&lay.build)?;
        util::remove_tree(&lay.prefix)?;
    }
    configure(&lay, &tc, crt, opt, reconfigure)?;

    let mut b = tc.command("cmake");
    b.arg("--build").arg(&lay.build).arg("--parallel").arg(jobs.to_string());
    util::run(&mut b)?;

    // a fresh prefix every time: no stale libraries from an earlier toolkit list
    util::remove_tree(&lay.prefix)?;
    let mut i = tc.command("cmake");
    i.arg("--install").arg(&lay.build);
    util::run(&mut i)?;

    write_manifest(&lay, crt, opt)?;
    let (n, bytes) = lib_totals(&lay.prefix)?;
    eprintln!(
        "OCCT {OCCT_VERSION}: {n} static libraries, {} in {} ({}s)",
        util::mb(bytes),
        lay.prefix.join("lib").display(),
        started.elapsed().as_secs()
    );
    Ok(())
}

/// The portable Windows SDK of this machine (`~/.local/msvc-portable`) lacks `lmcons.h`, which OCCT's OSD_Process.cxx
/// includes only for `UNLEN`. A stand-in with that one macro is written next to the build and appended to INCLUDE
/// after the SDK folders, so a complete SDK keeps using its own header.
fn supply_missing_headers(lay: &Layout, tc: &mut Toolchain) -> Result<()> {
    if !cfg!(windows) || tc.include_dirs().iter().any(|d| d.join("lmcons.h").is_file()) {
        return Ok(());
    }
    let dir = lay.root.join("shim").join("include");
    util::write_file(
        &dir.join("lmcons.h"),
        "/* Stand-in for the Windows SDK header lmcons.h, missing from the portable SDK. OCCT needs only UNLEN,
   the maximum user name length (256 in the SDK header). Appended to INCLUDE after the SDK folders. */
         #pragma once
#ifndef UNLEN
#define UNLEN 256
#endif
",
    )?;
    eprintln!("note: lmcons.h is not in the Windows SDK of this toolchain; using the stand-in {}", dir.display());
    tc.add_include(dir);
    Ok(())
}

/// Downloads the archive unless a verified copy is there.
fn fetch(lay: &Layout) -> Result<()> {
    if lay.archive.is_file() && verify_archive(&lay.archive).is_ok() {
        eprintln!("archive: {} (SHA-256 verified)", lay.archive.display());
        return Ok(());
    }
    fs::create_dir_all(&lay.download)?;
    let tmp = lay.download.join(format!("{ARCHIVE_NAME}.part"));
    let _ = fs::remove_file(&tmp);
    let curl = system_tool("curl");
    let mut c = std::process::Command::new(curl);
    c.args(["-fsSL", "--retry", "3", "--connect-timeout", "30", "--max-time", "1800", "-o"]).arg(&tmp).arg(ARCHIVE_URL);
    util::run(&mut c)?;
    if let Err(e) = verify_archive(&tmp) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, &lay.archive)?;
    eprintln!("downloaded {} ({})", ARCHIVE_URL, lay.archive.display());
    Ok(())
}

/// The size and the SHA-256 must match the pinned values.
pub fn verify_archive(path: &Path) -> Result<()> {
    let size = fs::metadata(path)?.len();
    if size != ARCHIVE_BYTES {
        bail!("{}: size {size}, expected {ARCHIVE_BYTES}", path.display());
    }
    let sum = util::sha256_file(path)?;
    if sum != ARCHIVE_SHA256 {
        bail!("{}: SHA-256 {sum}, expected {ARCHIVE_SHA256}", path.display());
    }
    Ok(())
}

/// The OS tar / curl (bsdtar and curl ship with Windows 10+); the Git for Windows `tar` on PATH cannot read `C:\` paths.
fn system_tool(name: &str) -> PathBuf {
    if cfg!(windows) {
        let sys = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let p = sys.join("System32").join(format!("{name}.exe"));
        if p.is_file() {
            return p;
        }
    }
    PathBuf::from(name)
}

fn extract(lay: &Layout) -> Result<()> {
    let marker = lay.src_parent.join(format!(".extracted-{}", &ARCHIVE_SHA256[..16]));
    if marker.is_file() && lay.src.join("CMakeLists.txt").is_file() {
        eprintln!("source: {} (extracted)", lay.src.display());
        return Ok(());
    }
    eprintln!("extracting {} ...", lay.archive.display());
    util::remove_tree(&lay.src)?;
    fs::create_dir_all(&lay.src_parent)?;
    let mut t = std::process::Command::new(system_tool("tar"));
    t.arg("-xzf").arg(&lay.archive).arg("-C").arg(&lay.src_parent);
    util::run(&mut t)?;
    if !lay.src.join("CMakeLists.txt").is_file() || !lay.src.join("OCCT_LGPL_EXCEPTION.txt").is_file() {
        bail!("{} does not look like the OCCT source tree", lay.src.display());
    }
    util::write_file(&marker, format!("{ARCHIVE_SHA256}\n{SOURCE_COMMIT}\n"))?;
    Ok(())
}

/// CMake arguments (everything except source / build folder). Public so the notes in the report can show them.
pub fn cmake_args(lay: &Layout, crt: Crt, opt: Opt, ninja: Option<&Path>) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-G".into(),
        "Ninja".into(),
        "-Wno-author".into(),
        "-DCMAKE_BUILD_TYPE=Release".into(),
        format!("-DCMAKE_INSTALL_PREFIX={}", lay.prefix.display()),
        "-DINSTALL_DIR_LAYOUT=Unix".into(),
        "-DBUILD_LIBRARY_TYPE=Static".into(),
        format!("-DBUILD_ADDITIONAL_TOOLKITS={}", ROOT_TOOLKITS.join(";")),
        // optional third-party libraries and everything that is not a library
        "-DUSE_FREETYPE=OFF".into(),
        "-DUSE_FREEIMAGE=OFF".into(),
        "-DUSE_TBB=OFF".into(),
        "-DUSE_RAPIDJSON=OFF".into(),
        "-DUSE_DRACO=OFF".into(),
        "-DUSE_VTK=OFF".into(),
        "-DUSE_OPENGL=OFF".into(),
        "-DUSE_GLES2=OFF".into(),
        "-DUSE_TK=OFF".into(),
        "-DUSE_FFMPEG=OFF".into(),
        "-DUSE_OPENVR=OFF".into(),
        "-DUSE_EIGEN=OFF".into(),
        "-DBUILD_DOC_Overview=OFF".into(),
        "-DBUILD_DOC_RefMan=OFF".into(),
        "-DINSTALL_DOC_RefMan=OFF".into(),
        "-DBUILD_GTEST=OFF".into(),
        "-DINSTALL_TEST_CASES=OFF".into(),
        "-DBUILD_USE_PCH=OFF".into(),
    ];
    for m in MODULES {
        a.push(format!("-DBUILD_MODULE_{m}=OFF"));
    }
    if let Some(n) = ninja {
        a.push(format!("-DCMAKE_MAKE_PROGRAM={}", n.display()));
    }
    if cfg!(windows) {
        // /Gy /Gw /GF: function and data sections + string pooling, so the final link drops what the program never calls
        let rt = match crt {
            Crt::Static => "/MT",
            Crt::Dynamic => "/MD",
        };
        let optimise = match opt {
            Opt::Speed => "/O2 /Ob2",
            Opt::Size => "/O1 /Ob1",
        };
        let mut flags = format!("{rt} {optimise} /DNDEBUG /Gy /Gw /GF");
        // __FILE__ in OCCT messages and assertions would otherwise carry the build folder (and the user name) into the
        // executable; the compiler accepts both slash styles of the include paths
        let src = lay.src.display().to_string();
        if !src.contains(' ') {
            flags.push_str(&format!(" /d1trimfile:{src}\\ /d1trimfile:{}/", src.replace('\\', "/")));
        }
        a.push("-DCMAKE_C_COMPILER=cl".into());
        a.push("-DCMAKE_CXX_COMPILER=cl".into());
        a.push(format!("-DCMAKE_C_FLAGS_RELEASE={flags}"));
        a.push(format!("-DCMAKE_CXX_FLAGS_RELEASE={flags}"));
    } else {
        a.push("-DUSE_XLIB=OFF".into());
        let flags = format!("{} -DNDEBUG -ffunction-sections -fdata-sections", if opt == Opt::Size { "-Os" } else { "-O2" });
        a.push(format!("-DCMAKE_C_FLAGS_RELEASE={flags}"));
        a.push(format!("-DCMAKE_CXX_FLAGS_RELEASE={flags}"));
    }
    a
}

fn configure(lay: &Layout, tc: &Toolchain, crt: Crt, opt: Opt, force: bool) -> Result<()> {
    let ninja = tc.require("ninja")?;
    tc.require("cmake")?;
    let args = cmake_args(lay, crt, opt, Some(&ninja));
    let stamp_path = lay.build.join("xtask-configure.stamp");
    // warning switches do not change the result: they stay out of the stamp
    let stamped: Vec<&str> = args.iter().map(String::as_str).filter(|a| !a.starts_with("-W")).collect();
    let stamp = format!("{ARCHIVE_SHA256}\n{}\n", stamped.join("\n"));
    let stored = fs::read_to_string(&stamp_path).ok().map(|t| {
        // stamps written before the warning switches were left out of it still match
        t.lines().filter(|l| !l.starts_with("-W")).collect::<Vec<_>>().join("\n") + "\n"
    });
    if !force && lay.build.join("CMakeCache.txt").is_file() && stored.as_deref() == Some(stamp.as_str()) {
        eprintln!("configure: unchanged ({})", lay.build.display());
        return Ok(());
    }
    if !force && lay.build.join("CMakeCache.txt").is_file() {
        // different options than the cached ones: start from an empty folder, stale cache entries would stick
        util::remove_tree(&lay.build)?;
    }
    fs::create_dir_all(&lay.build)?;
    let mut c = tc.command("cmake");
    c.arg("-S").arg(&lay.src).arg("-B").arg(&lay.build).args(&args);
    util::run(&mut c)?;
    util::write_file(&stamp_path, stamp)?;
    Ok(())
}

/// Toolkit -> toolkits it links against, from `OCCT_<tk>_EXTERNAL_LIBS` in the EXTERNLIB.cmake files, plus the
/// system libraries the toolkit asks for (`CSF_*`).
type ToolkitDeps = (BTreeMap<String, Vec<String>>, BTreeSet<String>);

fn read_externlibs(src: &Path, wanted: &BTreeSet<String>) -> Result<ToolkitDeps> {
    let mut deps = BTreeMap::new();
    let mut csf = BTreeSet::new();
    for module in fs::read_dir(src.join("src"))? {
        let module = module?.path();
        if !module.is_dir() {
            continue;
        }
        for tk in fs::read_dir(&module)? {
            let tk = tk?.path();
            let name = tk.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if !wanted.contains(&name) {
                continue;
            }
            let text = fs::read_to_string(tk.join("EXTERNLIB.cmake")).unwrap_or_default();
            let mut list = Vec::new();
            for word in text.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
                if word.starts_with("TK") && word != name && word != format!("OCCT_{name}_EXTERNAL_LIBS") {
                    list.push(word.to_string());
                } else if word.starts_with("CSF_") {
                    csf.insert(word.to_string());
                }
            }
            list.sort();
            list.dedup();
            deps.insert(name, list);
        }
    }
    Ok((deps, csf))
}

/// Dependents first (a toolkit before the toolkits it uses), the order a GNU linker needs for static archives.
fn dependents_first(deps: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    fn visit(n: &str, deps: &BTreeMap<String, Vec<String>>, seen: &mut BTreeSet<String>, out: &mut Vec<String>) {
        if !seen.insert(n.to_string()) {
            return;
        }
        if let Some(ds) = deps.get(n) {
            for d in ds {
                visit(d, deps, seen, out);
            }
        }
        out.push(n.to_string());
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for n in deps.keys() {
        visit(n, deps, &mut seen, &mut out);
    }
    out.reverse();
    out
}

fn write_manifest(lay: &Layout, crt: Crt, opt: Opt) -> Result<()> {
    let lib = lay.prefix.join("lib");
    let ext = if cfg!(windows) { "lib" } else { "a" };
    let mut built = BTreeSet::new();
    for e in fs::read_dir(&lib).with_context(|| format!("reading {}", lib.display()))? {
        let p = e?.path();
        if p.extension().and_then(|e| e.to_str()) == Some(ext) {
            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let stem = stem.strip_prefix("lib").filter(|_| !cfg!(windows)).unwrap_or(&stem).to_string();
            if stem.starts_with("TK") {
                built.insert(stem);
            }
        }
    }
    for root in ROOT_TOOLKITS {
        if !built.contains(*root) {
            bail!("toolkit {root} was not built (no {} in {})", root, lib.display());
        }
    }
    let (deps, csf) = read_externlibs(&lay.src, &built)?;
    let order = dependents_first(&deps);
    let missing: Vec<_> = built.iter().filter(|b| !order.contains(b)).collect();
    if !missing.is_empty() {
        bail!("toolkits without a dependency list: {missing:?}");
    }
    // system libraries the built toolkits ask for (CSF_* names -> import libraries / -l names)
    let mut sys: BTreeSet<&str> = BTreeSet::new();
    for c in &csf {
        let mapped = if cfg!(windows) {
            match c.as_str() {
                "CSF_advapi32" => Some("advapi32"),
                "CSF_gdi32" => Some("gdi32"),
                "CSF_user32" => Some("user32"),
                "CSF_wsock32" => Some("wsock32"),
                "CSF_winmm" => Some("winmm"),
                "CSF_psapi" => Some("psapi"),
                _ => None,
            }
        } else {
            match c.as_str() {
                "CSF_ThreadLibs" => Some("pthread"),
                "CSF_dl" => Some("dl"),
                _ => None,
            }
        };
        if let Some(m) = mapped {
            sys.insert(m);
        }
    }
    let version = fs::read_to_string(lay.prefix.join("include").join("opencascade").join("Standard_Version.hxx"))
        .context("Standard_Version.hxx was not installed")?;
    if !version.contains(&format!("\"{OCCT_VERSION}\"")) {
        bail!("installed headers are not OCCT {OCCT_VERSION}");
    }
    // licence texts of the library, shipped with every distribution that contains it
    let doc = lay.prefix.join("share").join("doc").join("opencascade");
    for f in ["LICENSE_LGPL_21.txt", "OCCT_LGPL_EXCEPTION.txt"] {
        util::copy_file(&lay.src.join(f), &doc.join(f))?;
    }
    let text = format!(
        "format=1\nocct_version={OCCT_VERSION}\nsource_url={ARCHIVE_URL}\nsource_sha256={ARCHIVE_SHA256}\nsource_commit={SOURCE_COMMIT}\n\
         crt={}\nopt={}\nroots={}\ntoolkits={}\nsystem_libs={}\n",
        if cfg!(windows) { crt.name() } else { "system" },
        opt.name(),
        ROOT_TOOLKITS.join(","),
        order.join(","),
        sys.iter().copied().collect::<Vec<_>>().join(",")
    );
    util::write_file(&lay.prefix.join("occt-static.txt"), text)?;
    Ok(())
}

fn lib_totals(prefix: &Path) -> Result<(usize, u64)> {
    let mut n = 0;
    let mut bytes = 0;
    for e in fs::read_dir(prefix.join("lib"))? {
        let e = e?;
        if e.file_type()?.is_file() {
            n += 1;
            bytes += e.metadata()?.len();
        }
    }
    Ok((n, bytes))
}

/// Reads `occt-static.txt` of an installed prefix.
pub struct Manifest {
    pub crt: String,
    pub toolkits: Vec<String>,
    pub system_libs: Vec<String>,
}

pub fn read_manifest(prefix: &Path) -> Result<Manifest> {
    let path = prefix.join("occt-static.txt");
    let text = fs::read_to_string(&path).with_context(|| format!("{} (run `cargo xtask occt-static` first)", path.display()))?;
    let get = |k: &str| -> String {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{k}=")))
            .unwrap_or_default()
            .to_string()
    };
    let list = |s: String| -> Vec<String> { s.split(',').filter(|x| !x.is_empty()).map(str::to_string).collect() };
    Ok(Manifest { crt: get("crt"), toolkits: list(get("toolkits")), system_libs: list(get("system_libs")) })
}

/// Sizes of the installed static libraries per toolkit (for the report).
pub fn toolkit_sizes(prefix: &Path) -> Result<Vec<(String, u64)>> {
    let mut v = Vec::new();
    for e in fs::read_dir(prefix.join("lib"))? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(stem) = name.strip_suffix(".lib").or_else(|| name.strip_suffix(".a")) {
            v.push((stem.to_string(), e.metadata()?.len()));
        }
    }
    v.sort();
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_archive_values() {
        assert_eq!(ARCHIVE_SHA256.len(), 64);
        assert!(ARCHIVE_SHA256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(SOURCE_COMMIT.len(), 40);
    }

    #[test]
    fn dependents_come_first() {
        let mut d: BTreeMap<String, Vec<String>> = BTreeMap::new();
        d.insert("TKXCAF".into(), vec!["TKCAF".into(), "TKernel".into()]);
        d.insert("TKCAF".into(), vec!["TKernel".into()]);
        d.insert("TKernel".into(), vec![]);
        let o = dependents_first(&d);
        let pos = |n: &str| o.iter().position(|x| x == n).unwrap();
        assert!(pos("TKXCAF") < pos("TKCAF") && pos("TKCAF") < pos("TKernel"));
    }

    #[test]
    fn cmake_options_leave_out_optional_libraries() {
        let lay = layout(Path::new("C:/x"), Crt::Static, Opt::Speed);
        let a = cmake_args(&lay, Crt::Static, Opt::Speed, None);
        for must in ["-DBUILD_LIBRARY_TYPE=Static", "-DUSE_FREEIMAGE=OFF", "-DUSE_VTK=OFF", "-DUSE_TBB=OFF", "-DBUILD_MODULE_Draw=OFF", "-DUSE_FREETYPE=OFF"] {
            assert!(a.iter().any(|x| x == must), "{must}");
        }
        if cfg!(windows) {
            assert!(a.iter().any(|x| x.starts_with("-DCMAKE_CXX_FLAGS_RELEASE=/MT ")));
            let d = cmake_args(&lay, Crt::Dynamic, Opt::Speed, None);
            assert!(d.iter().any(|x| x.starts_with("-DCMAKE_CXX_FLAGS_RELEASE=/MD ")));
        }
    }
}
