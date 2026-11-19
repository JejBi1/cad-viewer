//! Paths, toolchain discovery, process helpers and hashing shared by all xtask commands.
use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Version of the whole workspace (`[workspace.package] version`), the version of the release.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Rust target of the Windows release.
pub const WINDOWS_TARGET: &str = "x86_64-pc-windows-msvc";
/// Rust target of the Linux release (x86_64, glibc).
pub const LINUX_TARGET: &str = "x86_64-unknown-linux-gnu";

/// Environment variables of the Google Drive sign-in. cad-drive reads them with `option_env!` when the program is
/// compiled, so they must be in the environment of the build (`dist` forwards them, see [`forward_drive_env`]).
pub const DRIVE_CLIENT_ID_ENV: &str = "CADVIEWER_DRIVE_CLIENT_ID";
pub const DRIVE_CLIENT_SECRET_ENV: &str = "CADVIEWER_DRIVE_CLIENT_SECRET";

/// Passes the Drive variables to a build, or removes them when they are not set, so the build is the one the size
/// report describes. The values are not printed.
pub fn forward_drive_env(cmd: &mut Command) {
    for name in [DRIVE_CLIENT_ID_ENV, DRIVE_CLIENT_SECRET_ENV] {
        match std::env::var_os(name) {
            Some(v) => {
                cmd.env(name, v);
            }
            None => {
                cmd.env_remove(name);
            }
        }
    }
}

/// `<repo>`: this crate lives in `<repo>/rust/xtask`.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask lies in <repo>/rust/xtask")
        .to_path_buf()
}

/// `<repo>/rust`, the Cargo workspace.
pub fn rust_dir() -> PathBuf {
    repo_root().join("rust")
}

pub fn home() -> PathBuf {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// `~/.local` (the user-space tool folder of this project, see `rust/tools/devenv.ps1`).
pub fn local_dir() -> PathBuf {
    home().join(".local")
}

/// Default root of the static OCCT build: `~/.local/occt-static` (`OCCT_STATIC_ROOT` overrides it).
pub fn occt_root() -> PathBuf {
    std::env::var_os("OCCT_STATIC_ROOT")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| local_dir().join("occt-static"))
}

/// Default root of the release output: `~/.local/cad-dist`.
pub fn dist_root() -> PathBuf {
    local_dir().join("cad-dist")
}

/// Default Cargo target folder of the release build (never inside the synchronised repository).
pub fn default_target_dir(flavour: &str) -> PathBuf {
    home().join(".venvs").join(format!("cad-target-xtask-{flavour}"))
}

/// Build products, downloads and tools must not live in the Synology Drive folder or in the repository: the sync
/// client locks files that are being written or deleted.
pub fn forbid_synced(path: &Path, what: &str) -> Result<()> {
    let abs = std::path::absolute(path).with_context(|| format!("{what}: {}", path.display()))?;
    let text = abs.to_string_lossy().to_lowercase();
    let repo = std::path::absolute(repo_root())?.to_string_lossy().to_lowercase();
    if text.contains("synologydrive") || text.starts_with(&repo) {
        bail!(
            "{what} {} would lie inside the synchronised repository (SynologyDrive); choose a folder under your user profile",
            abs.display()
        );
    }
    Ok(())
}

/// User-space toolchain, the same one `rust/tools/devenv.ps1` sets up: portable MSVC + Windows SDK, cmake and ninja
/// from the OCCT conda environment, `~/.cargo/bin`. Everything that is already in the environment is kept.
pub struct Toolchain {
    vars: Vec<(String, String)>,
    path: Vec<PathBuf>,
    /// Include folders appended after the SDK ones (so a real header always wins over a stand-in).
    extra_include: Vec<PathBuf>,
}

