//! Pan and zoom maths of the 2D drawing view and the ruler ticks (SPEC section 10, `DrawingView` and `Ruler`).
//!
//! World coordinates are drawing units with +Y up; pixels are physical with the origin in the upper left and y down.
use crate::camera::Viewport;
use crate::scalebar::nice_step;

/// Margin of `fit`: 3 % of the larger side on each side of the content.
pub const FIT_MARGIN: f64 = 0.03;
/// Margin of `zoom_to_items`: 20 % of the larger side (plus 1e-6).
pub const ZOOM_ITEMS_MARGIN: f64 = 0.2;
/// 3D mouse: pan speed in px/s and zoom in e-folds/s.
pub const SM_PAN: f64 = 1100.0;
pub const SM_ZOOM: f64 = 2.2;
/// Ruler thickness in pixels and the cap on ticks (`THICK = 24`, `cnt < 3000`).
pub const RULER_THICKNESS: f64 = 24.0;
pub const RULER_MAX_TICKS: usize = 3000;

/// Axis-aligned rectangle `(min, max)` in drawing units.
pub type Rect2 = ([f64; 2], [f64; 2]);

/// View onto the drawing: the world point at the centre of the viewport and the zoom in pixels per drawing unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View2d {
    pub center: [f64; 2],
    pub scale: f64,
}

impl Default for View2d {
    fn default() -> Self {
        Self { center: [0.0, 0.0], scale: 1.0 }
    }
}

fn is_valid(r: &Rect2) -> bool {
    r.0.iter().chain(r.1.iter()).all(|v| v.is_finite()) && r.0[0] <= r.1[0] && r.0[1] <= r.1[1]
}

impl View2d {
    /// Pixel of a world point.
    pub fn to_screen(&self, p: [f64; 2], vp: Viewport) -> [f64; 2] {
        [(p[0] - self.center[0]) * self.scale + vp.width / 2.0, vp.height / 2.0 - (p[1] - self.center[1]) * self.scale]
    }

    /// World point under a pixel.
    pub fn to_world(&self, px: [f64; 2], vp: Viewport) -> [f64; 2] {
        [(px[0] - vp.width / 2.0) / self.scale + self.center[0], (vp.height / 2.0 - px[1]) / self.scale + self.center[1]]
    }

    /// Drawing units per pixel.
    pub fn units_per_pixel(&self) -> f64 {
        1.0 / self.scale
    }

    /// Visible rectangle in drawing units.
    pub fn view_rect(&self, vp: Viewport) -> Rect2 {
        let a = self.to_world([0.0, vp.height], vp);
        let b = self.to_world([vp.width, 0.0], vp);
        (a, b)
    }

    /// Fits a rectangle (with `margin` as a fraction of its larger side on each side) keeping the aspect ratio.
    pub fn fit_rect(&mut self, r: &Rect2, margin: f64, vp: Viewport) {
        if !is_valid(r) {
            return;
        }
        let (w, h) = (r.1[0] - r.0[0], r.1[1] - r.0[1]);
        let m = w.max(h) * margin;
        let (fw, fh) = (w + 2.0 * m, h + 2.0 * m);
        if fw <= 0.0 || fh <= 0.0 {
            return;
        }
        self.center = [(r.0[0] + r.1[0]) / 2.0, (r.0[1] + r.1[1]) / 2.0];
        self.scale = (vp.width / fw).min(vp.height / fh);
    }

    /// `fit`: the content with the 3 % margin; empty content leaves the view alone.
    pub fn fit(&mut self, content: &Rect2, vp: Viewport) {
        let (w, h) = (content.1[0] - content.0[0], content.1[1] - content.0[1]);
        if w <= 0.0 && h <= 0.0 {
            return;
        }
        self.fit_rect(content, FIT_MARGIN, vp);
    }

    /// `zoom_to_items`: a rectangle with 20 % margin (+ 1e-6).
    pub fn zoom_to_rect(&mut self, r: &Rect2, vp: Viewport) {
        if !is_valid(r) {
            return;
        }
        let (w, h) = (r.1[0] - r.0[0], r.1[1] - r.0[1]);
        if w <= 0.0 && h <= 0.0 {
            return;
        }
        let m = w.max(h) * ZOOM_ITEMS_MARGIN + 1e-6;
        let padded = ([r.0[0] - m, r.0[1] - m], [r.1[0] + m, r.1[1] + m]);
        self.fit_rect(&padded, 0.0, vp);
    }

