//! Keyboard shortcuts of the window (SPEC section 14, "Shortcut map"): Ctrl+O, F5, F, Z, 0 to 6, Shift+1 to Shift+7, E, W,
//! P, M, U, G, K, Ctrl+S, H, I, A, T, Esc, F1 and Ctrl+C.
use cad_render::StdView;
use eframe::egui::{Event, Key, Modifiers};

use crate::app::{Action, Mode};

/// The view of a number key: plain `0` to `6`, or Shift with `1` to `7` (Onshape).
pub fn view_of_digit(digit: u8, shift: bool) -> Option<StdView> {
    if shift {
        Some(match digit {
            1 => StdView::Front,
            2 => StdView::Back,
            3 => StdView::Left,
            4 => StdView::Right,
            5 => StdView::Top,
            6 => StdView::Bottom,
            7 => StdView::Iso,
            _ => return None,
        })
    } else {
        Some(match digit {
            0 => StdView::Iso,
            1 => StdView::Front,
            2 => StdView::Back,
            3 => StdView::Top,
            4 => StdView::Bottom,
            5 => StdView::Left,
            6 => StdView::Right,
            _ => return None,
        })
    }
}

fn digit_of(key: Key) -> Option<u8> {
    Some(match key {
        Key::Num0 => 0,
        Key::Num1 => 1,
        Key::Num2 => 2,
        Key::Num3 => 3,
        Key::Num4 => 4,
        Key::Num5 => 5,
        Key::Num6 => 6,
        Key::Num7 => 7,
        Key::Num8 => 8,
        Key::Num9 => 9,
        _ => return None,
    })
}

/// Actions that exist only in the 3D view (their toolbar buttons are hidden in 2D, and so are their shortcuts).
pub fn is_3d_only(a: &Action) -> bool {
    matches!(
        a,
        Action::View(_) | Action::Edges | Action::Wire | Action::Persp | Action::Transp | Action::Material | Action::Mass | Action::Collide
    )
}

/// The action of a key press. `text_focus`: a text field has the keyboard, so single keys are typing.
pub fn action_for(key: Key, physical: Option<Key>, m: Modifiers, text_focus: bool, mode: Mode) -> Option<Action> {
    let ctrl = m.ctrl || m.command;
    let single = !ctrl && !m.alt && !m.mac_cmd;
    let a = if ctrl && !m.alt && !m.shift {
        match key {
            Key::O => Some(Action::Open),
            Key::S => Some(Action::Shot),
            Key::C if !text_focus => Some(Action::CopyMeasure),
            _ => None,
        }
    } else if single && !m.shift && !text_focus {
        match key {
            Key::F => Some(Action::Fit),
            Key::Z => Some(Action::ZoomSel),
            Key::E => Some(Action::Edges),
            Key::W => Some(Action::Wire),
            Key::P => Some(Action::Persp),
            Key::M => Some(Action::Measure),
            Key::U => Some(Action::Angle),
            Key::G => Some(Action::Mass),
            Key::K => Some(Action::Collide),
            Key::H => Some(Action::Hide),
            Key::I => Some(Action::Isolate),
            Key::A => Some(Action::ShowAll),
            Key::T => Some(Action::Transp),
            _ => None,
        }
    } else {
        None
    };
    let a = a.or_else(|| match key {
        Key::F5 if !ctrl => Some(Action::Reload),
        Key::F1 if !ctrl => Some(Action::About),
        Key::Escape if single && !m.shift => Some(Action::Escape),
        _ => None,
    });
    let a = a.or_else(|| {
        if !single || text_focus {
            return None;
        }
        // the number row: by its physical key too, because on a Czech keyboard the plain digit keys type letters
        let d = digit_of(key).or_else(|| physical.and_then(digit_of))?;
        view_of_digit(d, m.shift).map(Action::View)
    });
    a.filter(|a| mode != Mode::TwoD || !is_3d_only(a))
}

