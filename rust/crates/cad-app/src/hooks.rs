//! Testing hooks of the Python viewer, kept as they were:
//! * `STEPVIEW_SHOT=<png>`: save a picture of the window and quit;
//! * `STEPVIEW_SHOT_DELAY=<ms>`: wait this long first (default 4000 ms);
//! * `STEPVIEW_SELECT=<text>`: select the first tree item whose name contains the text before the picture is taken.
//!
//! While a load is running the picture waits (re-checked every 500 ms); a selection gets 800 ms to show.
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::app::App;

pub const ENV_SHOT: &str = "STEPVIEW_SHOT";
pub const ENV_DELAY: &str = "STEPVIEW_SHOT_DELAY";
pub const ENV_SELECT: &str = "STEPVIEW_SELECT";
pub const DEFAULT_DELAY_MS: u64 = 4000;
pub const BUSY_RETRY: Duration = Duration::from_millis(500);
pub const SELECT_WAIT: Duration = Duration::from_millis(800);

/// State of the screenshot: 0 = not asked, 1 = asked, 2 = written, 3 = failed.
pub struct Hooks {
    shot: Option<PathBuf>,
    select: Option<String>,
    next_check: Instant,
    picked: bool,
    state: Arc<AtomicU8>,
}

/// Delay from the text of `STEPVIEW_SHOT_DELAY`; anything but a number gives the default.
pub fn parse_delay(v: Option<&str>) -> Duration {
    Duration::from_millis(v.and_then(|s| s.trim().parse().ok()).unwrap_or(DEFAULT_DELAY_MS))
}

impl Hooks {
    pub fn from_env() -> Hooks {
        let shot = std::env::var_os(ENV_SHOT).filter(|v| !v.is_empty()).map(PathBuf::from);
        let delay = parse_delay(std::env::var(ENV_DELAY).ok().as_deref());
        Hooks {
            shot,
            select: std::env::var(ENV_SELECT).ok().filter(|s| !s.is_empty()),
            next_check: Instant::now() + delay,
            picked: false,
            state: Arc::new(AtomicU8::new(0)),
        }
    }

    pub fn active(&self) -> bool {
        self.shot.is_some()
    }
}

/// RGBA bytes of a screenshot (the window is opaque, so premultiplied and straight colours agree).
pub fn rgba_bytes(img: &egui::ColorImage) -> Vec<u8> {
    img.pixels.iter().flat_map(|p| p.to_array()).collect()
}

impl App {
    /// Called every frame: drives the screenshot hook.
    pub fn run_hooks(&mut self, ctx: &egui::Context) {
        if !self.hooks.active() {
            return;
        }
        match self.hooks.state.load(Ordering::SeqCst) {
            2 | 3 => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            1 => {
                ctx.request_repaint_after(Duration::from_millis(50));
                return;
            }
            _ => {}
        }
        let now = Instant::now();
        if now < self.hooks.next_check {
            ctx.request_repaint_after(self.hooks.next_check - now);
            return;
        }
        if self.flow.active() || self.frames < 3 {
            self.hooks.next_check = now + BUSY_RETRY;
            ctx.request_repaint_after(BUSY_RETRY);
            return;
        }
        if let (Some(pick), false) = (self.hooks.select.clone(), self.hooks.picked) {
            self.hooks.picked = true;
            self.select_by_name(&pick);
            self.hooks.next_check = now + SELECT_WAIT;
            ctx.request_repaint_after(SELECT_WAIT);
            return;
        }
        let Some(path) = self.hooks.shot.clone() else { return };
        self.hooks.state.store(1, Ordering::SeqCst);
        let state = self.hooks.state.clone();
        ctx.request_screenshot(move |img| {
            let ok = cad_render::capture::png_from_rgba(img.width() as u32, img.height() as u32, rgba_bytes(&img))
                .ok()
                .and_then(|png| std::fs::write(&path, png).ok())
                .is_some();
            state.store(if ok { 2 } else { 3 }, Ordering::SeqCst);
        });
        ctx.request_repaint();
    }

    /// `STEPVIEW_SELECT`: the first node whose name contains the text, selected like a click in the tree.
    pub fn select_by_name(&mut self, text: &str) {
        let t = text.to_lowercase();
        let Some(m) = self.model.as_mut() else { return };
        let found = m.pre_order().into_iter().find(|id| m.node(*id).name.to_lowercase().contains(&t));
        if let Some(id) = found {
            m.select_only([id]);
            m.anchor = Some(id);
            m.expand_ancestors(id);
            m.scroll_to = Some(id);
            m.rows_dirty = true;
            self.on_tree_selection();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_defaults_to_four_seconds() {
        assert_eq!(parse_delay(None), Duration::from_millis(4000));
        assert_eq!(parse_delay(Some("")), Duration::from_millis(4000));
        assert_eq!(parse_delay(Some("abc")), Duration::from_millis(4000));
        assert_eq!(parse_delay(Some(" 1500 ")), Duration::from_millis(1500));
        assert_eq!(parse_delay(Some("0")), Duration::from_millis(0));
    }

    #[test]
    fn screenshot_pixels_become_rgba_bytes() {
        let img = egui::ColorImage::new([2, 1], vec![egui::Color32::from_rgb(1, 2, 3), egui::Color32::from_rgb(4, 5, 6)]);
        assert_eq!(rgba_bytes(&img), vec![1, 2, 3, 255, 4, 5, 6, 255]);
    }
}