    /// Content follows the cursor by `dx`, `dy` pixels (right, down).
    pub fn pan_pixels(&mut self, dx: f64, dy: f64) {
        self.center[0] -= dx / self.scale;
        self.center[1] += dy / self.scale;
    }

    /// Zoom by `factor` keeping the point under the pixel fixed (`AnchorUnderMouse`).
    pub fn zoom_at(&mut self, px: [f64; 2], factor: f64, vp: Viewport) {
        if !(factor.is_finite() && factor > 0.0) {
            return;
        }
        let before = self.to_world(px, vp);
        self.scale *= factor;
        let after = self.to_world(px, vp);
        self.center[0] += before[0] - after[0];
        self.center[1] += before[1] - after[1];
    }

    /// Zoom about the centre of the view (3D mouse).
    pub fn zoom_center(&mut self, factor: f64) {
        if factor.is_finite() && factor > 0.0 {
            self.scale *= factor;
        }
    }

    /// 3D mouse: push = pan, pull = zoom, rotations unused. `frac` keeps the sub-pixel remainder of the pan between
    /// calls (scroll bars were whole pixels). Returns the new remainder.
    pub fn space_motion(&mut self, v: [f64; 6], dt: f64, frac: [f64; 2]) -> [f64; 2] {
        let (tx, ty, tz) = (v[0], v[1], v[2]);
        if tz != 0.0 {
            self.zoom_center((tz * SM_ZOOM * dt).exp());
        }
        let mut rem = frac;
        if tx != 0.0 || ty != 0.0 {
            let fx = frac[0] - tx * SM_PAN * dt;
            let fy = frac[1] + ty * SM_PAN * dt;
            let (ix, iy) = (fx.trunc(), fy.trunc());
            rem = [fx - ix, fy - iy];
            // the Python code adds to the scroll bar values: +ix scrolls the content left
            self.pan_pixels(-ix, iy);
        }
        rem
    }
}

/// One tick of a ruler.
#[derive(Clone, Debug, PartialEq)]
pub struct RulerTick {
    /// Position along the ruler in pixels.
    pub pos: f64,
    pub major: bool,
    /// Label of major ticks (decimal comma).
    pub label: Option<String>,
}

/// Tick length as a fraction of the ruler thickness.
pub fn ruler_tick_length(major: bool) -> f64 {
    if major { 0.55 } else { 0.22 }
}

