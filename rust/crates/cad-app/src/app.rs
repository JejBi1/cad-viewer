//! The window: state, the frame loop and the commands (`MainWindow` of `main.py`).
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use cad_core::settings::Settings;
use cad_render::{Overlays, Renderer2d, Renderer3d, StdView, Style};
use eframe::egui;

use crate::collide::CollisionDlg;
use crate::drawing2::{Draw2, Measure2};
use crate::flow::{Event, Flow};
use crate::hooks::Hooks;
use crate::measure3::Measure3;
use crate::model3::Model3;
use crate::props::Props;
use crate::strings::APP_NAME;
use crate::view2d::Mouse2;
use crate::view3d::Mouse3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The start screen.
    Empty,
    ThreeD,
    TwoD,
}

/// Which measuring tool is on (`_measure_kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasureKind {
    Dist,
    Angle,
}

/// Every command of the toolbar, the shortcuts and the menus.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Open,
    OpenPath(PathBuf),
    Reload,
    Fit,
    ZoomSel,
    View(StdView),
    Edges,
    Wire,
    Persp,
    Measure,
    Angle,
    CopyMeasure,
    Material,
    Mass,
    Collide,
    Shot,
    Hide,
    Isolate,
    ShowAll,
    Transp,
    Escape,
    Expand,
    Collapse,
    About,
    PinMeasure,
    ClearPicked,
    EndMeasure,
    CopyNames,
    ClearCache,
    ThumbSettings,
    SpaceMouseSettings,
    /// Menu "Zaregistrovat odkazy Google Drive": `cadviewer://` for this user (HKCU / XDG), on the user's click only.
    RegisterLink,
    /// Menu "Odregistrovat odkazy Google Drive".
    UnregisterLink,
    /// Menu "Odhlásit se z Google Drive": forgets the stored sign-in.
    DriveSignOut,
}

pub struct Opts {
    pub edges: bool,
    pub wire: bool,
    pub persp: bool,
    /// 0 = off, 1..3 = X, Y, Z (`sec_axis.currentIndex()`).
    pub section_axis: usize,
    /// 0..1000.
    pub section_slider: f32,
    pub section_flip: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { edges: true, wire: false, persp: false, section_axis: 0, section_slider: 500.0, section_flip: false }
    }
}

