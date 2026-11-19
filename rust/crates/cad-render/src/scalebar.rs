//! Scale bar ("pravítko") of the orthographic 3D view: rounding, labels and tick geometry (`_update_scale_bar`).
use crate::camera::{Camera, Viewport};
use crate::colors::{Rgb8, SCALE_BAR_DARK_BG, SCALE_BAR_LIGHT_BG};

/// Target length of the bar in logical pixels.
pub const TARGET_PX: f64 = 110.0;
/// Offset of the lower left corner of the bar from the viewport corner, logical pixels.
pub const OFFSET_PX: [f64; 2] = [24.0, 22.0];
/// Label text height, logical pixels.
pub const LABEL_PX: f64 = 13.0;
/// Line width of the bar, pixels.
pub const LINE_WIDTH: f64 = 1.6;

/// 1 / 2 / 5 x 10^n not smaller than `raw` (`nice_step`).
pub fn nice_step(raw: f64) -> f64 {
    if !(raw > 0.0) || !raw.is_finite() {
        return 1.0;
    }
    let e = 10f64.powf(raw.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if m * e >= raw - 1e-12 {
            return m * e;
        }
    }
    10.0 * e
}

/// Python's `format(v, ".{prec}g")`.
pub fn format_g(v: f64, prec: usize) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    if !v.is_finite() {
        return if v.is_nan() { "nan".into() } else if v > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let prec = prec.max(1);
    let sci = format!("{:.*e}", prec - 1, v);
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let e: i32 = exp.parse().unwrap_or(0);
    if e < -4 || e >= prec as i32 {
        let mut m = mant.to_string();
        if m.contains('.') {
            m = m.trim_end_matches('0').trim_end_matches('.').to_string();
        }
        let sign = if e < 0 { '-' } else { '+' };
        format!("{m}e{sign}{:02}", e.abs())
    } else {
        let decimals = (prec as i32 - 1 - e).max(0) as usize;
        let mut s = format!("{v:.decimals$}");
        if s.contains('.') {
            s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        }
        s
    }
}

/// `fmt_val`: thousands separated by a space, decimal comma, six significant digits below 1000.
pub fn fmt_val(v: f64) -> String {
    let s = if v.abs() >= 1000.0 {
        let digits = format!("{:.0}", v.abs());
        let mut out = String::new();
        for (i, ch) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                out.push(' ');
            }
            out.push(ch);
        }
        if v < 0.0 { format!("-{out}") } else { out }
    } else {
        format_g(v, 6)
    };
    s.replace('.', ",")
}

/// Everything needed to draw the bar: segments in pixels relative to its lower left corner (y up).
#[derive(Clone, Debug, PartialEq)]
pub struct ScaleBar {
    /// Model length represented by the bar, mm.
    pub length: f64,
    /// Length of the bar in physical pixels.
    pub length_px: f64,
    /// `"<value> mm"`.
    pub label: String,
    /// Tick geometry: (from, to) in physical pixels relative to the lower left corner, y up.
    pub segments: Vec<([f64; 2], [f64; 2])>,
    /// Anchor of the label (centre, bottom) relative to the lower left corner.
    pub label_anchor: [f64; 2],
    pub label_height_px: f64,
    /// Offset of the lower left corner from the viewport's lower left corner, physical pixels.
    pub offset_px: [f64; 2],
    pub line_width_px: f64,
    pub color: Rgb8,
}

