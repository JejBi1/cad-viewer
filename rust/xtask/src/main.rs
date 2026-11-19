//! Release tooling of the Rust CAD viewer. Run through the `cargo xtask` alias (see `rust/.cargo/config.toml`, from
//! the `rust` folder) or `rust\xtask\xtask.ps1` (any folder, sets up the toolchain). `cargo xtask help` lists the commands.
mod dist;
mod linux;
mod notices;
mod occt;
mod occt_test;
mod pe;
mod sizes;
mod util;

use anyhow::Result;
use util::Args;

const HELP: &str = "\
cargo xtask <command> [options]

  occt-static   Build OCCT 8.0.1 from the pinned official archive as static libraries (MSVC, CMake, Ninja)
                  [--crt static|dynamic] [--opt speed|size] [--jobs N] [--root DIR] [--reconfigure] [--clean]
  occt-test     Run the occt-bridge tests with the `static-occt` feature and check the test binary imports
                  [--crt static|dynamic] [--occt-opt speed|size] [--profile NAME] [--skip-dynamic] [--build-only] [--lzma]
  dist          Windows distribution: installer stage, Inno Setup installer, portable zip, size report
                  [--no-dwg] [--out DIR] [--target-dir DIR] [--exe FILE] [--no-build] [--no-installer] [--no-sizes]
                  [--build-occt] [--crt static|dynamic] [--occt-opt speed|size] [--profile NAME]
  notices       Regenerate rust/xtask/installer/THIRD_PARTY_NOTICES.txt from cargo metadata
                  [--no-dwg] [--out FILE] [--check]
  linux-dist    Linux tar.gz / AppImage + size report (NOT RUN yet: to be run on the Ubuntu build host)
                  [--out DIR] [--target-dir DIR] [--no-dwg] [--no-build] [--dwg2dxf FILE]
  help          This text
";

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let cmd = args.next().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "help".into());
    let rest = Args::new(args.collect());
    match cmd.as_str() {
        "occt-static" => occt::command(rest),
        "occt-test" => occt_test::command(rest),
        "dist" => dist::command(rest),
        "notices" => notices::command(rest),
        "linux-dist" => linux::command(rest),
        "help" | "--help" | "-h" => {
            print!("{HELP}");
            Ok(())
        }
        other => anyhow::bail!("unknown command `{other}`\n\n{HELP}"),
    }
}