#[derive(Default)]
pub struct Status {
    pub model: String,
    /// A short note in the colour of warnings next to the position of the cursor: the drawing is drawn with less detail
    /// (`strings::STATUS_REDUCED_DETAIL`); empty when it is not. The whole note is in the panel of the drawing.
    pub note: String,
    pub coord: String,
    /// `statusBar().showMessage`: replaces the model text for a while.
    pub temp: Option<(String, Instant)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsgKind {
    Info,
    Warning,
    Critical,
}

pub struct MsgBox {
    pub kind: MsgKind,
    pub text: String,
}

/// Open dialogs other than the message boxes.
#[derive(Default)]
pub struct Dialogs {
    pub material: Option<crate::dialogs::MaterialDlg>,
    pub collisions: Option<CollisionDlg>,
    pub spacemouse: Option<crate::dialogs::SpaceMouseDlg>,
    pub thumbs: Option<crate::dialogs::ThumbDlg>,
    pub about: bool,
    pub context: Option<crate::menus::ContextMenu>,
}

/// Overlays that are not measurements.
#[derive(Default)]
pub struct OverlaySet {
    /// Centre of gravity.
    pub cog: Overlays,
    /// Interference volumes.
    pub collision: Overlays,
}

pub struct PanelLayout {
    pub left_width: f32,
    /// Share of the left column taken by the tree (3 : 1 in `main.py`).
    pub tree_frac: f32,
}

#[derive(Default)]
pub struct Focus {
    /// Arrow keys orbit the view when it was the last thing clicked.
    pub view: bool,
    pub tree: bool,
}

pub struct App {
    pub ctx: egui::Context,
    pub exe: PathBuf,
    pub r3: Arc<Mutex<Renderer3d>>,
    pub r2: Arc<Mutex<Renderer2d>>,
    pub settings: Settings,
    pub portable: bool,
    pub mode: Mode,
    pub model: Option<Model3>,
    pub draw: Option<Draw2>,
    pub flow: Flow,
    pub tx: Sender<Event>,
    pub rx: Receiver<Event>,
    pub opts: Opts,
    pub filter: String,
    pub props: Props,
    pub status: Status,
    pub msgs: VecDeque<MsgBox>,
    pub dlg: Dialogs,
    pub meas: Measure3,
    pub meas2: Measure2,
    pub measure: Option<MeasureKind>,
    pub mouse3: Mouse3,
    pub mouse2: Mouse2,
    pub sm: Option<crate::spacemouse_ctl::SpaceMouseCtl>,
    pub hooks: Hooks,
    pub queue: Vec<Action>,
    pub layout: PanelLayout,
    /// Size of the central area in physical pixels and the pixel ratio of the last frame.
    pub view_px: (u32, u32, f64),
    pub focus: Focus,
    pub title: String,
    pub overlays: OverlaySet,
    pub prune_due: Option<Instant>,
    pub load_secs: f64,
    pub drive: Option<crate::drive::DriveRun>,
    /// What the link and sign-out menu entries call (the system, or a fake in tests).
    pub link_host: Box<dyn crate::drive::LinkHost>,
    /// Recent files that still exist, found when the menu opened (the menu is drawn every frame while it is open and
    /// must not ask the file system every time). `None` while the menu is closed.
    pub recent_cache: Option<Vec<String>>,
    pub pending_link: Option<String>,
    /// File from the command line, opened on the first frame.
    pub initial_file: Option<PathBuf>,
    /// Window position, size and maximised state of the last frame.
    pub geometry: Option<(f32, f32, f32, f32, bool)>,
    /// The central area of the last frame in points.
    pub view_rect: egui::Rect,
    /// The settings file (the per-user one; tests use a temporary file).
    pub settings_path: PathBuf,
    /// Folder of material assignments and of the pruning (the cache folder; tests use a temporary one).
    pub data_dir: PathBuf,
    pub frames: u64,
}

impl App {
    pub fn r3(&self) -> MutexGuard<'_, Renderer3d> {
        self.r3.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn r2(&self) -> MutexGuard<'_, Renderer2d> {
        self.r2.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ------------------------------------------------------------------ messages and status

    pub fn warn(&mut self, text: impl Into<String>) {
        self.msgs.push_back(MsgBox { kind: MsgKind::Warning, text: text.into() });
    }

    pub fn critical(&mut self, text: impl Into<String>) {
        self.msgs.push_back(MsgBox { kind: MsgKind::Critical, text: text.into() });
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.msgs.push_back(MsgBox { kind: MsgKind::Info, text: text.into() });
    }

    /// `statusBar().showMessage(text, ms)`.
    pub fn status_msg(&mut self, text: impl Into<String>, millis: u64) {
        self.status.temp = Some((text.into(), Instant::now() + Duration::from_millis(millis)));
    }

    pub fn save_settings(&self) {
        let _ = self.settings.save_to(&self.settings_path);
    }

    /// The 3D display options as the renderer wants them.
    pub fn style3(&self) -> Style {
        let mut s = self.r3().style.clone();
        s.edges = self.opts.edges;
        s.wireframe = self.opts.wire;
        // the dark background goes with the wireframe (`toggle_wire`)
        s.dark_background = self.opts.wire;
        s.scale_bar = self.settings.scale_bar;
        s
    }

    pub fn apply_style3(&mut self) {
        let s = self.style3();
        self.r3().set_style(&s);
    }

    pub fn window_title(&self) -> String {
        match (self.mode, &self.model, &self.draw) {
            (Mode::ThreeD, Some(m), _) => format!("{} – {APP_NAME}", crate::model3::file_label(&m.path, &m.display_name)),
            (Mode::TwoD, _, Some(d)) => format!("{} – {APP_NAME}", crate::model3::file_label(&d.path, &d.display_name)),
            _ => APP_NAME.to_string(),
        }
    }

    /// Both renderers need to know the size of the view before the camera is fitted.
    pub fn sync_viewports(&mut self) {
        let (w, h, dpr) = self.view_px;
        if w > 0 && h > 0 {
            self.r3().set_viewport(w, h, dpr);
            self.r2().set_viewport(w, h, dpr);
        }
    }
}
