//! Creation of the window and the frame loop (`main.main`, `MainWindow.__init__`).
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cad_core::cache;
use cad_core::paths;
use cad_core::settings::Settings;
use cad_core::worker;
use cad_render::{Renderer2d, Renderer3d};
use eframe::egui;
use eframe::egui_wgpu;

use crate::app::{App, Dialogs, Focus, Mode, Opts, OverlaySet, PanelLayout, Status};
use crate::drawing2::Measure2;
use crate::flow::{worker_threshold, Flow, WarmSlot};
use crate::hooks::Hooks;
use crate::measure3::Measure3;
use crate::props::Props;
use crate::spacemouse_ctl::SpaceMouseCtl;
use crate::strings::APP_NAME;
use crate::util::{is_cad, is_drawing};
use crate::view2d::Mouse2;
use crate::view3d::Mouse3;

/// How long the exit waits for a DWG conversion to stop after it was told to (the converter is killed at once; the
/// wait covers the thread that does it and the removal of its folder).
pub const CONVERTER_STOP_WAIT: Duration = Duration::from_secs(3);

/// What the process was started with.
pub struct Init {
    pub file: Option<PathBuf>,
    pub link: Option<String>,
    pub exe: PathBuf,
    /// A worker started with the program for the file on the command line.
    pub early: Option<WarmSlot>,
}

/// Opened with a big 3D file (double click in Explorer): start the load job right now, so the worker's start-up and
/// the file reading run while the window is still being built (`EARLY_WORKER`). Not in portable mode: there the
/// source is copied to the local cache first.
pub fn early_start(exe: &Path, file: &Path) -> Option<WarmSlot> {
    if paths::is_portable() || !is_cad(file) {
        return None;
    }
    let path = std::path::absolute(file).ok()?;
    // a path too long for Open CASCADE is copied first by the normal load
    if crate::flow::needs_local_copy(&path) {
        return None;
    }
    let size = std::fs::metadata(&path).ok()?.len();
    if size < worker_threshold() {
        return None;
    }
    let cache_file = cache::cache_path(&path).ok()?;
    if cache_file.exists() {
        return None;
    }
    let w = worker::spawn(exe).ok()?;
    let tmp = cache::part_path(&cache_file);
    let _ = std::fs::remove_file(worker::log_path(&tmp));
    if worker::submit(&w.job_file, &path, &tmp).is_err() {
        w.discard();
        return None;
    }
    Some(WarmSlot { worker: w, submitted: Some(path) })
}

/// `geometry` of the settings: `x,y,w,h,maximized`.
pub fn parse_geometry(s: &str) -> Option<(f32, f32, f32, f32, bool)> {
    let p: Vec<&str> = s.split(',').collect();
    if p.len() != 5 {
        return None;
    }
    let f = |i: usize| p[i].trim().parse::<f32>().ok().filter(|v| v.is_finite());
    let (x, y, w, h) = (f(0)?, f(1)?, f(2)?, f(3)?);
    if w < 200.0 || h < 150.0 || w > 20000.0 || h > 20000.0 || x.abs() > 40000.0 || y.abs() > 40000.0 {
        return None;
    }
    Some((x, y, w, h, p[4].trim() == "1"))
}

pub fn format_geometry(x: f32, y: f32, w: f32, h: f32, maximized: bool) -> String {
    format!("{x:.0},{y:.0},{w:.0},{h:.0},{}", u8::from(maximized))
}

/// The window icon (`stepview.png`).
pub fn window_icon() -> Option<egui::IconData> {
    const PNG: &[u8] = include_bytes!("../../../../assets/stepview.png");
    let img = image::load_from_memory(PNG).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width, height })
}

/// A program that was killed while it converted a DWG (the task manager, a power failure) leaves its private folders in the
/// temporary folder and next to the cache. Those older than six hours are removed at the start, on a thread of their own:
/// a temporary folder with many files is not looked through while the window is made.
fn remove_stale_conversion_folders() {
    let _ = std::thread::Builder::new().name("stale-temp".into()).spawn(|| {
        let temp = std::env::temp_dir();
        let cache = cad_core::cache::cache_dir();
        // the Explorer thumbnail renderer runs with TEMP in LocalLow\StepViewerThumbs (shellext/Sandbox.cs)
        let thumbs = std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(format!("{}Low", l.to_string_lossy())).join("StepViewerThumbs"));
        let mut dirs: Vec<&std::path::Path> = vec![&temp];
        if let Some(t) = &thumbs {
            dirs.push(t);
        }
        cad_2d::dwg::remove_stale_conversion_dirs_in(&dirs, Some(&cache), cad_2d::dwg::STALE_AGE);
    });
}

