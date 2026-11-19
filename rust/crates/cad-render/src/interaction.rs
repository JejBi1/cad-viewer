//! Mouse and keyboard constants of the 3D and 2D views as plain functions, so the egui layer stays thin
//! (SPEC section 7 "Mouse (exact behaviour)" and section 10).
use crate::camera::ORBIT_RAD_PER_PX;

/// Drag starts after this manhattan distance in logical pixels (`manhattanLength() < 4`).
pub const DRAG_THRESHOLD_PX: f64 = 4.0;
/// Time after the last 3D mouse motion in which the wheel is ignored, seconds.
pub const WHEEL_IGNORE_AFTER_3D_MOUSE_SECS: f64 = 0.4;
/// Idle time after which the 3D mouse pivot is picked again, seconds.
pub const SPACE_PIVOT_REPICK_SECS: f64 = 0.3;
/// Pixel tolerance of hover and click picking, and of picking while measuring (logical pixels, multiplied by dpr).
pub const PICK_TOLERANCE_PX: f64 = 3.0;
pub const MEASURE_PICK_TOLERANCE_PX: f64 = 7.0;
/// Mesh vertex snap radius while measuring: this many pixels in world units.
pub const MESH_SNAP_PX: f64 = 7.0;
/// 2D: pick radius and snap radius in pixels.
pub const PICK_RADIUS_2D_PX: f64 = 5.0;
pub const SNAP_RADIUS_2D_PX: f64 = 10.0;

/// Arrow keys orbit by 15 degrees, with Shift by 90 degrees.
pub const ARROW_STEP_DEG: f64 = 15.0;
pub const ARROW_STEP_SHIFT_DEG: f64 = 90.0;

/// Zoom factor of a wheel step: `1.2 ** (delta / 120)`. `delta` is the vertical angle delta (120 per notch, positive
/// away from the user).
pub fn wheel_factor(delta: f64) -> f64 {
    1.2f64.powf(delta / 120.0)
}

/// Manhattan distance test of the drag threshold.
pub fn drag_started(dx: f64, dy: f64) -> bool {
    dx.abs() + dy.abs() >= DRAG_THRESHOLD_PX
}

/// Orbit angles `(ang_h, ang_v)` of a right-button drag by `(dx, dy)` physical pixels: `k = 0.006 / dpr` rad per
/// physical pixel.
pub fn orbit_angles(dx: f64, dy: f64, dpr: f64) -> (f64, f64) {
    let k = ORBIT_RAD_PER_PX / dpr.max(1e-6);
    (-dx * k, -dy * k)
}

/// Arrow keys of the 3D view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrowKey {
    Left,
    Right,
    Up,
    Down,
}

/// `(ang_h, ang_v)` for an arrow key, `None` when Ctrl, Alt or Meta is held (they disable the orbit).
pub fn arrow_orbit(key: ArrowKey, shift: bool, other_modifier: bool) -> Option<(f64, f64)> {
    if other_modifier {
        return None;
    }
    let step = (if shift { ARROW_STEP_SHIFT_DEG } else { ARROW_STEP_DEG }).to_radians();
    Some(match key {
        ArrowKey::Left => (step, 0.0),
        ArrowKey::Right => (-step, 0.0),
        ArrowKey::Up => (0.0, step),
        ArrowKey::Down => (0.0, -step),
    })
}

/// Tolerance in physical pixels for hover and click picking.
pub fn pick_tolerance(measuring: bool, dpr: f64) -> u32 {
    let base = if measuring { MEASURE_PICK_TOLERANCE_PX } else { PICK_TOLERANCE_PX };
    (base * dpr).floor().max(1.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_notch_is_1_2() {
        assert!((wheel_factor(120.0) - 1.2).abs() < 1e-12);
        assert!((wheel_factor(-120.0) - 1.0 / 1.2).abs() < 1e-12);
        assert!((wheel_factor(60.0) - 1.2f64.sqrt()).abs() < 1e-12);
        assert_eq!(wheel_factor(0.0), 1.0);
    }

    #[test]
    fn drag_threshold_is_manhattan() {
        assert!(!drag_started(1.0, 2.0));
        assert!(drag_started(2.0, 2.0));
        assert!(drag_started(-4.0, 0.0));
        assert!(!drag_started(3.9, 0.0));
    }

    #[test]
    fn orbit_scales_with_dpr() {
        let (h, v) = orbit_angles(100.0, -50.0, 1.0);
        assert!((h + 0.6).abs() < 1e-12 && (v - 0.3).abs() < 1e-12);
        let (h2, _) = orbit_angles(100.0, 0.0, 2.0);
        assert!((h2 + 0.3).abs() < 1e-12);
        // about 180 degrees per 520 px as the comment of the Python code says
        let (h3, _) = orbit_angles(-520.0, 0.0, 1.0);
        assert!((h3.to_degrees() - 178.7).abs() < 0.5);
    }

    #[test]
    fn arrows() {
        let (h, v) = arrow_orbit(ArrowKey::Left, false, false).expect("orbit");
        assert!((h.to_degrees() - 15.0).abs() < 1e-9 && v == 0.0);
        let (h, _) = arrow_orbit(ArrowKey::Right, true, false).expect("orbit");
        assert!((h.to_degrees() + 90.0).abs() < 1e-9);
        let (_, v) = arrow_orbit(ArrowKey::Up, false, false).expect("orbit");
        assert!((v.to_degrees() - 15.0).abs() < 1e-9);
        let (_, v) = arrow_orbit(ArrowKey::Down, true, false).expect("orbit");
        assert!((v.to_degrees() + 90.0).abs() < 1e-9);
        assert_eq!(arrow_orbit(ArrowKey::Left, false, true), None);
    }

    #[test]
    fn tolerances() {
        assert_eq!(pick_tolerance(false, 1.0), 3);
        assert_eq!(pick_tolerance(true, 1.0), 7);
        assert_eq!(pick_tolerance(true, 1.5), 10);
        assert_eq!(pick_tolerance(false, 2.0), 6);
    }
}