impl Toolchain {
    pub fn discover() -> Result<Toolchain> {
        let mut vars = Vec::new();
        let mut path = Vec::new();
        if cfg!(windows) {
            let msvc = local_dir().join("msvc-portable");
            let json = std::env::var_os("MSVC_ENV_JSON")
                .map(PathBuf::from)
                .unwrap_or_else(|| msvc.join("env.json"));
            if json.is_file() {
                let raw = fs::read(&json).with_context(|| format!("reading {}", json.display()))?;
                let raw = raw.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&raw);
                let v: serde_json::Value =
                    serde_json::from_slice(raw).with_context(|| format!("parsing {}", json.display()))?;
                for key in ["INCLUDE", "LIB", "VCToolsInstallDir", "WindowsSdkDir", "WindowsSDKVersion"] {
                    if let Some(s) = v.get(key).and_then(|s| s.as_str()) {
                        vars.push((key.to_string(), s.to_string()));
                    }
                }
                if let Some(p) = v.get("PATH").and_then(|s| s.as_str()) {
                    path.extend(p.split(';').filter(|s| !s.is_empty()).map(PathBuf::from));
                }
            }
            path.push(local_dir().join("occt-env").join("Library").join("bin"));
            path.push(home().join(".cargo").join("bin"));
        }
        Ok(Toolchain { vars, path, extra_include: Vec::new() })
    }

    /// The folders the compiler searches for headers (INCLUDE of this process, else of env.json).
    pub fn include_dirs(&self) -> Vec<PathBuf> {
        let base = std::env::var("INCLUDE")
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| self.vars.iter().find(|(k, _)| k == "INCLUDE").map(|(_, v)| v.clone()))
            .unwrap_or_default();
        base.split(';').filter(|s| !s.is_empty()).map(PathBuf::from).collect()
    }

    /// Appends a folder to INCLUDE for every command made from this toolchain.
    pub fn add_include(&mut self, dir: PathBuf) {
        self.extra_include.push(dir);
    }

    /// Sets the toolchain variables on `cmd`; the toolchain folders go in front of `PATH`.
    pub fn apply(&self, cmd: &mut Command) {
        for (k, v) in &self.vars {
            // an already prepared environment (devenv.ps1 was sourced) wins
            if k != "INCLUDE" && std::env::var_os(k).is_none() {
                cmd.env(k, v);
            }
        }
        let mut include: Vec<PathBuf> = self.include_dirs();
        include.extend(self.extra_include.iter().cloned());
        if !include.is_empty()
            && let Ok(joined) = std::env::join_paths(include)
        {
            cmd.env("INCLUDE", joined);
        }
        let mut dirs: Vec<PathBuf> = self.path.iter().filter(|p| p.is_dir()).cloned().collect();
        if let Some(old) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&old));
        }
        if let Ok(joined) = std::env::join_paths(dirs) {
            cmd.env("PATH", joined);
        }
    }

    /// A command for `program` (resolved through the toolchain folders first) with the environment applied.
    pub fn command(&self, program: &str) -> Command {
        let exe = self.find(program).unwrap_or_else(|| PathBuf::from(program));
        let mut c = Command::new(exe);
        self.apply(&mut c);
        c
    }

    /// Looks `exe` up in the toolchain folders, then in `PATH`.
    pub fn find(&self, exe: &str) -> Option<PathBuf> {
        let name = if cfg!(windows) && Path::new(exe).extension().is_none() {
            format!("{exe}.exe")
        } else {
            exe.to_string()
        };
        let mut dirs: Vec<PathBuf> = self.path.clone();
        if let Some(old) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&old));
        }
        dirs.into_iter().map(|d| d.join(&name)).find(|p| p.is_file())
    }

    pub fn require(&self, exe: &str) -> Result<PathBuf> {
        self.find(exe)
            .ok_or_else(|| anyhow!("`{exe}` was not found; source rust\\tools\\devenv.ps1 or install the user-space toolchain"))
    }
}

/// Runs `cmd` with inherited output; an error carries the printable command line.
pub fn run(cmd: &mut Command) -> Result<()> {
    eprintln!("> {}", display(cmd));
    let status = cmd.status().with_context(|| format!("starting {}", display(cmd)))?;
    if !status.success() {
        bail!("command failed ({status}): {}", display(cmd));
    }
    Ok(())
}

/// Runs `cmd` and returns stdout and stderr (lossy UTF-8, concatenated); the exit status is returned too.
pub fn capture(cmd: &mut Command) -> Result<(bool, String)> {
    let out = cmd.output().with_context(|| format!("starting {}", display(cmd)))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), text))
}

pub fn display(cmd: &Command) -> String {
    let mut s = cmd.get_program().to_string_lossy().into_owned();
    for a in cmd.get_args() {
        s.push(' ');
        let a = a.to_string_lossy();
        if a.contains(' ') {
            s.push('"');
            s.push_str(&a);
            s.push('"');
        } else {
            s.push_str(&a);
        }
    }
    s
}

