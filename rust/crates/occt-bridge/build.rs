//! Builds the C++ side of the bridge with cxx-build against OCCT 8.0.x.
//!
//! * Default: OCCT as DLLs found through `OCCT_PREFIX` (headers in include/opencascade, import libraries in lib,
//!   DLLs in bin), as before.
//! * Feature `static-occt`: OCCT as static libraries built by `cargo xtask occt-static`. They are looked up in
//!   `OCCT_STATIC_PREFIX`, else in `<OCCT_STATIC_ROOT or ~/.local/occt-static>/prefix-<flavour>` where the flavour
//!   follows the C runtime of this build: `mt` with `-C target-feature=+crt-static`, `md` without it (Windows), `linux`
//!   on Linux. The libraries and the list of toolkits come from `occt-static.txt` in the prefix.
//!   The C runtime of the libraries, of the bridge C++ code and of Rust must be the same; a mismatch stops the build.
use std::path::{Path, PathBuf};
use std::{env, fs};

/// OCCT toolkits the bridge needs when it links the DLLs (data exchange, XCAF, meshing, topology algorithms, kernel).
const OCCT_LIBS: &[&str] = &[
    "TKernel",
    "TKMath",
    "TKG2d",
    "TKG3d",
    "TKGeomBase",
    "TKBRep",
    "TKGeomAlgo",
    "TKTopAlgo",
    "TKPrim",
    "TKBO",
    "TKBool",
    "TKMesh",
    "TKShHealing",
    "TKXSBase",
    "TKDE",
    "TKDESTEP",
    "TKDEIGES",
    "TKDESTL",
    "TKLCAF",
    "TKCDF",
    "TKCAF",
    "TKXCAF",
    "TKBin",
    "TKBinL",
    "TKBinXCAF",
    "TKRWMesh",
    "TKXMesh",
];

/// What the static path needs on top of the include / lib folders.
struct StaticLink {
    crt_static: bool,
    toolkits: Vec<String>,
    system_libs: Vec<String>,
}

struct Occt {
    include: PathBuf,
    lib: PathBuf,
    static_link: Option<StaticLink>,
}

fn main() {
    println!("cargo:rerun-if-env-changed=OCCT_PREFIX");
    println!("cargo:rerun-if-env-changed=OCCT_STATIC_PREFIX");
    println!("cargo:rerun-if-env-changed=OCCT_STATIC_ROOT");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/ffi.rs");
    println!("cargo:rerun-if-changed=cpp");

    let occt = if env::var_os("CARGO_FEATURE_STATIC_OCCT").is_some() {
        static_occt()
    } else {
        dynamic_occt()
    };

    let mut build = cxx_build::bridge("src/ffi.rs");
    build
        .files(["cpp/doc.cpp", "cpp/extract.cpp", "cpp/analysis.cpp"])
        .include(&occt.include)
        .include("cpp")
        .std("c++17")
        .flag_if_supported("/EHsc")
        .flag_if_supported("/utf-8")
        .flag_if_supported("/permissive-")
        .flag_if_supported("/bigobj")
        .flag_if_supported("/wd4996")
        .flag_if_supported("/wd4267")
        .flag_if_supported("/wd4244")
        .flag_if_supported("/wd4100")
        .warnings(false);
    if let Some(s) = &occt.static_link {
        // Standard_Macro.hxx: without it the headers declare every OCCT symbol __declspec(dllimport)
        build.define("OCCT_STATIC_BUILD", None).define("OCCT_NO_PLUGINS", None);
        build.static_crt(s.crt_static);
    }
    build.compile("occt_bridge_cpp");

    println!("cargo:rustc-link-search=native={}", occt.lib.display());
    match &occt.static_link {
        None => {
            for l in OCCT_LIBS {
                println!("cargo:rustc-link-lib=dylib={l}");
            }
        }
        Some(s) => {
            // `-bundle`: the libraries are passed to the final link, not copied into the rlib (they are ~100 MB)
            for l in &s.toolkits {
                println!("cargo:rustc-link-lib=static:-bundle={l}");
            }
            for l in &s.system_libs {
                println!("cargo:rustc-link-lib=dylib={l}");
            }
            if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
                // libstdc++ must come after the static OCCT archives (--as-needed drops it when listed earlier)
                println!("cargo:rustc-link-lib=dylib=stdc++");
            }
        }
    }
}

fn dynamic_occt() -> Occt {
    let prefix = env::var("OCCT_PREFIX").unwrap_or_else(|_| {
        panic!("OCCT_PREFIX is not set (dot-source rust/tools/devenv.ps1 from the repository root first)")
    });
    let prefix = PathBuf::from(prefix);
    let include = prefix.join("include").join("opencascade");
    let lib = prefix.join("lib");
    assert!(
        include.is_dir(),
        "OCCT headers not found in {}",
        include.display()
    );
    assert!(
        lib.is_dir(),
        "OCCT libraries not found in {}",
        lib.display()
    );
    Occt { include, lib, static_link: None }
}

fn static_occt() -> Occt {
    let features = env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();
    let crt_static = features.split(',').any(|f| f == "crt-static");
    let windows = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let prefix = match env::var_os("OCCT_STATIC_PREFIX").filter(|v| !v.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => {
            let root = env::var_os("OCCT_STATIC_ROOT")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join(".local").join("occt-static"));
            let flavour = match (windows, crt_static) {
                (false, _) => "linux",
                (true, true) => "mt",
                (true, false) => "md",
            };
            root.join(format!("prefix-{flavour}"))
        }
    };
    let manifest_path = prefix.join("occt-static.txt");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let manifest = fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        panic!(
            "feature `static-occt`: {} is missing ({e}). Build the static OCCT first: `cargo xtask occt-static{}`",
            manifest_path.display(),
            if windows && !crt_static { " --crt dynamic" } else { "" }
        )
    });
    let get = |key: &str| -> String {
        manifest
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{key}=")))
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let list = |key: &str| -> Vec<String> {
        get(key).split(',').filter(|s| !s.is_empty()).map(str::to_string).collect()
    };
    if windows {
        let built = get("crt");
        let wanted = if crt_static { "static" } else { "dynamic" };
        if built != wanted {
            panic!(
                "feature `static-occt`: the OCCT libraries in {} use the {built} C runtime, this build uses the {wanted} one. \
                 {}",
                prefix.display(),
                if crt_static {
                    "Build with `cargo xtask occt-static --crt dynamic` or drop RUSTFLAGS=-C target-feature=+crt-static."
                } else {
                    "Set RUSTFLAGS=-C target-feature=+crt-static (and build with --target), or run `cargo xtask occt-static --crt dynamic`."
                }
            );
        }
    }
    let toolkits = list("toolkits");
    assert!(!toolkits.is_empty(), "no toolkits listed in {}", manifest_path.display());
    let include = prefix.join("include").join("opencascade");
    let lib = prefix.join("lib");
    assert!(include.is_dir(), "OCCT headers not found in {}", include.display());
    assert!(lib.is_dir(), "OCCT libraries not found in {}", lib.display());
    Occt {
        include,
        lib,
        static_link: Some(StaticLink { crt_static, toolkits, system_libs: list("system_libs") }),
    }
}

fn home() -> PathBuf {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env::var_os(var).map(PathBuf::from).unwrap_or_else(|| Path::new(".").to_path_buf())
}
