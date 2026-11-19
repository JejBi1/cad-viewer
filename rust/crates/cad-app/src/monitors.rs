//! Where the window may be put at the start. A saved position that lies off every monitor (the second monitor was
//! unplugged, the resolution changed, a remote session) would open the window where nobody can see or grab it; Qt's
//! `restoreGeometry` moved such a window back, and so does this.

/// A monitor's usable area (without the task bar) in the units of the saved geometry (points).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// `x, y, width, height, maximised`, as `ui::parse_geometry` gives them.
pub type Geometry = (f32, f32, f32, f32, bool);

/// Width of the title bar that has to lie on a monitor so that the window can be grabbed with the mouse.
const GRAB_W: f32 = 100.0;
/// Height of the strip that stands for the title bar.
const BAR_H: f32 = 32.0;

fn overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

/// True when the title bar of the window is on a monitor: at least 100 points of its width and half its height.
pub fn title_bar_reachable(g: Geometry, monitors: &[Area]) -> bool {
    let (x, y, w, _, _) = g;
    monitors
        .iter()
        .any(|m| overlap(x, x + w, m.x, m.x + m.w) >= GRAB_W.min(w) && overlap(y, y + BAR_H, m.y, m.y + m.h) >= BAR_H / 2.0)
}

/// The geometry unchanged when the window can be reached on some monitor, else centred on the first monitor (the
/// main one) and no bigger than it. Without any monitor information (other systems) nothing is changed.
pub fn fit_to_monitors(g: Geometry, monitors: &[Area]) -> Geometry {
    let Some(main) = monitors.first() else { return g };
    if title_bar_reachable(g, monitors) {
        return g;
    }
    let (w, h) = (g.2.min(main.w), g.3.min(main.h));
    (main.x + (main.w - w) / 2.0, main.y + (main.h - h) / 2.0, w, h, g.4)
}

/// The work areas of the monitors, the main one first. Windows only; elsewhere the list is empty.
///
/// The saved geometry is in points (egui's logical units). The call is made before the window system is started, when
/// the process is not yet DPI aware, so Windows hands out the monitor rectangles in the same logical units of the
/// system scale. Only the main monitor matches them exactly when the monitors have different scales; the test is
/// meant for a window that is plainly off every monitor, and the place it moves a window to is the main monitor.
#[cfg(windows)]
pub fn monitor_areas() -> Vec<Area> {
    use windows_sys::Win32::Foundation::{LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};

    const PRIMARY: u32 = 1; // MONITORINFOF_PRIMARY

    unsafe extern "system" fn each(monitor: HMONITOR, _dc: HDC, _rect: *mut RECT, data: LPARAM) -> i32 {
        // SAFETY: `data` is the address of the vector `monitor_areas` passed, alive during the enumeration
        let out = unsafe { &mut *(data as *mut Vec<(bool, Area)>) };
        // SAFETY: MONITORINFO is plain numbers; `cbSize` is set as the call demands
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        // SAFETY: `info` is a valid, sized MONITORINFO
        if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
            let r = info.rcWork;
            let (w, h) = ((r.right - r.left) as f32, (r.bottom - r.top) as f32);
            if w > 0.0 && h > 0.0 {
                out.push((info.dwFlags & PRIMARY != 0, Area { x: r.left as f32, y: r.top as f32, w, h }));
            }
        }
        1
    }

    let mut found: Vec<(bool, Area)> = Vec::new();
    // SAFETY: no device context and no clip rectangle means all monitors; the callback only uses `data` as above
    unsafe {
        EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(each), &mut found as *mut _ as LPARAM);
    }
    found.sort_by_key(|(primary, _)| !*primary);
    found.into_iter().map(|(_, a)| a).collect()
}

