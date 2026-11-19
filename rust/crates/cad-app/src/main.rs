//! CAD prohlížeč 2.0: the `StepViewer` executable. One program, several process roles chosen by the command line
//! (see `cli`): the window, the load worker, the Explorer thumbnail renderer and the Google Drive link commands.
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

mod app;
mod cli;
mod collide;
mod commands;
mod dialogs;
mod drawing2;
mod drive;
mod flow;
mod hooks;
mod keys;
mod legacy;
mod limits;
mod linkcmd;
mod mass;
mod measure3;
mod menus;
mod model3;
mod monitors;
mod panels;
mod props;
mod rich;
mod roles;
mod scene2d;
mod spacemouse;
mod spacemouse_ctl;
mod steprefs;
mod strings;
mod thumbnail;
mod thumbsettings;
mod toolbar;
mod ui;
mod util;
mod view2d;
mod view3d;

#[cfg(windows)]
mod spacemouse_win;
#[cfg(test)]
mod testgen;
#[cfg(test)]
mod tests_doc;
#[cfg(test)]
mod ui_tests;

use std::path::PathBuf;

use crate::strings::APP_NAME;

/// Unexpected errors of the window end in a message box (`sys.excepthook`): the last 1800 characters of the text.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let text = format!("{info}");
        eprintln!("{text}");
        let tail: String = {
            let chars: Vec<char> = text.chars().collect();
            chars[chars.len().saturating_sub(1800)..].iter().collect()
        };
        linkcmd::message(&format!("Neočekávaná chyba:\n\n{tail}"), true);
    }));
}

fn run_window(file: Option<PathBuf>, link: Option<String>) -> i32 {
    install_panic_hook();
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("StepViewer"));
    // a link never comes with a file (the parser guarantees it; this keeps it true if the parser changes): no path
    // is looked at before the link has been validated
    let file = if link.is_some() { None } else { file };
    let early = file.as_ref().and_then(|f| ui::early_start(&exe, f));
    match ui::run(ui::Init { file, link, exe, early }) {
        Ok(()) => 0,
        Err(e) => {
            linkcmd::message(&format!("Program se nepodařilo spustit:\n{e}"), true);
            1
        }
    }
}

fn main() {
    let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let code = match cli::parse(&argv) {
        cli::Command::Worker { src, dst } => roles::worker(src, dst),
        cli::Command::WorkerServe { job } => roles::serve(job),
        cli::Command::Thumbnail { src, out, size } => {
            // the renderer parses untrusted files: bound memory and CPU time (Linux; Windows has the job object)
            limits::limit_thumbnail_resources();
            thumbnail::run(&src, &out, &size)
        }
        cli::Command::RegisterLink => linkcmd::register(&std::env::current_exe().unwrap_or_else(|_| PathBuf::from(APP_NAME))),
        cli::Command::UnregisterLink => linkcmd::unregister(),
        // a link command line of another shape (see `cli::open_link`): nothing is opened, no path is touched
        cli::Command::BadLink(e) => {
            linkcmd::message(&drive::link_error_text(&e), true);
            1
        }
        cli::Command::Gui { file, link } => run_window(file, link),
    };
    std::process::exit(code);
}
