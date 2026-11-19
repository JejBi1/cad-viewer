//! 3Dconnexion 3D mouse (SpaceMouse, SpaceNavigator, SpacePilot ...): the part that does not depend on the platform
//! (`stepview/spacemouse.py`). HID report parsing, dead zone, response curve, axis mapping, button table and the
//! Czech texts of the settings dialog. The Raw Input listener is in `spacemouse_win.rs`.
//!
//! HID axes: X right, Y towards the user, Z down. They are converted to the usual screen frame X right, Y up,
//! Z towards the user (rotations alike, right-handed). The device reports only when the values change, so the last
//! state holds until a zero report arrives; the window turns it into a velocity (frame-rate independent).
use cad_core::settings::SpaceMouse as Config;

/// Full deflection of the cap (int16 units).
pub const FULL_SCALE: f64 = 350.0;
pub const DEAD_ZONE: f64 = 0.04;
/// Response exponent: fine control near the rest position.
pub const CURVE: f64 = 1.6;
/// The longest step handed to the view after a pause.
pub const MAX_DT: f64 = 0.1;
/// Timer period of the Python version, 16 ms.
pub const TICK_SECS: f64 = 0.016;

pub const PRODUCTS: &[(u16, &str)] = &[
    (0xC603, "SpaceMouse Plus"),
    (0xC621, "SpaceBall 5000"),
    (0xC623, "SpaceTraveler"),
    (0xC625, "SpacePilot"),
    (0xC626, "SpaceNavigator"),
    (0xC627, "SpaceExplorer"),
    (0xC628, "SpaceNavigator for Notebooks"),
    (0xC629, "SpacePilot Pro"),
    (0xC62B, "SpaceMouse Pro"),
    (0xC62E, "SpaceMouse Wireless"),
    (0xC62F, "SpaceMouse Wireless"),
    (0xC631, "SpaceMouse Pro Wireless"),
    (0xC632, "SpaceMouse Pro Wireless"),
    (0xC633, "SpaceMouse Enterprise"),
    (0xC635, "SpaceMouse Compact"),
    (0xC636, "SpaceMouse Module"),
    (0xC652, "3Dconnexion Universal Receiver"),
];

/// Devices whose button bits are the 3Dconnexion "virtual keys" (bit = key - 1): Menu, Fit, Top, Left, Right, Front,
/// Bottom, Back, Roll CW, Roll CCW, ISO1, ISO2 ... (2-button devices: left = Menu, right = Fit).
pub const VK_LAYOUT: &[u16] = &[0xC626, 0xC628, 0xC62B, 0xC62E, 0xC62F, 0xC631, 0xC632, 0xC633, 0xC635, 0xC652];

/// Button bit to action. Bit 0 is Menu, left to the 3DxWare driver.
pub fn button_action(bit: u32) -> Option<&'static str> {
    Some(match bit {
        1 => "fit",
        2 => "top",
        3 => "left",
        4 => "right",
        5 => "front",
        6 => "bottom",
        7 => "back",
        8 => "roll_cw",
        9 => "roll_ccw",
        10 | 11 => "iso",
        _ => return None,
    })
}

pub const AXES: [&str; 6] = [
    "Posun vlevo / vpravo",
    "Posun nahoru / dolů",
    "Zoom (tah k sobě / od sebe)",
    "Klopení (vpřed / vzad)",
    "Otáčení (kroucení)",
    "Naklápění (do stran)",
];

pub const TITLE: &str = "3D myš (3Dconnexion)";
pub const NONE_FOUND: &str = "Žádná 3D myš nenalezena. Připoj ji – funguje i připojení za běhu programu.";
pub const INFO: &str = "Model se pohybuje jako čepička (tlač doprava = model doprava, tah k sobě = přiblížit, kroucení = otáčení). Tlačítka: Fit = zobrazit vše, na SpaceMouse Pro / Enterprise i pohledy (Top, Front, Right, …) a Roll. Levé tlačítko (Menu) obsluhuje ovladač 3DxWare.";

pub fn product_name(pid: u16) -> &'static str {
    PRODUCTS.iter().find(|(p, _)| *p == pid).map(|(_, n)| *n).unwrap_or("3D myš")
}