#[cfg(not(windows))]
pub fn monitor_areas() -> Vec<Area> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(x: f32, y: f32, w: f32, h: f32) -> Area {
        Area { x, y, w, h }
    }

    const MAIN: Area = Area { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 };

    #[test]
    fn a_window_on_a_monitor_stays_where_it_is() {
        for g in [
            (100.0, 50.0, 1500.0, 900.0, false),
            (0.0, 0.0, 1920.0, 1040.0, true),
            // a window that sticks out to the right but whose title bar can be grabbed
            (1800.0, 100.0, 1500.0, 900.0, false),
            // and one that sticks out below
            (100.0, 1000.0, 1500.0, 900.0, false),
        ] {
            assert_eq!(fit_to_monitors(g, &[MAIN]), g);
        }
    }

    #[test]
    fn a_window_of_an_unplugged_monitor_comes_back_to_the_main_one() {
        // saved on a second monitor to the right, which is not there any more
        let g = (2500.0, 200.0, 1500.0, 900.0, false);
        let back = fit_to_monitors(g, &[MAIN]);
        assert_eq!(back, (210.0, 70.0, 1500.0, 900.0, false));
        assert!(title_bar_reachable(back, &[MAIN]));
        // saved on a monitor to the left
        assert!(title_bar_reachable(fit_to_monitors((-1900.0, 10.0, 1200.0, 800.0, false), &[MAIN]), &[MAIN]));
        // saved far below / far above (title bar not reachable)
        for (x, y) in [(100.0, 5000.0), (100.0, -700.0), (-3000.0, -3000.0), (40000.0, 0.0)] {
            let back = fit_to_monitors((x, y, 1000.0, 700.0, false), &[MAIN]);
            assert!(title_bar_reachable(back, &[MAIN]), "({x}, {y}) -> {back:?}");
        }
        // the maximised flag is kept
        assert!(fit_to_monitors((9000.0, 0.0, 1000.0, 700.0, true), &[MAIN]).4);
    }

    #[test]
    fn a_window_on_a_second_monitor_that_is_there_stays() {
        let left = area(-1920.0, 0.0, 1920.0, 1080.0);
        let g = (-1900.0, 10.0, 1200.0, 800.0, false);
        assert_eq!(fit_to_monitors(g, &[MAIN, left]), g);
        // the same window with that monitor gone is moved
        assert_ne!(fit_to_monitors(g, &[MAIN]), g);
    }

    #[test]
    fn a_window_is_not_made_bigger_than_the_monitor_it_comes_back_to() {
        let small = area(0.0, 0.0, 1280.0, 720.0);
        let back = fit_to_monitors((5000.0, 0.0, 1500.0, 900.0, false), &[small]);
        assert_eq!(back, (0.0, 0.0, 1280.0, 720.0, false));
    }

    #[test]
    fn without_monitor_information_nothing_changes() {
        let g = (9999.0, 9999.0, 800.0, 600.0, false);
        assert_eq!(fit_to_monitors(g, &[]), g);
        #[cfg(not(windows))]
        assert!(monitor_areas().is_empty());
    }

    #[test]
    fn the_title_bar_needs_a_grip_on_the_monitor() {
        // 99 points of the width on the monitor is not enough, 100 is
        assert!(!title_bar_reachable((-1401.0, 100.0, 1500.0, 800.0, false), &[MAIN]));
        assert!(title_bar_reachable((-1400.0, 100.0, 1500.0, 800.0, false), &[MAIN]));
        // a narrow window needs only its own width
        assert!(title_bar_reachable((10.0, 10.0, 60.0, 400.0, false), &[MAIN]));
        // the bar has to be on the monitor vertically (more than half of it)
        assert!(!title_bar_reachable((100.0, -20.0, 800.0, 600.0, false), &[MAIN]));
        assert!(title_bar_reachable((100.0, -10.0, 800.0, 600.0, false), &[MAIN]));
        assert!(!title_bar_reachable((100.0, 1030.0, 800.0, 600.0, false), &[MAIN]));
    }

    #[cfg(windows)]
    #[test]
    fn the_monitors_of_this_computer_can_be_listed() {
        // a session without a display may have none; every area that is listed is a real rectangle
        for a in monitor_areas() {
            assert!(a.w > 0.0 && a.h > 0.0 && a.x.is_finite() && a.y.is_finite(), "{a:?}");
        }
    }
}