/// SHA-256 of a file, lower-case hex.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

pub fn sha256_bytes(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex(&h.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Decimal megabytes with one or two decimals, the unit the 1.9 baseline uses (55.9 MB installer).
pub fn mb(bytes: u64) -> String {
    format!("{:.2} MB", bytes as f64 / 1e6)
}

/// All regular files below `dir` (sorted, relative `/`-separated names).
pub fn list_files(dir: &Path) -> Result<Vec<(PathBuf, String)>> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(PathBuf, String)>) -> Result<()> {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .with_context(|| format!("reading {}", dir.display()))?
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let ty = e.file_type()?;
            if ty.is_dir() {
                walk(base, &p, out)?;
            } else if ty.is_file() {
                let rel = p.strip_prefix(base)?.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
                out.push((p, rel));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
}

/// Removes a folder tree if it exists (also read-only files, which CMake and Git create).
pub fn remove_tree(dir: &Path) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(_) => {
            for (p, _) in list_files(dir).unwrap_or_default() {
                if let Ok(meta) = fs::metadata(&p) {
                    let mut perm = meta.permissions();
                    #[allow(clippy::permissions_set_readonly_false)] // Windows: only the read-only attribute
                    perm.set_readonly(false);
                    let _ = fs::set_permissions(&p, perm);
                }
            }
            fs::remove_dir_all(dir).with_context(|| format!("removing {}", dir.display()))
        }
    }
}

pub fn write_file(path: &Path, data: impl AsRef<[u8]>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, data).with_context(|| format!("writing {}", path.display()))
}

pub fn copy_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(from, to).with_context(|| format!("copying {} -> {}", from.display(), to.display()))?;
    Ok(())
}

/// Parses `--name value` / `--name=value` / `--flag` arguments. Unknown arguments are an error.
pub struct Args {
    rest: Vec<OsString>,
}

impl Args {
    pub fn new(rest: Vec<OsString>) -> Args {
        Args { rest }
    }

    /// True (and consumed) when the flag is present.
    pub fn flag(&mut self, name: &str) -> bool {
        let before = self.rest.len();
        self.rest.retain(|a| a != OsStr::new(name));
        self.rest.len() != before
    }

    /// Value of `--name value` or `--name=value`.
    pub fn value(&mut self, name: &str) -> Result<Option<String>> {
        let prefix = format!("{name}=");
        let mut i = 0;
        while i < self.rest.len() {
            let a = self.rest[i].to_string_lossy().into_owned();
            if a == name {
                if i + 1 >= self.rest.len() {
                    bail!("{name} needs a value");
                }
                let v = self.rest[i + 1].to_string_lossy().into_owned();
                self.rest.drain(i..=i + 1);
                return Ok(Some(v));
            }
            if let Some(v) = a.strip_prefix(&prefix) {
                let v = v.to_string();
                self.rest.remove(i);
                return Ok(Some(v));
            }
            i += 1;
        }
        Ok(None)
    }

    pub fn finish(self) -> Result<()> {
        if let Some(a) = self.rest.first() {
            bail!("unknown argument {:?} (see `cargo xtask help`)", a);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Args {
        Args::new(v.iter().map(OsString::from).collect())
    }

    #[test]
    fn flags_and_values() {
        let mut a = args(&["--no-dwg", "--out", "D:/x y", "--jobs=4"]);
        assert!(a.flag("--no-dwg"));
        assert!(!a.flag("--no-dwg"));
        assert_eq!(a.value("--out").unwrap().as_deref(), Some("D:/x y"));
        assert_eq!(a.value("--jobs").unwrap().as_deref(), Some("4"));
        assert!(a.value("--missing").unwrap().is_none());
        a.finish().unwrap();
        assert!(args(&["--oops"]).finish().is_err());
        assert!(args(&["--out"]).value("--out").is_err());
    }

    #[test]
    fn sha256_known_vector() {
        assert_eq!(sha256_bytes(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn synced_folders_are_refused() {
        assert!(forbid_synced(&repo_root().join("target"), "x").is_err());
        assert!(forbid_synced(Path::new(r"C:\Users\someone\SynologyDrive\x"), "x").is_err());
        assert!(forbid_synced(&home().join(".local").join("cad-dist"), "x").is_ok());
    }

    #[test]
    fn mb_is_decimal() {
        assert_eq!(mb(55_901_762), "55.90 MB");
    }
}
