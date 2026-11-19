//! Command line of `StepViewer` (SPEC section 14, "CLI"). One executable runs in several roles; the role is chosen
//! from the arguments before anything else is initialised.
use std::ffi::OsString;
use std::path::PathBuf;

use cad_drive::LinkError;

/// What this process has to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// `--worker <src> <dst>`: one load into a cache file, then exit.
    Worker { src: PathBuf, dst: PathBuf },
    /// `--worker-serve <job>`: a pre-started idle worker that waits for its job file.
    WorkerServe { job: PathBuf },
    /// `--thumbnail <src> <png> <size>`: render an Explorer thumbnail, exit code 0 = PNG written.
    Thumbnail { src: PathBuf, out: PathBuf, size: String },
    /// `--register-link`: add the `cadviewer` scheme under HKCU (or the XDG equivalent).
    RegisterLink,
    /// `--unregister-link`.
    UnregisterLink,
    /// The window. `file` is the first argument that does not start with `--`; `link` comes from `--open-link` (then
    /// `file` is always `None`).
    Gui { file: Option<PathBuf>, link: Option<String> },
    /// `--open-link` that is not exactly `--open-link <valid link>`: nothing is opened, the reason is shown.
    BadLink(LinkError),
}

/// The flag of the registered link command (`"<exe>" --open-link "%1"`).
const OPEN_LINK: &str = "--open-link";

fn is_flag(a: &OsString) -> bool {
    a.to_string_lossy().starts_with("--")
}

/// `--open-link` is the command of a registered `cadviewer://` link, so its argument comes from outside (a web page
/// can start it with any text). A quote in the link splits it into more arguments on a Windows command line:
/// `--open-link "cadviewer://open?fileId=x" "\\server\share\a.stl"` would otherwise open the second one as a file
/// (and touch a network path). So the only accepted shape is exactly `<program> --open-link <link>`, the link is
/// validated here, before anything else happens, and every other shape is refused with the link error.
fn open_link(argv: &[OsString]) -> Command {
    if argv.len() < 3 {
        return Command::BadLink(LinkError::Empty);
    }
    if argv.len() != 3 || argv[1].to_string_lossy() != OPEN_LINK {
        return Command::BadLink(LinkError::BadCharacters);
    }
    let Some(url) = argv[2].to_str() else {
        return Command::BadLink(LinkError::BadCharacters);
    };
    match cad_drive::parse_link(url) {
        Ok(_) => Command::Gui { file: None, link: Some(url.to_string()) },
        Err(e) => Command::BadLink(e),
    }
}

