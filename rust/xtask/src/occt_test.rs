//! `cargo xtask occt-test`: proof that `occt-bridge` links the static OCCT (`static-occt`) and that its whole test suite
//! passes with it, with the test executables free of OCCT DLL imports and run without any OCCT DLL reachable.
//! A dynamic build of the same tests (the default, DLLs from OCCT_PREFIX) is built and run for the size comparison.
use crate::dist;
use crate::occt::{self, Crt};
use crate::pe;
use crate::sizes::group;
use crate::util::{self, Args, Toolchain, WINDOWS_TARGET};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// `cargo xtask occt-test [--crt static|dynamic] [--profile NAME] [--skip-dynamic] [--build-only]`
pub fn command(mut args: Args) -> Result<()> {
    if !cfg!(windows) {
        bail!("occt-test is implemented for the Windows (MSVC) build");
    }
    let crt = match args.value("--crt")? {
        Some(v) => Crt::parse(&v)?,
        None => Crt::Static,
    };
    let opt = match args.value("--occt-opt")? {
        Some(v) => occt::Opt::parse(&v)?,
        None => occt::Opt::Speed,
    };
    let profile = args.value("--profile")?.unwrap_or_else(|| "dev".to_string());
    let skip_dynamic = args.flag("--skip-dynamic");
    let build_only = args.flag("--build-only");
    let lzma = args.flag("--lzma");
    args.finish()?;

    let tc = Toolchain::discover()?;
    let prefix = occt::default_prefix_opt(crt, opt);
    let manifest = occt::read_manifest(&prefix)?;
    println!(
        "static OCCT in {}: CRT {}, {} toolkits ({}), system libraries: {}",
        prefix.display(),
        manifest.crt,
        manifest.toolkits.len(),
        manifest.toolkits.join(" "),
        manifest.system_libs.join(" ")
    );

    // ---- static OCCT
    let sdir = util::default_target_dir(&format!("occt-static-{}{}", crt.name(), if opt == occt::Opt::Size { "-os" } else { "" }));
    util::forbid_synced(&sdir, "target folder")?;
    let exes = test_executables(&tc, true, crt, &profile, &sdir, &prefix)?;
    if exes.is_empty() {
        bail!("no test executable of occt-bridge was built");
    }
    let mut static_total = 0u64;
    let mut passed_static = 0usize;
    println!("\n== static OCCT ({} CRT, optimised for {}), profile {profile}", crt.name(), opt.name());
    for exe in &exes {
        let size = fs::metadata(exe)?.len();
        static_total += size;
        let deps = pe::dependents(&tc, exe)?;
        println!("{}: {} bytes ({:.2} MB)\n  imports: {}", exe.display(), group(size), size as f64 / 1e6, deps.join(", "));
        let imports = pe::check_self_contained(&tc, exe, crt == Crt::Static)?;
        let tk: Vec<_> = imports.iter().filter(|d| d.starts_with("tk")).collect();
        if !tk.is_empty() {
            bail!("test binary imports OCCT DLLs: {tk:?}");
        }
        println!("  no TK*.dll imports; every import is a Windows system DLL");
        if !build_only {
            passed_static += run_tests(exe, true)?;
        }
    }

    // ---- dynamic OCCT, the default build
    if skip_dynamic {
        return Ok(());
    }
    let ddir = util::default_target_dir("occt-dynamic");
    util::forbid_synced(&ddir, "target folder")?;
    let dprefix = std::env::var_os("OCCT_PREFIX")
        .map(PathBuf::from)
        .unwrap_or_else(|| util::local_dir().join("occt-env").join("Library"));
    let dexes = test_executables(&tc, false, Crt::Dynamic, &profile, &ddir, &dprefix)?;
    println!("\n== dynamic OCCT (DLLs from {}), profile {profile}", dprefix.display());
    let bin = dprefix.join("bin");
    let mut dyn_total = 0u64;
    let mut dll_bytes = 0u64;
    let mut dll_names: Vec<String> = Vec::new();
    let mut dyn_exe: Option<PathBuf> = None;
    let mut passed_dynamic = 0usize;
    for exe in &dexes {
        let size = fs::metadata(exe)?.len();
        dyn_total += size;
        let (names, bytes) = pe::dll_closure(&tc, exe, &bin)?;
        println!("{}: {} bytes ({:.2} MB); needs {} DLLs from {}: {} bytes ({:.2} MB)", exe.display(), group(size), size as f64 / 1e6, names.len(), bin.display(), group(bytes), bytes as f64 / 1e6);
        if bytes > dll_bytes {
            dll_bytes = bytes;
            dll_names = names;
            dyn_exe = Some(exe.clone());
        }
        if !build_only {
            passed_dynamic += run_tests(exe, false)?;
        }
    }

    println!("\n== comparison (test executables of occt-bridge, profile {profile})");
    println!("{:<34} {:>16} {:>16}", "", "dynamic (DLLs)", "static");
    println!("{:<34} {:>16} {:>16}", "test executable, bytes", group(dyn_total), group(static_total));
    println!("{:<34} {:>16} {:>16}", "OCCT DLLs needed next to it, bytes", group(dll_bytes), "0");
    println!("{:<34} {:>16} {:>16}", "total on disk, bytes", group(dyn_total + dll_bytes), group(static_total));
    let saved = (dyn_total + dll_bytes) as i64 - static_total as i64;
    println!("static saves {} bytes ({:.2} MB); OCCT code the static link adds to the executable: {} bytes ({:.2} MB)", group(saved.unsigned_abs()), saved as f64 / 1e6, group(static_total.saturating_sub(dyn_total)), static_total.saturating_sub(dyn_total) as f64 / 1e6);
    println!("DLLs of the dynamic build ({}): {}", dll_names.len(), dll_names.join(" "));
    let tk_bytes: u64 = dll_names.iter().filter(|n| n.starts_with("tk")).map(|n| fs::metadata(bin.join(n)).map(|m| m.len()).unwrap_or(0)).sum();
    println!("  of which the TK*.dll toolkits: {} files, {} bytes ({:.2} MB)", dll_names.iter().filter(|n| n.starts_with("tk")).count(), group(tk_bytes), tk_bytes as f64 / 1e6);
    if lzma {
        // xz preset 9e, one solid stream per set
        let all: Vec<(PathBuf, String)> = dyn_exe
            .iter()
            .map(|e| (e.clone(), "exe".to_string()))
            .chain(dll_names.iter().map(|n| (bin.join(n), n.clone())))
            .collect();
        let tk: Vec<(PathBuf, String)> = dyn_exe
            .iter()
            .map(|e| (e.clone(), "exe".to_string()))
            .chain(dll_names.iter().filter(|n| n.starts_with("tk")).map(|n| (bin.join(n), n.clone())))
            .collect();
        let stat: Vec<(PathBuf, String)> = exes.iter().map(|e| (e.clone(), "exe".to_string())).collect();
        println!("xz (preset 9e): dynamic exe + all {} DLLs {} bytes; dynamic exe + TK*.dll only {} bytes; static exe {} bytes",
            dll_names.len(),
            group(crate::sizes::lzma_size(&all)?),
            group(crate::sizes::lzma_size(&tk)?),
            group(crate::sizes::lzma_size(&stat)?));
    }
    if !build_only {
        println!("tests passed: static {passed_static}, dynamic {passed_dynamic}");
    }
    Ok(())
}

