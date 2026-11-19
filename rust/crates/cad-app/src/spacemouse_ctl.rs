//! The window's side of the 3D mouse: turns the device state into motion for the views and button presses into
//! view commands, on Windows. Other systems have no listener and no 3D mouse.
use std::time::Instant;

use cad_core::settings::SpaceMouse as Config;
use eframe::egui;

use crate::spacemouse::{levels, shaped, tick_dt};

#[cfg(windows)]
pub struct SpaceMouseCtl {
    shared: std::sync::Arc<std::sync::Mutex<crate::spacemouse_win::Shared>>,
    last_tick: Option<Instant>,
    was_active: bool,
}

#[cfg(not(windows))]
pub struct SpaceMouseCtl {
    _unused: (),
}

/// What a frame got from the device.
#[derive(Default)]
pub struct Frame {
    /// `(v, dt)` for the active view.
    pub motion: Option<([f64; 6], f64)>,
    /// Actions of the buttons: `fit`, `top`, `left`, ..., `roll_cw`, `roll_ccw`, `iso`.
    pub actions: Vec<&'static str>,
}

impl SpaceMouseCtl {
    #[cfg(windows)]
    pub fn start(ctx: &egui::Context) -> Option<SpaceMouseCtl> {
        crate::spacemouse_win::start(ctx).map(|shared| SpaceMouseCtl { shared, last_tick: None, was_active: false })
    }

    #[cfg(not(windows))]
    pub fn start(_ctx: &egui::Context) -> Option<SpaceMouseCtl> {
        None
    }

    #[cfg(windows)]
    fn lock(&self) -> std::sync::MutexGuard<'_, crate::spacemouse_win::Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Deflection per axis for the settings dialog.
    #[cfg(windows)]
    pub fn levels(&self) -> [f64; 6] {
        levels(&self.lock().state.hid)
    }

    #[cfg(not(windows))]
    pub fn levels(&self) -> [f64; 6] {
        levels(&[0; 6])
    }

    #[cfg(windows)]
    pub fn last_button(&self) -> Option<u32> {
        self.lock().state.last_button
    }

    #[cfg(not(windows))]
    pub fn last_button(&self) -> Option<u32> {
        None
    }

    /// `reset`: no motion, no buttons (the window lost the focus, or the 3D mouse was switched off).
    #[cfg(windows)]
    pub fn reset(&mut self) {
        let mut s = self.lock();
        s.state.hid = [0; 6];
        s.state.buttons = 0;
        s.actions.clear();
        drop(s);
        self.last_tick = None;
    }

    #[cfg(not(windows))]
    pub fn reset(&mut self) {}

    /// Called every frame: tells the listener whether the window is active and takes motion and buttons.
    #[cfg(windows)]
    pub fn poll(&mut self, cfg: &Config, active: bool, ctx: &egui::Context) -> Frame {
        {
            let mut s = self.lock();
            s.active = active;
            s.enabled = cfg.enabled;
        }
        if !active && self.was_active {
            // no WM_INPUT reaches an inactive window: the zero report gets lost
            self.reset();
        }
        self.was_active = active;
        let mut out = Frame::default();
        let (hid, actions) = {
            let mut s = self.lock();
            (s.state.hid, std::mem::take(&mut s.actions))
        };
        if cfg.enabled {
            out.actions = actions;
        }
        let now = Instant::now();
        if cfg.enabled && hid.iter().any(|v| *v != 0) {
            let dt = tick_dt(self.last_tick.map(|t| now.duration_since(t).as_secs_f64()).unwrap_or(crate::spacemouse::TICK_SECS));
            self.last_tick = Some(now);
            let v = shaped(&hid, cfg);
            if v.iter().any(|x| *x != 0.0) {
                out.motion = Some((v, dt));
            }
            // the cap is held: keep the frames coming
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(crate::spacemouse::TICK_SECS));
        } else {
            self.last_tick = None;
        }
        out
    }

    #[cfg(not(windows))]
    pub fn poll(&mut self, _cfg: &Config, _active: bool, _ctx: &egui::Context) -> Frame {
        Frame::default()
    }
}