/// Every shortcut in the events of a frame, in order. Repeats do not count.
pub fn actions_in(events: &[Event], text_focus: bool, mode: Mode) -> Vec<Action> {
    let mut out = Vec::new();
    for ev in events {
        match ev {
            Event::Key { key, physical_key, pressed: true, repeat: false, modifiers } => {
                if let Some(a) = action_for(*key, *physical_key, *modifiers, text_focus, mode) {
                    out.push(a);
                }
            }
            // the integration turns Ctrl+C into a Copy event
            Event::Copy if !text_focus => out.push(Action::CopyMeasure),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(key: Key, m: Modifiers) -> Option<Action> {
        action_for(key, None, m, false, Mode::ThreeD)
    }

    #[test]
    fn letter_shortcuts() {
        let n = Modifiers::NONE;
        assert_eq!(act(Key::F, n), Some(Action::Fit));
        assert_eq!(act(Key::Z, n), Some(Action::ZoomSel));
        assert_eq!(act(Key::E, n), Some(Action::Edges));
        assert_eq!(act(Key::W, n), Some(Action::Wire));
        assert_eq!(act(Key::P, n), Some(Action::Persp));
        assert_eq!(act(Key::M, n), Some(Action::Measure));
        assert_eq!(act(Key::U, n), Some(Action::Angle));
        assert_eq!(act(Key::G, n), Some(Action::Mass));
        assert_eq!(act(Key::K, n), Some(Action::Collide));
        assert_eq!(act(Key::H, n), Some(Action::Hide));
        assert_eq!(act(Key::I, n), Some(Action::Isolate));
        assert_eq!(act(Key::A, n), Some(Action::ShowAll));
        assert_eq!(act(Key::T, n), Some(Action::Transp));
        assert_eq!(act(Key::Q, n), None);
        // a letter with Shift or Alt is another shortcut
        assert_eq!(act(Key::F, Modifiers::SHIFT), None);
        assert_eq!(act(Key::F, Modifiers::ALT), None);
    }

    #[test]
    fn control_and_function_keys() {
        assert_eq!(act(Key::O, Modifiers::CTRL), Some(Action::Open));
        assert_eq!(act(Key::S, Modifiers::CTRL), Some(Action::Shot));
        assert_eq!(act(Key::C, Modifiers::CTRL), Some(Action::CopyMeasure));
        assert_eq!(act(Key::F5, Modifiers::NONE), Some(Action::Reload));
        assert_eq!(act(Key::F1, Modifiers::NONE), Some(Action::About));
        assert_eq!(act(Key::Escape, Modifiers::NONE), Some(Action::Escape));
        assert_eq!(act(Key::O, Modifiers::NONE), None);
    }

    #[test]
    fn number_keys_choose_views() {
        let n = Modifiers::NONE;
        let views = [
            (Key::Num0, StdView::Iso),
            (Key::Num1, StdView::Front),
            (Key::Num2, StdView::Back),
            (Key::Num3, StdView::Top),
            (Key::Num4, StdView::Bottom),
            (Key::Num5, StdView::Left),
            (Key::Num6, StdView::Right),
        ];
        for (k, v) in views {
            assert_eq!(act(k, n), Some(Action::View(v)), "{k:?}");
        }
        assert_eq!(act(Key::Num7, n), None);
        // Onshape: Shift+1 front ... Shift+7 isometric
        let s = Modifiers::SHIFT;
        let shifted = [
            (Key::Num1, StdView::Front),
            (Key::Num2, StdView::Back),
            (Key::Num3, StdView::Left),
            (Key::Num4, StdView::Right),
            (Key::Num5, StdView::Top),
            (Key::Num6, StdView::Bottom),
            (Key::Num7, StdView::Iso),
        ];
        for (k, v) in shifted {
            assert_eq!(act(k, s), Some(Action::View(v)), "shift {k:?}");
        }
        assert_eq!(act(Key::Num0, s), None);
        // on a Czech keyboard the digit row without Shift produces other characters: the physical key counts
        assert_eq!(
            action_for(Key::Plus, Some(Key::Num1), Modifiers::NONE, false, Mode::ThreeD),
            Some(Action::View(StdView::Front))
        );
    }

    #[test]
    fn typing_in_a_text_field_is_not_a_shortcut() {
        assert_eq!(action_for(Key::F, None, Modifiers::NONE, true, Mode::ThreeD), None);
        assert_eq!(action_for(Key::Num1, None, Modifiers::NONE, true, Mode::ThreeD), None);
        // but Ctrl+O and the function keys still work
        assert_eq!(action_for(Key::O, None, Modifiers::CTRL, true, Mode::ThreeD), Some(Action::Open));
        assert_eq!(action_for(Key::F5, None, Modifiers::NONE, true, Mode::ThreeD), Some(Action::Reload));
        // Ctrl+C belongs to the text field
        assert_eq!(action_for(Key::C, None, Modifiers::CTRL, true, Mode::ThreeD), None);
    }

    #[test]
    fn three_d_commands_do_not_exist_in_2d() {
        for k in [Key::E, Key::W, Key::P, Key::G, Key::K, Key::T, Key::Num1] {
            assert_eq!(action_for(k, None, Modifiers::NONE, false, Mode::TwoD), None, "{k:?}");
        }
        assert_eq!(action_for(Key::F, None, Modifiers::NONE, false, Mode::TwoD), Some(Action::Fit));
        assert_eq!(action_for(Key::M, None, Modifiers::NONE, false, Mode::TwoD), Some(Action::Measure));
        assert_eq!(action_for(Key::H, None, Modifiers::NONE, false, Mode::TwoD), Some(Action::Hide));
    }

    #[test]
    fn events_in_order_without_repeats() {
        let ev = |key, repeat| Event::Key { key, physical_key: None, pressed: true, repeat, modifiers: Modifiers::NONE };
        let events = vec![ev(Key::F, false), ev(Key::F, true), Event::Copy, ev(Key::H, false)];
        assert_eq!(actions_in(&events, false, Mode::ThreeD), vec![Action::Fit, Action::CopyMeasure, Action::Hide]);
        assert_eq!(actions_in(&events, true, Mode::ThreeD), vec![]);
    }
}