/// Builds the test executables of occt-bridge and returns their paths (from cargo's JSON messages).
fn test_executables(tc: &Toolchain, static_occt: bool, crt: Crt, profile: &str, target_dir: &Path, occt_prefix: &Path) -> Result<Vec<PathBuf>> {
    let mut c = dist::cargo(tc);
    c.current_dir(util::rust_dir())
        .args(["test", "--package", "occt-bridge", "--no-run", "--message-format", "json", "--profile", profile])
        .arg("--target-dir")
        .arg(target_dir);
    if static_occt {
        // --target keeps the C runtime flag away from build scripts and proc macros
        c.args(["--target", WINDOWS_TARGET, "--features", "static-occt"])
            .env("CARGO_ENCODED_RUSTFLAGS", dist::rustflags(crt))
            .env("OCCT_STATIC_PREFIX", occt_prefix);
    } else {
        c.env_remove("RUSTFLAGS").env_remove("CARGO_ENCODED_RUSTFLAGS").env("OCCT_PREFIX", occt_prefix);
    }
    eprintln!("> {}", util::display(&c));
    let out = c.stdout(Stdio::piped()).stderr(Stdio::inherit()).output().context("starting cargo test --no-run")?;
    if !out.status.success() {
        bail!("cargo test --no-run failed ({})", out.status);
    }
    let mut exes = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v["reason"] != "compiler-artifact" || v["profile"]["test"] != true {
            continue;
        }
        if v["target"]["name"] != "occt_bridge" && v["target"]["name"] != "occt-bridge" {
            // integration tests of the package have their own target names but the same package id
            if !v["package_id"].as_str().is_some_and(|p| p.contains("occt-bridge")) {
                continue;
            }
        }
        if let Some(e) = v["executable"].as_str() {
            exes.push(PathBuf::from(e));
        }
    }
    Ok(exes)
}

/// Runs a test executable. The static one gets a PATH with Windows only: no OCCT DLL can be picked up by accident.
/// Returns the number of tests that passed.
fn run_tests(exe: &Path, windows_only_path: bool) -> Result<usize> {
    let crate_dir = util::rust_dir().join("crates").join("occt-bridge");
    let mut c = std::process::Command::new(exe);
    c.current_dir(&crate_dir).env("CARGO_MANIFEST_DIR", &crate_dir);
    if windows_only_path {
        let sys = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        c.env("PATH", std::env::join_paths([sys.join("System32"), sys])?);
    } else {
        let tc = Toolchain::discover()?;
        tc.apply(&mut c);
        c.env("PATH", std::env::join_paths(
            [util::local_dir().join("occt-env").join("Library").join("bin")]
                .into_iter()
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())),
        )?);
    }
    eprintln!("> {} (PATH = {})", exe.display(), if windows_only_path { "Windows only" } else { "with the OCCT DLLs" });
    let out = c.output().with_context(|| exe.display().to_string())?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let summary = text.lines().rev().find(|l| l.starts_with("test result:")).unwrap_or("(no summary line)");
    println!("  {summary}");
    if !out.status.success() {
        let tail: Vec<&str> = text.lines().rev().take(60).collect();
        bail!("tests failed ({}):\n{}", out.status, tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
    }
    let passed = summary
        .split("passed")
        .next()
        .and_then(|s| s.rsplit(' ').find(|w| w.parse::<usize>().is_ok()))
        .and_then(|w| w.parse().ok())
        .unwrap_or(0);
    Ok(passed)
}