/// Parses `argv` (the program name first, as `std::env::args_os` gives it). The length rules are those of
/// `main.py`: `--thumbnail` needs `len >= 5`, `--worker` `len >= 4`, `--worker-serve` `len >= 3`; a flag with too
/// few arguments is ignored and the window opens. `--open-link` anywhere in the arguments takes over completely
/// (see [`open_link`]): no other role and no file argument is looked at.
pub fn parse(argv: &[OsString]) -> Command {
    if argv.iter().skip(1).any(|a| a.to_string_lossy() == OPEN_LINK) {
        return open_link(argv);
    }
    let arg = |i: usize| argv.get(i).map(|s| s.to_string_lossy().into_owned());
    match arg(1).as_deref() {
        Some("--thumbnail") if argv.len() >= 5 => {
            return Command::Thumbnail {
                src: PathBuf::from(&argv[2]),
                out: PathBuf::from(&argv[3]),
                size: argv[4].to_string_lossy().into_owned(),
            };
        }
        Some("--worker") if argv.len() >= 4 => {
            return Command::Worker { src: PathBuf::from(&argv[2]), dst: PathBuf::from(&argv[3]) };
        }
        Some("--worker-serve") if argv.len() >= 3 => {
            return Command::WorkerServe { job: PathBuf::from(&argv[2]) };
        }
        Some("--register-link") => return Command::RegisterLink,
        Some("--unregister-link") => return Command::UnregisterLink,
        _ => {}
    }
    let file = argv.iter().skip(1).find(|a| !is_flag(a)).map(PathBuf::from);
    Command::Gui { file, link: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn plain_start_opens_the_window() {
        assert_eq!(parse(&argv(&["StepViewer"])), Command::Gui { file: None, link: None });
    }

    #[test]
    fn first_argument_without_dashes_is_the_file() {
        assert_eq!(
            parse(&argv(&["StepViewer", "C:\\m\\a.step"])),
            Command::Gui { file: Some(PathBuf::from("C:\\m\\a.step")), link: None }
        );
        assert_eq!(
            parse(&argv(&["StepViewer", "--something", "x.dxf", "y.dxf"])),
            Command::Gui { file: Some(PathBuf::from("x.dxf")), link: None }
        );
    }

    #[test]
    fn thumbnail_needs_four_arguments() {
        assert_eq!(
            parse(&argv(&["StepViewer", "--thumbnail", "in.step", "out.png", "256"])),
            Command::Thumbnail { src: "in.step".into(), out: "out.png".into(), size: "256".into() }
        );
        // too few arguments: the window opens (main.py checks len(sys.argv) >= 5)
        assert!(matches!(parse(&argv(&["StepViewer", "--thumbnail", "in.step", "out.png"])), Command::Gui { .. }));
    }

    #[test]
    fn worker_roles() {
        assert_eq!(
            parse(&argv(&["StepViewer", "--worker", "a.step", "b.xbf"])),
            Command::Worker { src: "a.step".into(), dst: "b.xbf".into() }
        );
        assert_eq!(
            parse(&argv(&["StepViewer", "--worker-serve", "job.tmp"])),
            Command::WorkerServe { job: "job.tmp".into() }
        );
        assert!(matches!(parse(&argv(&["StepViewer", "--worker", "a.step"])), Command::Gui { .. }));
        assert!(matches!(parse(&argv(&["StepViewer", "--worker-serve"])), Command::Gui { .. }));
    }

    const LINK: &str = "cadviewer://open?fileId=abcdefghij";

    #[test]
    fn link_commands() {
        assert_eq!(parse(&argv(&["StepViewer", "--register-link"])), Command::RegisterLink);
        assert_eq!(parse(&argv(&["StepViewer", "--unregister-link"])), Command::UnregisterLink);
        assert_eq!(
            parse(&argv(&["StepViewer", "--open-link", LINK])),
            Command::Gui { file: None, link: Some(LINK.into()) }
        );
    }

    #[test]
    fn open_link_value_is_never_taken_for_a_file() {
        let c = parse(&argv(&["StepViewer", "--open-link", LINK]));
        assert!(matches!(c, Command::Gui { file: None, .. }));
    }

    /// What Windows makes of `"<exe>" --open-link "%1"` when `%1` is
    /// `cadviewer://open?fileId=abcdefghij" "<file>`: a second argument that used to be opened as a file.
    #[test]
    fn a_quote_in_the_link_cannot_smuggle_in_a_file_argument() {
        for file in [r"C:\Users\x\injected.stl", r"\\server\share\a.step", "x.dxf", "--worker", ""] {
            let c = parse(&argv(&["StepViewer", "--open-link", LINK, file]));
            assert_eq!(c, Command::BadLink(LinkError::BadCharacters), "{file:?}");
        }
        // more of them, and the file first
        assert_eq!(
            parse(&argv(&["StepViewer", "--open-link", LINK, "a.stl", "b.stl"])),
            Command::BadLink(LinkError::BadCharacters)
        );
        assert_eq!(parse(&argv(&["StepViewer", "a.stl", "--open-link", LINK])), Command::BadLink(LinkError::BadCharacters));
        assert_eq!(parse(&argv(&["StepViewer", "--open-link", "--open-link", LINK])), Command::BadLink(LinkError::BadCharacters));
        // no role and no flag of the others can come along
        for other in ["--register-link", "--unregister-link", "--worker-serve"] {
            assert!(matches!(parse(&argv(&["StepViewer", other, "--open-link", LINK])), Command::BadLink(_)), "{other}");
        }
        assert!(matches!(
            parse(&argv(&["StepViewer", "--thumbnail", "a.step", "o.png", "--open-link", LINK])),
            Command::BadLink(_)
        ));
        // the window never gets a file together with a link
        for a in [
            argv(&["StepViewer", "--open-link", LINK, "x.stl"]),
            argv(&["StepViewer", "x.stl", "--open-link", LINK]),
            argv(&["StepViewer", "--open-link", LINK]),
        ] {
            assert!(!matches!(parse(&a), Command::Gui { file: Some(_), .. }));
        }
    }

    #[test]
    fn open_link_without_a_link_is_refused() {
        assert_eq!(parse(&argv(&["StepViewer", "--open-link"])), Command::BadLink(LinkError::Empty));
        assert_eq!(parse(&argv(&["StepViewer", "--open-link", ""])), Command::BadLink(LinkError::Empty));
    }

    #[test]
    fn the_link_is_validated_before_the_window_exists() {
        // only the exact shape passes; a file path, an URL of another kind or anything with a quote does not
        for (bad, expect) in [
            (r"C:\Users\x\injected.stl", LinkError::BadStructure),
            ("injected.stl", LinkError::BadScheme),
            ("https://example.com/a.stl", LinkError::BadScheme),
            ("cadviewer://open?fileId=abc", LinkError::BadFileId),
            ("cadviewer://open?fileId=abcdefghij\"", LinkError::BadFileId),
            ("cadviewer://open?fileId=abcdefghij&x=1", LinkError::UnexpectedParameter),
            ("cadviewer://open?fileId=abcdefghij abc", LinkError::BadCharacters),
            ("cadviewer://evil?fileId=abcdefghij", LinkError::BadHost),
        ] {
            assert_eq!(parse(&argv(&["StepViewer", "--open-link", bad])), Command::BadLink(expect), "{bad:?}");
        }
        // the Czech text the user sees
        assert_eq!(LinkError::BadCharacters.to_string(), "Odkaz obsahuje nepovolené znaky.");
        assert_eq!(LinkError::Empty.to_string(), "Odkaz je prázdný.");
    }

    #[test]
    fn a_file_name_that_looks_like_the_flag_is_not_the_flag() {
        // the Explorer thumbnail role passes full paths: only the exact text `--open-link` counts
        let c = parse(&argv(&["StepViewer", "--thumbnail", r"C:\x\--open-link", "o.png", "64"]));
        assert!(matches!(c, Command::Thumbnail { .. }));
    }
}