/// Runs the window until it is closed.
pub fn run(init: Init) -> Result<(), eframe::Error> {
    remove_stale_conversion_folders();
    // no settings file yet: the settings of the 1.9 version are taken over once (read only)
    let settings = Settings::load_or_import(&paths::settings_file(), crate::legacy::source().as_ref());
    let mut vb = egui::ViewportBuilder::default().with_title(APP_NAME).with_app_id("cad-viewer").with_min_inner_size([640.0, 400.0]).with_drag_and_drop(true);
    // a saved position that is off every monitor (a monitor was unplugged) is moved back to the main one
    let saved = settings.geometry.as_deref().and_then(parse_geometry).map(|g| crate::monitors::fit_to_monitors(g, &crate::monitors::monitor_areas()));
    match saved {
        Some((x, y, w, h, max)) => {
            vb = vb.with_position([x, y]).with_inner_size([w, h]).with_maximized(max);
        }
        None => vb = vb.with_inner_size([1500.0, 900.0]),
    }
    if let Some(icon) = window_icon() {
        vb = vb.with_icon(icon);
    }
    // DX12 on Windows, Vulkan elsewhere, nothing else (ARCHITECTURE.md): probing the OpenGL backend as well made the
    // start take 11 s instead of 4 s. `WGPU_BACKEND` still overrides it.
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    if cad_render::wgpu::Backends::from_env().is_none() {
        setup.instance_descriptor.backends =
            if cfg!(windows) { cad_render::wgpu::Backends::DX12 } else { cad_render::wgpu::Backends::VULKAN };
    }
    let wgpu_options = egui_wgpu::WgpuConfiguration { wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(setup), ..Default::default() };
    let options = eframe::NativeOptions {
        viewport: vb,
        wgpu_options,
        // settings live in `cad_core::paths`; nothing else is persisted, and nothing next to the program
        persist_window: false,
        persistence_path: Some(paths::config_root().join("window.ron")),
        ..Default::default()
    };
    eframe::run_native(APP_NAME, options, Box::new(move |cc| Ok(Box::new(App::new(cc, init, settings)?))))
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, init: Init, settings: Settings) -> Result<App, String> {
        let rs = cc.wgpu_render_state.as_ref().ok_or_else(|| "Grafika (wgpu) není k dispozici.".to_string())?;
        let r3 = Renderer3d::from_render_state(rs).shared();
        let r2 = Renderer2d::from_render_state(rs).shared();
        let ctx = cc.egui_ctx.clone();
        let sm = SpaceMouseCtl::start(&ctx);
        Ok(App::build(ctx, r3, r2, sm, init, settings))
    }

    /// The window state around renderers that already exist (tests make their own).
    pub fn build(
        ctx: egui::Context,
        r3: Arc<Mutex<Renderer3d>>,
        r2: Arc<Mutex<Renderer2d>>,
        sm: Option<SpaceMouseCtl>,
        init: Init,
        settings: Settings,
    ) -> App {
        // MSAA: the environment first, else the key `msaa` of the settings (read only), else 2 (`msaa_samples()`)
        if std::env::var_os("STEPVIEW_MSAA").is_none() {
            if let Some(m) = settings.msaa {
                let samples = cad_render::renderer3d::msaa_from_value(Some(&m.to_string()));
                let mut r = r3.lock().unwrap_or_else(|e| e.into_inner());
                let mut style = r.style.clone();
                style.msaa = samples;
                r.set_style(&style);
            }
        }
        let (tx, rx) = mpsc::channel();
        let mut flow = Flow::new();
        flow.warm = init.early;
        let left_width = settings.split.as_deref().and_then(|s| s.trim().parse::<f32>().ok()).filter(|w| (150.0..=1200.0).contains(w)).unwrap_or(340.0);
        App {
            ctx,
            exe: init.exe,
            r3,
            r2,
            portable: paths::is_portable(),
            settings,
            mode: Mode::Empty,
            model: None,
            draw: None,
            flow,
            tx,
            rx,
            opts: Opts::default(),
            filter: String::new(),
            props: Props::Welcome,
            status: Status::default(),
            msgs: VecDeque::new(),
            dlg: Dialogs::default(),
            meas: Measure3::default(),
            meas2: Measure2::default(),
            measure: None,
            mouse3: Mouse3::default(),
            mouse2: Mouse2::default(),
            sm,
            hooks: Hooks::from_env(),
            queue: Vec::new(),
            layout: PanelLayout { left_width, tree_frac: 0.65 },
            view_px: (800, 600, 1.0),
            focus: Focus::default(),
            title: String::new(),
            overlays: OverlaySet::default(),
            prune_due: Some(Instant::now() + Duration::from_secs(5)),
            load_secs: 0.0,
            drive: None,
            link_host: Box::new(crate::drive::SystemHost),
            recent_cache: None,
            // a link never comes with a file: the link is validated first, and no path is touched before that
            initial_file: if init.link.is_some() { None } else { init.file },
            pending_link: init.link,
            geometry: None,
            view_rect: egui::Rect::NOTHING,
            settings_path: paths::settings_file(),
            data_dir: cache::cache_dir(),
            frames: 0,
        }
    }

    /// Drag and drop: a supported file over the window pre-starts what its loading needs, a drop opens it.
    fn drag_and_drop(&mut self, ctx: &egui::Context) {
        let (hovered, dropped) = ctx.input(|i| {
            (
                i.raw.hovered_files.iter().filter_map(|f| f.path.clone()).collect::<Vec<_>>(),
                i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect::<Vec<_>>(),
            )
        });
        if hovered.iter().any(|p| is_cad(p)) {
            self.prestart_worker();
        }
        if !dropped.is_empty() {
            self.open_dropped(&dropped);
        }
        let _ = is_drawing;
    }

    fn start_pending(&mut self) {
        if let Some(f) = self.initial_file.take() {
            self.open_file(f, None);
        }
        if let Some(url) = self.pending_link.take() {
            self.start_drive_link(&url);
        }
    }

    /// 3Dconnexion: motion for the active view and the buttons (`_space_motion`, `_space_button`).
    fn space_mouse(&mut self, ctx: &egui::Context) {
        let Some(sm) = self.sm.as_mut() else { return };
        let active = ctx.input(|i| i.focused);
        let frame = sm.poll(&self.settings.spacemouse, active, ctx);
        if self.modal_open() {
            return;
        }
        if let Some((v, dt)) = frame.motion {
            match self.mode {
                Mode::ThreeD => self.space_motion_3d(v, dt),
                Mode::TwoD => self.space_motion_2d(v, dt),
                Mode::Empty => {}
            }
        }
        for a in frame.actions {
            if a == "fit" {
                self.fit();
            } else if self.mode == Mode::ThreeD {
                match a {
                    "roll_cw" => self.r3().camera.roll(-std::f64::consts::FRAC_PI_2),
                    "roll_ccw" => self.r3().camera.roll(std::f64::consts::FRAC_PI_2),
                    other => {
                        if let Some(v) = cad_render::StdView::from_key(other) {
                            self.r3().set_orientation(v);
                        }
                    }
                }
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frames += 1;
        self.pump_events();
        self.poll_worker();
        self.maybe_prune();
        if self.frames == 1 {
            self.start_pending();
        }
        self.drag_and_drop(&ctx);
        self.tick_layer_redraw();
        self.poll_gpu_warnings();
        self.space_mouse(&ctx);
        // keyboard shortcuts of the window
        if !self.modal_open() {
            let events = ctx.input(|i| i.events.clone());
            let mut shortcuts = crate::keys::actions_in(&events, ctx.egui_wants_keyboard_input(), self.mode);
            if self.dlg.context.is_some() {
                // Esc closes an open menu and does nothing else
                shortcuts.retain(|a| *a != crate::app::Action::Escape);
            }
            self.queue.extend(shortcuts);
            self.tree_keys(&ctx);
        }

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(2.0);
            self.show_toolbar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(1.0);
            self.show_status(ui);
        });
        let left = egui::Panel::left("left")
            .resizable(true)
            .default_size(self.layout.left_width)
            .size_range(160.0..=900.0)
            .show(ui, |ui| self.show_left(ui));
        self.layout.left_width = left.response.rect.width();
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| match self.mode {
            Mode::Empty => self.show_empty(ui),
            Mode::ThreeD => self.show_view3d(ui),
            Mode::TwoD => self.show_view2d(ui),
        });

        // dialogs
        self.show_collisions(&ctx);
        self.show_context_menu(&ctx);
        self.show_material_dialog(&ctx);
        self.show_spacemouse_dialog(&ctx);
        self.show_thumb_dialog(&ctx);
        self.show_about(&ctx);
        self.show_messages(&ctx);

        self.run_queue(frame);
        if !self.queue.is_empty() {
            ctx.request_repaint();
        }
        let title = self.window_title();
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        self.remember_geometry(&ctx);
        self.run_hooks(&ctx);
    }

    fn on_exit(&mut self) {
        // a thread that cannot be stopped does not hold the program back: the process ends when this returns
        self.drop_warm();
        self.settings.split = Some(format!("{:.0}", self.layout.left_width));
        if let Some(g) = self.geometry.take() {
            self.settings.geometry = Some(format_geometry(g.0, g.1, g.2, g.3, g.4));
        }
        self.save_settings();
        if let Some(run) = self.flow.run.take() {
            let mut run = run;
            let _ = run.child.kill();
            let _ = run.child.wait();
            worker::remove_tmp(&run.tmp);
            let _ = std::fs::remove_file(&run.job);
        }
        if let Some(d) = self.drive.take() {
            d.cancel.cancel();
        }
        // a DWG conversion still running is not left behind as an orphan process
        self.stop_drawing_work(CONVERTER_STOP_WAIT);
    }

    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }
}