/// "Připojeno: " followed by the devices as `(vendor id, product id)`.
pub fn devices_text(devs: &[(u16, u16)]) -> String {
    if devs.is_empty() {
        return NONE_FOUND.to_string();
    }
    let list: Vec<String> =
        devs.iter().map(|(v, p)| format!("{} ({:04X}:{:04X})", product_name(*p), v, p)).collect();
    format!("Připojeno: {}", list.join(", "))
}

/// Speed label of the dialog: `2 ** ((slider - 50) / 25)` as `0.25×` ... `4×`, two decimals, comma.
pub fn speed_label(slider: i32) -> String {
    format!("{:.2}×", speed_from_slider(slider)).replace('.', ",")
}

/// Slider 0..100 to speed 0.25 ... 4 (logarithmic).
pub fn speed_from_slider(slider: i32) -> f64 {
    2f64.powf((f64::from(slider) - 50.0) / 25.0)
}

/// Speed to the nearest slider position.
pub fn slider_from_speed(speed: f64) -> i32 {
    (50.0 + 25.0 * speed.max(1e-6).log2()).round().clamp(0.0, 100.0) as i32
}

/// The raw state of the device and the buttons held down.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HidState {
    /// tx, ty, tz, rx, ry, rz as the device sends them.
    pub hid: [i16; 6],
    pub buttons: u64,
    /// Number of the last pressed button (bit + 1) for the dialog.
    pub last_button: Option<u32>,
}

/// What a report changed beyond the axes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReportOutcome {
    /// Buttons that went down with this report (bit numbers).
    pub pressed: Vec<u32>,
}

fn i16_at(data: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([data[at], data[at + 1]])
}

/// Applies one HID report (`SpaceMouse.report`): id 1 is translation (a report of at least 13 bytes carries the
/// rotation too), id 2 is rotation, id 3 is the button bits.
pub fn apply_report(state: &mut HidState, data: &[u8]) -> ReportOutcome {
    let mut out = ReportOutcome::default();
    let Some(&rid) = data.first() else { return out };
    match rid {
        1 if data.len() >= 7 => {
            for k in 0..3 {
                state.hid[k] = i16_at(data, 1 + 2 * k);
            }
            if data.len() >= 13 {
                for k in 0..3 {
                    state.hid[3 + k] = i16_at(data, 7 + 2 * k);
                }
            }
        }
        2 if data.len() >= 7 => {
            for k in 0..3 {
                state.hid[3 + k] = i16_at(data, 1 + 2 * k);
            }
        }
        3 => {
            let mut bits = 0u64;
            for (i, b) in data[1..].iter().take(8).enumerate() {
                bits |= u64::from(*b) << (8 * i);
            }
            let pressed = bits & !state.buttons;
            state.buttons = bits;
            for bit in 0..(data[1..].len().min(8) * 8) as u32 {
                if pressed >> bit & 1 == 1 {
                    state.last_button = Some(bit + 1);
                    out.pressed.push(bit);
                }
            }
        }
        _ => {}
    }
    out
}

/// Deflection per axis in the screen frame, -1..1, without dead zone and curve (`levels`).
pub fn levels(hid: &[i16; 6]) -> [f64; 6] {
    let h = |i: usize| f64::from(hid[i]);
    let clamp = |x: f64| (x / FULL_SCALE).clamp(-1.0, 1.0);
    [clamp(h(0)), clamp(-h(2)), clamp(h(1)), clamp(h(3)), clamp(-h(5)), clamp(h(4))]
}

/// Dead zone and response curve of one axis.
pub fn shape_axis(x: f64) -> f64 {
    let a = x.abs();
    if a < DEAD_ZONE {
        0.0
    } else {
        (((a - DEAD_ZONE) / (1.0 - DEAD_ZONE)).powf(CURVE)).copysign(x)
    }
}