/// Scale bar for an orthographic camera; `None` in perspective or for an empty viewport.
pub fn scale_bar(camera: &Camera, vp: Viewport, dpr: f64, dark_background: bool) -> Option<ScaleBar> {
    if camera.perspective || !(camera.scale > 0.0) {
        return None;
    }
    let h = vp.min_side();
    if h <= 0.0 {
        return None;
    }
    let s = dpr;
    let upp = camera.scale / h;
    let length = nice_step(TARGET_PX * s * upp);
    let l = length / upp;
    let tk = 7.0 * s;
    let mut segs = vec![
        ([0.0, 0.0], [l, 0.0]),
        ([0.0, 0.0], [0.0, tk]),
        ([l, 0.0], [l, tk]),
        ([l / 2.0, 0.0], [l / 2.0, tk * 0.6]),
    ];
    for i in 1..10 {
        if i != 5 {
            let x = l * f64::from(i) / 10.0;
            segs.push(([x, 0.0], [x, tk * 0.35]));
        }
    }
    Some(ScaleBar {
        length,
        length_px: l,
        label: format!("{} mm", fmt_val(length)),
        segments: segs,
        label_anchor: [l / 2.0, tk + 3.0 * s],
        label_height_px: LABEL_PX * s,
        offset_px: [OFFSET_PX[0] * s, OFFSET_PX[1] * s],
        line_width_px: LINE_WIDTH,
        color: if dark_background { SCALE_BAR_DARK_BG } else { SCALE_BAR_LIGHT_BG },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_step_rounds_up_to_1_2_5() {
        assert_eq!(nice_step(0.3), 0.5);
        assert_eq!(nice_step(1.0), 1.0);
        assert_eq!(nice_step(1.01), 2.0);
        assert_eq!(nice_step(2.0), 2.0);
        assert_eq!(nice_step(2.1), 5.0);
        assert_eq!(nice_step(5.5), 10.0);
        assert_eq!(nice_step(73.0), 100.0);
        assert_eq!(nice_step(110.0), 200.0);
        assert_eq!(nice_step(0.0), 1.0);
        assert_eq!(nice_step(-3.0), 1.0);
        assert_eq!(nice_step(f64::NAN), 1.0);
        let v = nice_step(0.00037);
        assert!((v - 0.0005).abs() < 1e-15, "{v}");
    }

    #[test]
    fn fmt_val_matches_python() {
        assert_eq!(fmt_val(5000.0), "5 000");
        assert_eq!(fmt_val(1234567.0), "1 234 567");
        assert_eq!(fmt_val(-2500.0), "-2 500");
        assert_eq!(fmt_val(1000.0), "1 000");
        assert_eq!(fmt_val(12.5), "12,5");
        assert_eq!(fmt_val(0.25), "0,25");
        assert_eq!(fmt_val(100.0), "100");
        assert_eq!(fmt_val(999.9999999), "1000");
        assert_eq!(fmt_val(0.0), "0");
        assert_eq!(fmt_val(0.0001), "0,0001");
        assert_eq!(fmt_val(0.00001), "1e-05");
        assert_eq!(fmt_val(0.0005), "0,0005");
        assert_eq!(fmt_val(123.456789), "123,457");
    }

    #[test]
    fn format_g_edge_cases() {
        assert_eq!(format_g(1e6, 6), "1e+06");
        assert_eq!(format_g(123456.0, 6), "123456");
        assert_eq!(format_g(1234567.0, 6), "1.23457e+06");
        assert_eq!(format_g(1.5e-7, 6), "1.5e-07");
        assert_eq!(format_g(0.1, 6), "0.1");
        assert_eq!(format_g(2.0, 6), "2");
    }

    #[test]
    fn bar_geometry() {
        let mut cam = Camera::default();
        cam.scale = 600.0; // 600 mm over 600 px: 1 mm per pixel
        let bar = scale_bar(&cam, Viewport::new(800.0, 600.0), 1.0, false).expect("bar");
        assert_eq!(bar.length, 200.0); // nice_step(110) = 200
        assert!((bar.length_px - 200.0).abs() < 1e-9);
        assert_eq!(bar.label, "200 mm");
        assert_eq!(bar.segments.len(), 4 + 8);
        assert_eq!(bar.segments[0], ([0.0, 0.0], [200.0, 0.0]));
        assert_eq!(bar.segments[3].0, [100.0, 0.0]);
        assert!((bar.segments[3].1[1] - 4.2).abs() < 1e-12 && bar.segments[3].1[0] == 100.0);
        assert_eq!(bar.offset_px, [24.0, 22.0]);
        assert_eq!(bar.color, SCALE_BAR_LIGHT_BG);
        assert!((bar.label_anchor[1] - 10.0).abs() < 1e-9);
        let dark = scale_bar(&cam, Viewport::new(800.0, 600.0), 2.0, true).expect("bar");
        assert_eq!(dark.color, SCALE_BAR_DARK_BG);
        assert_eq!(dark.offset_px, [48.0, 44.0]);
        assert_eq!(dark.label_height_px, 26.0);
    }

    #[test]
    fn bar_uses_the_shorter_side_and_hides_in_perspective() {
        let mut cam = Camera::default();
        cam.scale = 300.0;
        // tall window: scale is the width (300 px) -> 1 mm per pixel as well
        let bar = scale_bar(&cam, Viewport::new(300.0, 900.0), 1.0, false).expect("bar");
        assert_eq!(bar.length, 200.0);
        cam.perspective = true;
        assert!(scale_bar(&cam, Viewport::new(300.0, 900.0), 1.0, false).is_none());
    }
}
