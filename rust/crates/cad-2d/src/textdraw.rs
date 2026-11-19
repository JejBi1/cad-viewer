//! TEXT, ATTRIB, ATTDEF and MTEXT drawing (ezdxf `drawing.text` and the complex MTEXT renderer).
use crate::colors::*;
use crate::dxf::*;
use crate::font::{self, Font};
use crate::frontend::Frontend;
use crate::geom::*;
use crate::mtext::{self, has_inline_codes, layout, parse_mtext, plain_mtext, text_wrap, caret_decode, Ctx, LayoutOptions, ParaAlign};
use crate::props::Properties;
use crate::scene::{FLAG_BOX, FLAG_NOSNAP, FLAG_TEXT, MAX_LINE_GLYPH_ELEMS};
use std::sync::Arc;

pub const MIN_CAP_HEIGHT: f64 = 1e-9;
const DEFAULT_LINE_SPACING: f64 = 5.0 / 3.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HA {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum VA {
    Top,
    LowerCaseCenter,
    Baseline,
    Bottom,
    UpperCaseCenter,
}

/// (alignment, is aligned / fit) from the DXF halign / valign flags.
fn text_alignment(halign: i32, valign: i32) -> (HA, VA, i32) {
    // 3 = ALIGNED, 5 = FIT (4 = MIDDLE)
    match (halign, valign) {
        (3, _) => (HA::Center, VA::Baseline, 3),
        (5, _) => (HA::Center, VA::Baseline, 5),
        (4, _) => (HA::Center, VA::LowerCaseCenter, 4),
        (h, v) => {
            let ha = match h {
                1 => HA::Center,
                2 => HA::Right,
                _ => HA::Left,
            };
            let va = match v {
                1 => VA::Bottom,
                2 => VA::UpperCaseCenter,
                3 => VA::Top,
                _ => VA::Baseline,
            };
            (ha, va, 0)
        }
    }
}

fn mtext_alignment(attachment: i32) -> (HA, VA) {
    let h = match attachment {
        2 | 5 | 8 => HA::Center,
        3 | 6 | 9 => HA::Right,
        _ => HA::Left,
    };
    let v = match attachment {
        4..=6 => VA::LowerCaseCenter,
        7..=9 => VA::Bottom,
        _ => VA::Top,
    };
    (h, v)
}

/// `%%c`, `%%d`, `%%p` and ignored `%%k %%o %%u` of single line text.
pub fn plain_text(text: &str) -> String {
    let t = caret_decode(text).replace(['\r', '\n'], "");
    let c: Vec<char> = t.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '%' && i + 2 < c.len() + 0 && c.get(i + 1) == Some(&'%') {
            if let Some(&code) = c.get(i + 2) {
                match code.to_ascii_lowercase() {
                    'c' => {
                        out.push('Ø');
                        i += 3;
                        continue;
                    }
                    'd' => {
                        out.push('°');
                        i += 3;
                        continue;
                    }
                    'p' => {
                        out.push('±');
                        i += 3;
                        continue;
                    }
                    'k' | 'o' | 'u' => {
                        i += 3;
                        continue;
                    }
                    _ => {}
                }
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

fn non_printable(text: &str, replacement: char) -> String {
    text.chars().map(|c| if (c as u32) < 32 && c != '\t' { replacement } else { c }).collect()
}

impl<'a> Frontend<'a> {
    pub(crate) fn style_font(&self, style: u32) -> Arc<Font> {
        let name = self.doc.style_names.name(style);
        let def = self.doc.styles.iter().find(|s| s.name.eq_ignore_ascii_case(name));
        font::font_for_style(def, &self.fonts)
    }

    /// Filled glyph paths of one line of text; `tx` maps text coordinates (baseline at y = 0) to the WCS.
    pub(crate) fn draw_text_line(&mut self, text: &str, tx: &Xform, props: &Properties, cap_height: f64, font: &Font, width_factor: f64, is_mtext: bool) {
        if text.trim().is_empty() {
            return;
        }
        let mut s = if is_mtext { non_printable(text, '▯') } else { non_printable(text, '?') };
        if is_mtext {
            s = s.replace('\t', "        ");
        } else {
            s = s.replace('\t', "?");
        }
        let full = tx.then(&self.ctm);
        // The outlines of the letters take 500 to 1 000 path elements per line of text. A drawing of a hundred thousand
        // texts would take gigabytes before anything is made of them, so they have a budget (`Recorder::glyph_elems_left`):
        // the texts after it is used up are recorded as a box of five elements, which the scene draws as a bar.
        let left = self.rec.glyph_elems_left().min(MAX_LINE_GLYPH_ELEMS);
        let needed = font.glyph_elems_up_to(&s, left);
        if needed > left {
            self.draw_text_box(&s, &full, props, cap_height, font, width_factor);
            return;
        }
        let paths = font.glyph_paths(&s, cap_height, width_factor);
        if paths.is_empty() {
            return;
        }
        self.rec.add_glyph_elems(paths.iter().map(|e| e.len()).sum());
        let elems: Vec<Elems> = paths
            .into_iter()
            .map(|mut e| {
                e.transform(&full);
                e
            })
            .collect();
        self.out_filled_elems_flags(elems, props, FLAG_TEXT);
    }

    /// A line of text as the box of its baseline and cap height (`full` maps text coordinates to the drawing): the box the
    /// scene draws as a bar when the outlines of the letters would not fit. Nothing is left out of the drawing but the
    /// letters.
    fn draw_text_box(&mut self, text: &str, full: &Xform, props: &Properties, cap_height: f64, font: &Font, width_factor: f64) {
        let w = font.advance_width(text.trim_end(), cap_height, width_factor);
        if !(w.is_finite() && w > 0.0) {
            return;
        }
        let corner = |x: f64, y: f64| full.apply2([x, y]);
        let mut e = Elems::new();
        e.move_to(corner(0.0, 0.0));
        e.line_to(corner(w, 0.0));
        e.line_to(corner(w, cap_height));
        e.line_to(corner(0.0, cap_height));
        e.line_to(corner(0.0, 0.0));
        // the corners of the box are not points of the drawing: they do not snap
        self.out_filled_elems_flags(vec![e], props, FLAG_TEXT | FLAG_BOX | FLAG_NOSNAP);
    }

    pub(crate) fn draw_text_entity(&mut self, _e: &Entity, t: &TextData, props: &Properties) {
        if t.height < MIN_CAP_HEIGHT {
            return;
        }
        let ocs = Ocs::new(_e.c.extrusion);
        // spatial text is not supported
        if _e.c.extrusion[0].abs() > 1e-9 || _e.c.extrusion[1].abs() > 1e-9 {
            return;
        }
        let font = self.style_font(t.style);
        let cap = if t.height > MIN_CAP_HEIGHT { t.height } else { 2.5 };
        let raw = if t.kind == TextKind::Attdef { &t.tag } else { &t.text };
        let text = plain_text(raw);
        let (ha, va, special) = text_alignment(t.halign, t.valign);
        let m = font.measurements(cap);
        let line_w = font.text_width(&text, cap, 1.0);
        let line_y = -m.baseline - cap;
        let box_w = line_w;
        let (anchor_x, line_x) = match ha {
            HA::Left => (0.0, 0.0),
            HA::Center => (box_w / 2.0, box_w / 2.0 - line_w / 2.0),
            HA::Right => (box_w, box_w - line_w),
        };
        let anchor_y = match va {
            VA::Top => 0.0,
            VA::LowerCaseCenter => (line_y + m.x_height + line_y) / 2.0,
            VA::UpperCaseCenter => (line_y + m.cap_height + line_y) / 2.0,
            VA::Baseline => line_y,
            VA::Bottom => line_y - m.descender,
        };
        let aligned = special == 3 || special == 5;
        let rotation = if aligned {
            let d = sub(t.align_point, t.insert);
            if !v3_close(t.insert, t.align_point) {
                d[1].atan2(d[0])
            } else {
                t.rotation.to_radians()
            }
        } else {
            t.rotation.to_radians()
        };
        let mut sx = t.width_factor;
        let mut sy = 1.0;
        if aligned && line_w.abs() > 1e-9 {
            let defined = dist(t.align_point, t.insert);
            let f = defined / line_w.abs();
            sx = f;
            if special == 3 {
                sy = f;
            }
        }
        if t.generation & 2 != 0 {
            sx = -sx;
        }
        if t.generation & 4 != 0 {
            sy = -sy;
        }
        sx *= if _e.c.extrusion[2] < 0.0 { -1.0 } else { 1.0 };
        // insertion point: LEFT / BASELINE uses the insert point, aligned types the middle, the rest the align point
        let ins = if t.halign == 0 && t.valign == 0 {
            t.insert
        } else if aligned {
            lerp(t.insert, t.align_point, 0.5)
        } else if t.has_align_point {
            t.align_point
        } else {
            t.insert
        };
        let ins_wcs = ocs.to_wcs(ins);
        let rot = Xform::axis_rotate(_e.c.extrusion, rotation);
        let tx = Xform::translate([line_x - anchor_x, line_y - anchor_y, 0.0])
            .then(&Xform::scale([sx, sy, 1.0]))
            .then(&rot)
            .then(&Xform::translate(ins_wcs));
        self.draw_text_line(&text, &tx, props, cap, &font, 1.0, false);
    }

    pub(crate) fn draw_mtext_entity(&mut self, _e: &Entity, m: &MTextData, props: &Properties) {
        if _e.c.extrusion[0].abs() > 1e-9 || _e.c.extrusion[1].abs() > 1e-9 {
            return; // 3D MTEXT is not supported
        }
        let style = self.doc.style_names.name(m.style).to_string();
        let sdef = self.doc.styles.iter().find(|s| s.name.eq_ignore_ascii_case(&style)).cloned();
        let (style_w, style_obl) = sdef.as_ref().map(|s| (s.width_factor, s.oblique)).unwrap_or((1.0, 0.0));
        let has_columns = m.column_count > 1 || m.column_type != 0;
        let complex = has_columns || has_inline_codes(&m.text) || style_w != 1.0 || style_obl != 0.0;
        let font = self.style_font(m.style);
        let dir = match m.text_direction {
            Some(d) if !is_null(d) => normalize(d),
            _ => {
                let r = m.rotation.to_radians();
                [r.cos(), r.sin(), 0.0]
            }
        };
        let uz = normalize_or_z(_e.c.extrusion);
        // text coordinate system: x = text direction, z = extrusion
        let uy = normalize(cross(uz, dir));
        let ux = normalize(cross(uy, uz));
        let ucs = Xform::from_axes(ux, uy, uz, m.insert);
        if complex {
            self.draw_mtext_complex(_e, m, props, &font, &ucs, style_w, style_obl);
        } else {
            self.draw_mtext_simple(_e, m, props, &font, &ucs);
        }
    }

    fn draw_mtext_simple(&mut self, e: &Entity, m: &MTextData, props: &Properties, font: &Font, ucs: &Xform) {
        let cap = if m.char_height > MIN_CAP_HEIGHT { m.char_height } else { 2.5 };
        let text = plain_mtext(&m.text);
        let box_w = if m.width == 0.0 { None } else { Some(m.width) };
        let lines = text_wrap(&text, box_w, &|s: &str| font.advance_width(s, cap, 1.0));
        if lines.is_empty() {
            return;
        }
        let (ha, va) = mtext_alignment(m.attachment);
        let spacing = cap * DEFAULT_LINE_SPACING * m.line_spacing_factor;
        let widths: Vec<f64> = lines.iter().map(|l| font.text_width(l, cap, 1.0)).collect();
        let fm = font.measurements(cap);
        let line_ys: Vec<f64> = (0..lines.len()).map(|i| -fm.baseline - (cap + i as f64 * spacing)).collect();
        let bw = box_w.unwrap_or_else(|| widths.iter().cloned().fold(0.0, f64::max));
        let last = *line_ys.last().unwrap_or(&0.0);
        let (anchor_x, xs): (f64, Vec<f64>) = match ha {
            HA::Left => (0.0, vec![0.0; widths.len()]),
            HA::Center => (bw / 2.0, widths.iter().map(|w| bw / 2.0 - w / 2.0).collect()),
            HA::Right => (bw, widths.iter().map(|w| bw - w).collect()),
        };
        let anchor_y = match va {
            VA::Top => 0.0,
            VA::LowerCaseCenter => (line_ys[0] + fm.x_height + last) / 2.0,
            VA::UpperCaseCenter => (line_ys[0] + fm.cap_height + last) / 2.0,
            VA::Baseline => last,
            VA::Bottom => last - fm.descender,
        };
        let scale_y = if e.c.extrusion[2] < 0.0 { -1.0 } else { 1.0 };
        for (i, line) in lines.iter().enumerate() {
            let tx = Xform::translate([xs[i] - anchor_x, line_ys[i] - anchor_y, 0.0]).then(&Xform::scale([1.0, scale_y, 1.0])).then(ucs);
            self.draw_text_line(line, &tx, props, cap, font, 1.0, true);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_mtext_complex(&mut self, e: &Entity, m: &MTextData, props: &Properties, font0: &Arc<Font>, ucs: &Xform, style_w: f64, style_obl: f64) {
        let cap = if m.char_height > MIN_CAP_HEIGHT { m.char_height } else { 2.5 };
        let aci = e.c.color;
        let mut base = Ctx::new(cap, style_w, style_obl, aci);
        if let Some(tc) = e.c.true_color {
            base.rgb = Some(rgb_from_u32(tc));
        }
        let (ha, _) = mtext_alignment(m.attachment);
        base.para.align = match ha {
            HA::Left => ParaAlign::Left,
            HA::Center => ParaAlign::Center,
            HA::Right => ParaAlign::Right,
        };
        let tokens = parse_mtext(&m.text, base.clone());
        let fonts_opts = self.fonts.clone();
        let f0 = font0.clone();
        let font_of = move |c: &Ctx| -> Arc<Font> {
            match &c.font_family {
                Some((family, bold, italic)) => {
                    let st = StyleDef { family: family.clone(), bold: *bold, italic: *italic, ..Default::default() };
                    font::font_for_style(Some(&st), &fonts_opts)
                }
                None => f0.clone(),
            }
        };
        // reference width: the defined width, else the width of the widest line laid out without wrapping
        let mut width = m.width;
        let unwrapped = width < 1e-6;
        if unwrapped {
            width = 1e9;
        }
        let columns = if m.column_count > 1 || m.column_type != 0 {
            let n = (m.column_count.max(1) as usize).min(mtext::MAX_COLUMNS);
            let heights: Vec<Option<f64>> = if !m.column_heights.is_empty() {
                // dynamic columns with manual heights: the last column has automatic height
                let mut h: Vec<Option<f64>> = m.column_heights.iter().take(mtext::MAX_COLUMNS).map(|&v| Some(v)).collect();
                if let Some(l) = h.last_mut() {
                    *l = None;
                }
                h
            } else if m.defined_height.abs() >= 1e-6 {
                vec![Some(m.defined_height.abs()); n]
            } else {
                vec![None; n]
            };
            Some((n, m.column_width, m.column_gutter, heights))
        } else {
            None
        };
        let mut opt = LayoutOptions { width, attachment: if (1..=9).contains(&m.attachment) { m.attachment } else { 1 }, line_spacing: m.line_spacing_factor, base_ctx: base, columns };
        let mut laid = layout(&tokens, &opt, &font_of);
        if unwrapped {
            opt.width = laid.max_line.max(cap * 1e-3) * (1.0 + 1e-9);
            laid = layout(&tokens, &opt, &font_of);
        }
        let cmds = laid.cmds;
        let scale_y = if e.c.extrusion[2] < 0.0 { -1.0 } else { 1.0 };
        for c in cmds {
            match c {
                mtext::Cmd::Word { text, left, bottom, ctx } => {
                    let sx = if (ctx.width_factor - 1.0).abs() > 1e-6 { ctx.width_factor } else { 1.0 };
                    let _ = sx;
                    let tan = if ctx.oblique.abs() > 1e-3 { ctx.oblique.to_radians().tan() } else { 0.0 };
                    // x' = x + tan * y + left, y' = y + bottom (the width factor is part of the glyph advance)
                    let shear = Xform { m: [[1.0, tan, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [left, bottom * scale_y, 0.0] };
                    let tx = shear.then(&Xform::scale([1.0, scale_y, 1.0])).then(ucs);
                    let mut p = props.clone();
                    if let Some(rgb) = ctx.rgb {
                        p.color = rgba(rgb, 255);
                        p.role = crate::props::Role::None;
                    } else {
                        let c = self.ctx.resolve_aci_color(ctx.aci, p.layer);
                        p.color = rgba(c, 255);
                        p.role = self.ctx.aci_role(ctx.aci, p.layer);
                    }
                    let f = font_of(&ctx);
                    self.draw_text_line(&text, &tx, &p, ctx.cap_height, &f, ctx.width_factor, true);
                }
                mtext::Cmd::Line { a, b } => {
                    let a = ucs.apply([a[0], a[1] * scale_y, 0.0]);
                    let b = ucs.apply([b[0], b[1] * scale_y, 0.0]);
                    self.out_line(a, b, props);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_characters_in_single_line_text() {
        assert_eq!(plain_text("%%c25 %%d %%p1"), "Ø25 ° ±1");
        assert_eq!(plain_text("a%%ub"), "ab");
        assert_eq!(plain_text("100%"), "100%");
    }

    #[test]
    fn alignment_flags() {
        assert_eq!(text_alignment(0, 0).2, 0);
        assert_eq!(text_alignment(3, 0).2, 3);
        assert_eq!(text_alignment(5, 2).2, 5);
        assert_eq!(text_alignment(1, 2).0, HA::Center);
        assert_eq!(text_alignment(1, 2).1, VA::UpperCaseCenter);
        assert_eq!(mtext_alignment(5), (HA::Center, VA::LowerCaseCenter));
        assert_eq!(mtext_alignment(9), (HA::Right, VA::Bottom));
    }
}