/// `shaped()`: dead zone and curve, inversion, pan / rotate masks, dominant axis, speed (in this order).
pub fn shaped(hid: &[i16; 6], cfg: &Config) -> [f64; 6] {
    let mut v = levels(hid);
    for (i, x) in v.iter_mut().enumerate() {
        let y = shape_axis(*x);
        *x = if cfg.invert[i] { -y } else { y };
    }
    if !cfg.pan {
        v[0..3].fill(0.0);
    }
    if !cfg.rotate {
        v[3..6].fill(0.0);
    }
    if cfg.dominant && v.iter().any(|x| *x != 0.0) {
        let mut k = 0;
        for i in 1..6 {
            if v[i].abs() > v[k].abs() {
                k = i;
            }
        }
        for (i, x) in v.iter_mut().enumerate() {
            if i != k {
                *x = 0.0;
            }
        }
    }
    v.map(|x| x * cfg.speed)
}

/// The step of time handed to the view: the time since the last tick, clamped to `0..=0.1` s.
pub fn tick_dt(elapsed_secs: f64) -> f64 {
    elapsed_secs.clamp(0.0, MAX_DT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::default()
    }

    #[test]
    fn dead_zone_and_curve() {
        assert_eq!(shape_axis(0.0), 0.0);
        assert_eq!(shape_axis(0.039), 0.0);
        assert_eq!(shape_axis(-0.039), 0.0);
        assert!((shape_axis(1.0) - 1.0).abs() < 1e-12);
        assert!((shape_axis(-1.0) + 1.0).abs() < 1e-12);
        // y = ((|x| - 0.04) / 0.96) ^ 1.6
        let x = 0.5;
        assert!((shape_axis(x) - ((0.5 - 0.04) / 0.96f64).powf(1.6)).abs() < 1e-12);
        assert!(shape_axis(-0.5) < 0.0);
        // fine control near the rest position: small deflection gives a much smaller output
        assert!(shape_axis(0.1) < 0.1 * 0.5);
    }

    #[test]
    fn axis_mapping_to_the_screen_frame() {
        // HID (tx, ty, tz, rx, ry, rz) -> screen (tx, -tz, ty, rx, -rz, ry)
        let l = levels(&[35, 70, 105, 140, 175, 210]);
        let e = [0.1, -0.3, 0.2, 0.4, -0.6, 0.5];
        for k in 0..6 {
            assert!((l[k] - e[k]).abs() < 1e-12, "{k}: {l:?}");
        }
        // clamped to full scale
        assert_eq!(levels(&[1000, 0, 0, 0, 0, 0])[0], 1.0);
        assert_eq!(levels(&[-1000, 0, 0, 0, 0, 0])[0], -1.0);
    }

    #[test]
    fn six_axis_and_split_reports() {
        let mut st = HidState::default();
        let mut r = vec![1u8];
        for v in [10i16, -20, 30, 40, -50, 60] {
            r.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(r.len(), 13);
        apply_report(&mut st, &r);
        assert_eq!(st.hid, [10, -20, 30, 40, -50, 60]);
        // a short translation report keeps the rotation, report 2 sets the rotation
        let mut t = vec![1u8];
        for v in [1i16, 2, 3] {
            t.extend_from_slice(&v.to_le_bytes());
        }
        apply_report(&mut st, &t);
        assert_eq!(st.hid, [1, 2, 3, 40, -50, 60]);
        let mut rot = vec![2u8];
        for v in [7i16, 8, 9] {
            rot.extend_from_slice(&v.to_le_bytes());
        }
        apply_report(&mut st, &rot);
        assert_eq!(st.hid, [1, 2, 3, 7, 8, 9]);
        // too short or unknown reports change nothing
        apply_report(&mut st, &[1, 0, 0]);
        apply_report(&mut st, &[9, 1, 2, 3, 4, 5, 6, 7]);
        apply_report(&mut st, &[]);
        assert_eq!(st.hid, [1, 2, 3, 7, 8, 9]);
    }

    #[test]
    fn buttons_report_only_new_presses() {
        let mut st = HidState::default();
        let o = apply_report(&mut st, &[3, 0b0000_0110]);
        assert_eq!(o.pressed, vec![1, 2]);
        assert_eq!(st.last_button, Some(3));
        // holding bit 1 and pressing bit 3: only bit 3 is new
        let o = apply_report(&mut st, &[3, 0b0000_1010]);
        assert_eq!(o.pressed, vec![3]);
        // release everything
        let o = apply_report(&mut st, &[3, 0]);
        assert!(o.pressed.is_empty());
        // second byte holds bits 8..15
        let o = apply_report(&mut st, &[3, 0, 0b0000_0011]);
        assert_eq!(o.pressed, vec![8, 9]);
    }

    #[test]
    fn button_table() {
        assert_eq!(button_action(0), None);
        assert_eq!(button_action(1), Some("fit"));
        assert_eq!(button_action(2), Some("top"));
        assert_eq!(button_action(3), Some("left"));
        assert_eq!(button_action(4), Some("right"));
        assert_eq!(button_action(5), Some("front"));
        assert_eq!(button_action(6), Some("bottom"));
        assert_eq!(button_action(7), Some("back"));
        assert_eq!(button_action(8), Some("roll_cw"));
        assert_eq!(button_action(9), Some("roll_ccw"));
        assert_eq!(button_action(10), Some("iso"));
        assert_eq!(button_action(11), Some("iso"));
        assert_eq!(button_action(12), None);
        assert!(VK_LAYOUT.contains(&0xC62B));
        assert!(!VK_LAYOUT.contains(&0xC603));
    }

    #[test]
    fn shaped_applies_masks_inversion_dominance_and_speed() {
        let hid = [350, 0, 0, 0, 0, 0]; // full right push, X
        let mut c = cfg();
        assert!((shaped(&hid, &c)[0] - 1.0).abs() < 1e-12);
        c.speed = 2.5;
        assert!((shaped(&hid, &c)[0] - 2.5).abs() < 1e-12);
        c.speed = 1.0;
        c.invert[0] = true;
        assert!((shaped(&hid, &c)[0] + 1.0).abs() < 1e-12);
        c.invert[0] = false;
        c.pan = false;
        assert_eq!(shaped(&hid, &c)[0], 0.0);
        c.pan = true;
        // rotation about HID X is screen axis 3
        let rot = [0, 0, 0, 350, 0, 0];
        assert!((shaped(&rot, &c)[3] - 1.0).abs() < 1e-12);
        c.rotate = false;
        assert_eq!(shaped(&rot, &c)[3], 0.0);
        c.rotate = true;
        // dominant axis keeps only the largest
        c.dominant = true;
        let mixed = [350, 100, 0, 0, 0, 0]; // tx full, screen axis 2 (HID y) smaller
        let v = shaped(&mixed, &c);
        assert!(v[0] > 0.9 && v[2] == 0.0);
        // speed multiplies last, after dominance
        c.speed = 3.0;
        assert!((shaped(&mixed, &c)[0] - 3.0).abs() < 1e-12);
    }

    #[test]
    fn dt_is_clamped() {
        assert_eq!(tick_dt(-1.0), 0.0);
        assert_eq!(tick_dt(0.05), 0.05);
        assert_eq!(tick_dt(5.0), 0.1);
    }

    #[test]
    fn speed_slider_is_logarithmic() {
        assert!((speed_from_slider(50) - 1.0).abs() < 1e-12);
        assert!((speed_from_slider(0) - 0.25).abs() < 1e-12);
        assert!((speed_from_slider(100) - 4.0).abs() < 1e-12);
        assert_eq!(slider_from_speed(1.0), 50);
        assert_eq!(slider_from_speed(0.25), 0);
        assert_eq!(slider_from_speed(4.0), 100);
        assert_eq!(slider_from_speed(10.0), 100);
        assert_eq!(speed_label(0), "0,25×");
        assert_eq!(speed_label(50), "1,00×");
        assert_eq!(speed_label(100), "4,00×");
    }

    #[test]
    fn device_text() {
        assert_eq!(devices_text(&[]), NONE_FOUND);
        assert_eq!(devices_text(&[(0x046D, 0xC62B)]), "Připojeno: SpaceMouse Pro (046D:C62B)");
        assert_eq!(devices_text(&[(0x256F, 0x1234)]), "Připojeno: 3D myš (256F:1234)");
    }
}