impl App {
    /// Remembers the window position and size for the next start (`saveGeometry`).
    fn remember_geometry(&mut self, ctx: &egui::Context) {
        let (outer, inner, maximized) = ctx.input(|i| {
            let v = i.viewport();
            (v.outer_rect, v.inner_rect, v.maximized.unwrap_or(false))
        });
        if let (Some(o), Some(inn)) = (outer, inner) {
            if !maximized && inn.width() > 200.0 && inn.height() > 150.0 {
                // the inner size is what `with_inner_size` takes back
                self.geometry = Some((o.min.x, o.min.y, inn.width(), inn.height(), false));
            } else if maximized {
                let (x, y, w, h) = match self.geometry {
                    Some((x, y, w, h, _)) => (x, y, w, h),
                    None => (o.min.x, o.min.y, inn.width(), inn.height()),
                };
                self.geometry = Some((x, y, w, h, true));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_round_trip_and_bad_values() {
        let s = format_geometry(100.4, 50.6, 1500.0, 900.0, false);
        assert_eq!(s, "100,51,1500,900,0");
        assert_eq!(parse_geometry(&s), Some((100.0, 51.0, 1500.0, 900.0, false)));
        assert_eq!(parse_geometry("0,0,1200,800,1"), Some((0.0, 0.0, 1200.0, 800.0, true)));
        assert_eq!(parse_geometry("0,0,10,10,0"), None);
        assert_eq!(parse_geometry("a,b,c,d,e"), None);
        assert_eq!(parse_geometry("1,2,3"), None);
        assert_eq!(parse_geometry("0,0,99999,800,0"), None);
        // negative positions are fine (monitors left of the main one)
        assert_eq!(parse_geometry("-1900,10,1200,800,0"), Some((-1900.0, 10.0, 1200.0, 800.0, false)));
    }

    #[test]
    fn the_window_icon_decodes() {
        let icon = window_icon().expect("icon");
        assert!(icon.width > 0 && icon.height > 0);
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
    }

    #[test]
    fn early_start_needs_a_big_model_without_a_cache() {
        let dir = tempfile::tempdir().unwrap();
        let small = dir.path().join("small.step");
        std::fs::write(&small, b"x").unwrap();
        assert!(early_start(Path::new("worker.exe"), &small).is_none());
        // drawings never start a worker
        let dxf = dir.path().join("a.dxf");
        std::fs::write(&dxf, vec![0u8; 2 * 1024 * 1024]).unwrap();
        assert!(early_start(Path::new("worker.exe"), &dxf).is_none());
        // a missing file gives nothing
        assert!(early_start(Path::new("worker.exe"), &dir.path().join("none.step")).is_none());
    }
}