/// Ticks of a ruler spanning `a` (at pixel 0) to `b` (at pixel `n`) in drawing units; `a > b` is fine (vertical rulers).
pub fn ruler_ticks(a: f64, b: f64, n: f64) -> Vec<RulerTick> {
    if n <= 0.0 || (b - a).abs() < 1e-12 || !(a.is_finite() && b.is_finite()) {
        return Vec::new();
    }
    let upp = (b - a).abs() / n;
    let step = nice_step(75.0 * upp);
    let exp = step.log10().floor();
    let mant = (step / 10f64.powf(exp)).round();
    let minor = step / if mant == 2.0 { 4.0 } else { 5.0 };
    let dec = (-exp).max(0.0) as usize;
    let (lo, hi) = (a.min(b), a.max(b));
    let mut v = (lo / minor).floor() * minor;
    let mut out = Vec::new();
    let mut cnt = 0usize;
    while v <= hi && cnt < RULER_MAX_TICKS {
        cnt += 1;
        let pos = (v - a) / (b - a) * n;
        let k = v / step;
        let major = (k - k.round()).abs() < 1e-6;
        let label = major.then(|| {
            let mut txt = format!("{v:.dec$}");
            if txt.parse::<f64>().map(|x| x == 0.0).unwrap_or(false) {
                txt = txt.trim_start_matches('-').to_string();
            }
            txt.replace('.', ",")
        });
        out.push(RulerTick { pos, major, label });
        v += minor;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp() -> Viewport {
        Viewport::new(800.0, 600.0)
    }

    #[test]
    fn screen_world_roundtrip_and_y_flip() {
        let v = View2d { center: [10.0, 20.0], scale: 4.0 };
        assert_eq!(v.to_screen([10.0, 20.0], vp()), [400.0, 300.0]);
        // +Y is up on the screen
        assert!(v.to_screen([10.0, 21.0], vp())[1] < 300.0);
        let p = [13.5, 17.25];
        let back = v.to_world(v.to_screen(p, vp()), vp());
        assert!((back[0] - p[0]).abs() < 1e-12 && (back[1] - p[1]).abs() < 1e-12);
        let r = v.view_rect(vp());
        assert!((r.0[0] - (10.0 - 100.0)).abs() < 1e-12 && (r.1[1] - (20.0 + 75.0)).abs() < 1e-12);
    }

    #[test]
    fn fit_uses_three_percent_margin_and_keeps_aspect() {
        let mut v = View2d::default();
        v.fit(&([0.0, 0.0], [100.0, 50.0]), vp());
        // margin 3 on each side of the larger side (100): 106 x 56 -> scale limited by the width
        assert!((v.scale - 800.0 / 106.0).abs() < 1e-9, "{}", v.scale);
        assert_eq!(v.center, [50.0, 25.0]);
        // the content fits inside the viewport
        let a = v.to_screen([0.0, 0.0], vp());
        let b = v.to_screen([100.0, 50.0], vp());
        assert!(a[0] > 0.0 && b[0] < 800.0 && b[1] > 0.0 && a[1] < 600.0);
        // tall content is limited by the height
        let mut t = View2d::default();
        t.fit(&([0.0, 0.0], [10.0, 200.0]), vp());
        assert!((t.scale - 600.0 / 212.0).abs() < 1e-9);
        // empty or invalid content leaves the view alone
        let before = t;
        t.fit(&([5.0, 5.0], [5.0, 5.0]), vp());
        t.fit(&([f64::NAN, 0.0], [1.0, 1.0]), vp());
        t.fit(&([2.0, 2.0], [1.0, 1.0]), vp());
        assert_eq!(t, before);
    }

    #[test]
    fn zoom_to_rect_uses_twenty_percent() {
        let mut v = View2d::default();
        v.zoom_to_rect(&([0.0, 0.0], [10.0, 10.0]), vp());
        let m = 10.0 * 0.2 + 1e-6;
        assert!((v.scale - 600.0 / (10.0 + 2.0 * m)).abs() < 1e-9);
        assert_eq!(v.center, [5.0, 5.0]);
        // a single point (zero size) is ignored like an empty rectangle
        let before = v;
        v.zoom_to_rect(&([1.0, 1.0], [1.0, 1.0]), vp());
        assert_eq!(v, before);
    }

    #[test]
    fn wheel_zoom_keeps_the_cursor_point() {
        let mut v = View2d { center: [3.0, -4.0], scale: 2.5 };
        let px = [650.0, 120.0];
        let w0 = v.to_world(px, vp());
        v.zoom_at(px, 1.2, vp());
        assert!((v.scale - 3.0).abs() < 1e-12);
        let w1 = v.to_world(px, vp());
        assert!((w0[0] - w1[0]).abs() < 1e-9 && (w0[1] - w1[1]).abs() < 1e-9);
        v.zoom_at(px, 0.0, vp());
        v.zoom_at(px, f64::NAN, vp());
        assert!((v.scale - 3.0).abs() < 1e-12);
    }

    #[test]
    fn pan_moves_the_content_with_the_cursor() {
        let mut v = View2d { center: [0.0, 0.0], scale: 2.0 };
        let p = [5.0, 5.0];
        let s0 = v.to_screen(p, vp());
        v.pan_pixels(30.0, -10.0);
        let s1 = v.to_screen(p, vp());
        assert!((s1[0] - s0[0] - 30.0).abs() < 1e-9 && (s1[1] - s0[1] + 10.0).abs() < 1e-9);
    }

    #[test]
    fn space_motion_pans_and_zooms() {
        let mut v = View2d { center: [0.0, 0.0], scale: 1.0 };
        let rem = v.space_motion([0.0, 0.0, 1.0, 0.0, 0.0, 0.0], 0.1, [0.0, 0.0]);
        assert!((v.scale - (SM_ZOOM * 0.1).exp()).abs() < 1e-12);
        assert_eq!(rem, [0.0, 0.0]);
        // push right: the drawing follows the cap (moves right on screen)
        let mut w = View2d { center: [0.0, 0.0], scale: 1.0 };
        let p = [0.0, 0.0];
        let s0 = w.to_screen(p, vp());
        let mut frac = [0.0, 0.0];
        for _ in 0..10 {
            frac = w.space_motion([1.0, 0.0, 0.0, 0.0, 0.0, 0.0], 0.01, frac);
        }
        let s1 = w.to_screen(p, vp());
        // 10 steps of 0.01 s at 1100 px/s = 110 px, with the remainder carried, whole pixels only
        assert!((s0[0] - s1[0]).abs() > 100.0 && (s0[0] - s1[0]).abs() <= 110.0, "{}", s1[0] - s0[0]);
        assert!(frac[0].abs() < 1.0);
    }

    #[test]
    fn ruler_steps_follow_nice_numbers() {
        // 1000 units over 1000 px: step nice(75) = 100, minor 20
        let t = ruler_ticks(0.0, 1000.0, 1000.0);
        let majors: Vec<&RulerTick> = t.iter().filter(|x| x.major).collect();
        assert_eq!(majors.len(), 11);
        assert_eq!(majors[0].label.as_deref(), Some("0"));
        assert_eq!(majors[1].label.as_deref(), Some("100"));
        assert_eq!(t.len(), 51);
        assert!((t[1].pos - 20.0).abs() < 1e-9);
        // mantissa 2: step 0.2 with minor 0.05 and one decimal in the labels ("0,0", like f"{v:.1f}")
        let t = ruler_ticks(0.0, 1.0, 375.0);
        let majors: Vec<String> = t.iter().filter(|x| x.major).filter_map(|x| x.label.clone()).collect();
        assert_eq!(&majors[..5], ["0,0", "0,2", "0,4", "0,6", "0,8"]);
        let minors = t.iter().take_while(|x| x.pos < 375.0 * 0.2 - 1e-6).count();
        assert_eq!(minors, 4, "three minor ticks between majors plus the first major");
    }

    #[test]
    fn ruler_labels_use_a_comma_and_no_negative_zero() {
        let t = ruler_ticks(-10.0, 10.0, 400.0);
        let labels: Vec<String> = t.iter().filter_map(|x| x.label.clone()).collect();
        assert!(labels.contains(&"0".to_string()), "{labels:?}");
        assert!(!labels.iter().any(|l| l == "-0"));
        assert!(labels.iter().any(|l| l.starts_with('-')));
        let small = ruler_ticks(0.0, 0.05, 400.0);
        let labels: Vec<String> = small.iter().filter_map(|x| x.label.clone()).collect();
        assert!(labels.iter().any(|l| l.contains(',')), "{labels:?}");
        assert!(labels.iter().all(|l| !l.contains('.')));
    }

    #[test]
    fn vertical_rulers_may_run_backwards() {
        // y axis points up while pixels go down: a = top value > b
        let t = ruler_ticks(100.0, 0.0, 100.0);
        assert!(!t.is_empty());
        assert!(t.iter().all(|x| x.pos >= -1e-9 && x.pos <= 100.0 + 1e-9));
        // values grow while the pixel position shrinks
        assert!(t.windows(2).all(|w| w[1].pos < w[0].pos));
    }

    #[test]
    fn ruler_degenerate_input_and_cap() {
        assert!(ruler_ticks(0.0, 0.0, 100.0).is_empty());
        assert!(ruler_ticks(0.0, 10.0, 0.0).is_empty());
        assert!(ruler_ticks(f64::NAN, 10.0, 100.0).is_empty());
        let huge = ruler_ticks(0.0, 1e12, 1.0);
        assert!(huge.len() <= RULER_MAX_TICKS);
        assert!((ruler_tick_length(true) - 0.55).abs() < 1e-12 && (ruler_tick_length(false) - 0.22).abs() < 1e-12);
    }
}
