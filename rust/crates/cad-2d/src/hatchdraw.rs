//! HATCH: boundary loops, solid filling and pattern lines (ezdxf `draw_hatch_entity`, `path.from_hatch_*` and `render.hatching`).
use crate::bspline::*;
use crate::colors::{rgb_from_u32, rgba};
use crate::ctl::Ctl;
use crate::dxf::*;
use crate::frontend::Frontend;
use crate::geom::*;
use crate::props::Properties;
use std::f64::consts::TAU;

pub const BOUNDARY_EXTERNAL: i32 = 1;
pub const BOUNDARY_TEXTBOX: i32 = 8;
pub const BOUNDARY_OUTERMOST: i32 = 16;
/// narrower pattern lines are rendered as solid filling (`Configuration.min_hatch_line_distance`)
pub const MIN_HATCH_LINE_DISTANCE: f64 = 1e-4;
/// more pattern lines than this for one definition are rendered as solid filling
pub const MAX_PATTERN_LINES: f64 = 200_000.0;
/// more pattern segments (dashes) than this for one HATCH are rendered as solid filling: a dash of 0.0002 along a
/// line of 1000 units alone is 5 000 000 segments
pub const MAX_PATTERN_SEGMENTS: f64 = 1_000_000.0;
/// seconds a single HATCH may take for all its pattern lines together (`Configuration.hatching_timeout`); then it is
/// filled solid
pub const HATCHING_TIMEOUT: f64 = 30.0;
/// Most crossings of ring edges with pattern lines one pattern definition collects (16 bytes each); more is a pattern
/// that is far too dense for the shape, and the hatch is filled solid.
pub const MAX_CROSSINGS: usize = 4_000_000;

fn angle_to_param(ratio: f64, angle: f64) -> f64 {
    fmod((angle.sin() / ratio).atan2(angle.cos()), TAU)
}

/// Path of one boundary path in WCS (possibly several loops as sub paths).
pub fn boundary_path(bp: &HatchPath, ocs: &Ocs, elevation: f64) -> Path {
    match bp {
        HatchPath::Poly { closed, vertices, .. } => {
            let pts: Vec<(f64, f64, f64)> = vertices.iter().map(|v| (v[0], v[1], v[2])).collect();
            polyline_path(&pts, *closed, ocs, elevation)
        }
        HatchPath::Edges { edges, .. } => edge_path(edges, ocs, elevation),
    }
}

fn edge_path(edges: &[HatchEdge], ocs: &Ocs, elevation: f64) -> Path {
    let wcs = |p: V2| -> V3 { ocs.to_wcs([p[0], p[1], elevation]) };
    let wcs_t = |p: V2| -> V3 { ocs.to_wcs([p[0], p[1], 0.0]) };
    let ext = ocs.uz;
    let mut path = Path::default();
    let mut have_path = false;
    let mut lp: Option<Path> = None;
    for edge in edges {
        let seg: Option<Path> = match edge {
            HatchEdge::Line { start, end } => {
                let mut s = Path::new(wcs(*start));
                s.line_to(wcs(*end));
                Some(s)
            }
            HatchEdge::Arc { center, radius, start, end, ccw } => {
                if radius.abs() > 1e-9 {
                    let (a0, a1) = if *ccw { (*start, *end) } else { (360.0 - end, 360.0 - start) };
                    Some(arc_path([center[0], center[1], elevation], *radius, ext, a0, a1))
                } else {
                    None
                }
            }
            HatchEdge::Ellipse { center, major, ratio, start, end, ccw } => {
                if major[0].abs() + major[1].abs() > 1e-12 && *ratio > 0.0 {
                    let (a0, a1) = if *ccw { (*start, *end) } else { (360.0 - end, 360.0 - start) };
                    let (p0, p1) = (angle_to_param(*ratio, a0.to_radians()), angle_to_param(*ratio, a1.to_radians()));
                    let c = wcs(*center);
                    let maj = wcs_t(*major);
                    let minr = minor_axis(maj, ext, *ratio);
                    let mut s = Path::default();
                    add_ellipse(&mut s, c, maj, minr, p0, p1, 1, true);
                    Some(s)
                } else {
                    None
                }
            }
            HatchEdge::Spline { degree, knots, control_points, weights, fit_points, start_tangent, end_tangent, .. } => {
                let mut s = Path::default();
                if !control_points.is_empty() {
                    let cps: Vec<V3> = control_points.iter().map(|&p| wcs(p)).collect();
                    match BSpline::new(cps, (*degree).max(1) as usize, knots.clone(), weights.clone()) {
                        Some(bs) => {
                            s.start = bs.point(bs.domain().0);
                            let mut done = false;
                            if bs.degree == 3 && !bs.is_rational() && bs.is_clamped() {
                                if let Some(segs) = bs.bezier_decomposition() {
                                    let curves: Vec<[V3; 4]> = segs.into_iter().filter(|c| c.len() == 4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
                                    add_bezier4p(&mut s, curves);
                                    done = true;
                                }
                            }
                            if !done {
                                for p in bs.sample(16).iter().skip(1) {
                                    s.line_to(*p);
                                }
                            }
                            Some(s)
                        }
                        None => None,
                    }
                } else if fit_points.len() >= 2 {
                    let fps: Vec<V3> = fit_points.iter().map(|&p| wcs(p)).collect();
                    let t = match (start_tangent, end_tangent) {
                        (Some(a), Some(b)) => Some((wcs_t(*a), wcs_t(*b))),
                        _ => None,
                    };
                    let pts = fit_points_curve(&fps, t, 16);
                    s.start = pts[0];
                    for p in pts.iter().skip(1) {
                        s.line_to(*p);
                    }
                    Some(s)
                } else {
                    None
                }
            }
        };
        let Some(mut next) = seg else { continue };
        if next.is_empty() && v3_close(next.start, NULL3) && false {
            continue;
        }
        match lp.take() {
            None => lp = Some(next),
            Some(mut l) => {
                if v3_close(l.end(), next.start) {
                    l.append_path(&next);
                    lp = Some(l);
                } else if v3_close(l.end(), next.end()) {
                    l.append_path(&next.reversed());
                    lp = Some(l);
                } else if v3_close(l.start, next.end()) {
                    next.append_path(&l);
                    lp = Some(next);
                } else if v3_close(l.start, next.start) {
                    let mut r = l.reversed();
                    r.append_path(&next);
                    lp = Some(r);
                } else if l.is_closed() {
                    // start a new loop
                    if !have_path {
                        path = l;
                        have_path = true;
                    } else {
                        path.move_to(l.start);
                        path.cmds.extend(l.cmds);
                    }
                    lp = Some(next);
                } else {
                    // close the gap by a straight line
                    l.append_path(&next);
                    lp = Some(l);
                }
            }
        }
    }
    if let Some(mut l) = lp {
        l.close();
        if !have_path {
            path = l;
        } else {
            path.move_to(l.start);
            path.cmds.extend(l.cmds);
        }
    }
    path
}

/// Pattern lines of one definition inside the rings (even-odd), dashes included, appended to `out`. `Err`: the pattern
/// is too dense (lines closer than `MIN_HATCH_LINE_DISTANCE`, more lines, segments or crossings than the limits) or
/// `ctl` stopped (time limit of the hatch, cancel): the caller fills the hatch solid.
pub(crate) fn pattern_lines_render(rings: &[Vec<V2>], pl: &PatternLine, out: &mut Vec<(V2, V2)>, ctl: &Ctl) -> Result<(), ()> {
    if ctl.halt().is_some() {
        return Err(());
    }
    let a = pl.angle.to_radians();
    let d = [a.cos(), a.sin()];
    let n = [-a.sin(), a.cos()];
    let spacing = pl.offset[0] * n[0] + pl.offset[1] * n[1];
    let shift = pl.offset[0] * d[0] + pl.offset[1] * d[1];
    if spacing.abs() < MIN_HATCH_LINE_DISTANCE {
        return Err(());
    }
    // extent of the loops along the normal
    let (mut smin, mut smax) = (f64::INFINITY, f64::NEG_INFINITY);
    for r in rings {
        for p in r {
            let s = (p[0] - pl.base[0]) * n[0] + (p[1] - pl.base[1]) * n[1];
            smin = smin.min(s);
            smax = smax.max(s);
        }
    }
    if !smin.is_finite() {
        return Ok(());
    }
    // the number of lines is counted in f64: converted to i64 first, the indices of a 1e15 box at the distance 1e-4
    // saturate and their difference wraps around to a small number
    let (lo, hi) = {
        let (a, b) = (smin / spacing, smax / spacing);
        (a.min(b).floor(), a.max(b).ceil())
    };
    if !(hi - lo <= MAX_PATTERN_LINES) {
        return Err(());
    }
    let (i0, i1) = (lo as i64, hi as i64);
    let cycle: f64 = pl.dashes.iter().map(|x| x.abs()).sum();
    let origin_of = |i: i64| [pl.base[0] + pl.offset[0] * i as f64, pl.base[1] + pl.offset[1] * i as f64];
    // Crossings of the ring edges with the lines, as (line, parameter along the line). Every edge is tested only with
    // the lines between its end points: a shape of two small rings far apart, or a ring of 20 000 edges next to a line
    // pattern of 200 000 lines, used to cost lines x edges (minutes); it costs the edges and the crossings now.
    let mut cross: Vec<(i64, f64)> = Vec::new();
    let mut tick = 0u32;
    let (f0, f1) = (i0 as f64, i1 as f64);
    for r in rings {
        let m = r.len();
        for k in 0..m {
            tick += 1;
            if tick % 2048 == 0 && ctl.halt().is_some() {
                return Err(());
            }
            let (p, q) = (r[k], r[(k + 1) % m]);
            let sp = ((p[0] - pl.base[0]) * n[0] + (p[1] - pl.base[1]) * n[1]) / spacing;
            let sq = ((q[0] - pl.base[0]) * n[0] + (q[1] - pl.base[1]) * n[1]) / spacing;
            if !(sp.is_finite() && sq.is_finite()) {
                continue;
            }
            // one line more on each side: the exact test below decides
            let first = (sp.min(sq).floor() - 1.0).max(f0);
            let last = (sp.max(sq).ceil() + 1.0).min(f1);
            if first > last {
                continue;
            }
            for i in (first as i64)..=(last as i64) {
                let origin = origin_of(i);
                let spl = (p[0] - origin[0]) * n[0] + (p[1] - origin[1]) * n[1];
                let sql = (q[0] - origin[0]) * n[0] + (q[1] - origin[1]) * n[1];
                if (spl > 0.0) != (sql > 0.0) {
                    let t = spl / (spl - sql);
                    let x = [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t];
                    cross.push((i, (x[0] - origin[0]) * d[0] + (x[1] - origin[1]) * d[1]));
                    if cross.len() > MAX_CROSSINGS {
                        return Err(());
                    }
                }
            }
        }
    }
    cross.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)));
    let _ = shift;
    let mut g0 = 0;
    let mut lines_done = 0u32;
    while g0 < cross.len() {
        let i = cross[g0].0;
        let mut g1 = g0;
        while g1 < cross.len() && cross[g1].0 == i {
            g1 += 1;
        }
        let ts: Vec<f64> = cross[g0..g1].iter().map(|c| c.1).collect();
        g0 = g1;
        lines_done += 1;
        if lines_done % 256 == 0 && ctl.halt().is_some() {
            return Err(());
        }
        if ts.len() < 2 {
            continue;
        }
        let origin = origin_of(i);
        for pair in ts.chunks(2) {
            if pair.len() < 2 {
                break;
            }
            let (t0, t1) = (pair[0], pair[1]);
            let pt = |t: f64| [origin[0] + d[0] * t, origin[1] + d[1] * t];
            if pl.dashes.is_empty() || cycle < 1e-12 {
                if out.len() as f64 + 1.0 > MAX_PATTERN_SEGMENTS {
                    return Err(());
                }
                out.push((pt(t0), pt(t1)));
                continue;
            }
            // segments this piece of the line needs (a dot may add a second one per element), counted before anything
            // is generated; too many: the whole hatch is filled solid
            let needed = (t1 - t0) / cycle * 2.0 * pl.dashes.len() as f64 + 2.0;
            if !(out.len() as f64 + needed <= MAX_PATTERN_SEGMENTS) {
                return Err(());
            }
            // dash pattern anchored at the origin of the line (t = 0)
            let mut pos = fmod(t0, cycle);
            let mut t = t0;
            // find the pattern element containing `pos`
            let mut idx = 0;
            let mut acc = 0.0;
            for (k, dl) in pl.dashes.iter().enumerate() {
                if pos < acc + dl.abs() || k + 1 == pl.dashes.len() {
                    idx = k;
                    break;
                }
                acc += dl.abs();
            }
            let mut left = pl.dashes[idx].abs() - (pos - acc);
            if left < 0.0 {
                left = 0.0;
            }
            pos = 0.0;
            let _ = pos;
            let mut guard = 0u32;
            while t < t1 && guard < 5_000_000 {
                guard += 1;
                let dl = pl.dashes[idx];
                let seg = left.min(t1 - t);
                if dl > 0.0 {
                    out.push((pt(t), pt(t + seg)));
                } else if dl == 0.0 {
                    out.push((pt(t), pt(t)));
                }
                t += seg;
                idx = (idx + 1) % pl.dashes.len();
                left = pl.dashes[idx].abs();
                if left == 0.0 && pl.dashes[idx] == 0.0 {
                    // a dot takes no length: emit it and move on
                    out.push((pt(t), pt(t)));
                    idx = (idx + 1) % pl.dashes.len();
                    left = pl.dashes[idx].abs();
                }
            }
        }
    }
    Ok(())
}

impl<'a> Frontend<'a> {
    pub(crate) fn draw_hatch(&mut self, e: &Entity, h: &HatchData, ocs: &Ocs, props: &Properties) {
        let _ = e;
        if h.mpolygon {
            self.draw_mpolygon(h, ocs, props);
            return;
        }
        // boundary paths to render, ordered external, outermost, others
        let rank = |f: i32| if f & BOUNDARY_EXTERNAL != 0 { 0 } else if f & BOUNDARY_OUTERMOST != 0 { 1 } else { 2 };
        let ignore = match h.hatch_style {
            0 => 3,
            1 => 2,
            _ => 1,
        };
        let mut idx: Vec<(i32, usize)> = h.paths.iter().enumerate().map(|(i, p)| (rank(p.flags()), i)).collect();
        idx.sort();
        let used: Vec<&HatchPath> = idx.into_iter().filter(|(r, _)| *r < ignore).map(|(_, i)| &h.paths[i]).collect();
        let mut loops: Vec<Path> = Vec::new();
        for bp in &used {
            if bp.flags() & BOUNDARY_TEXTBOX != 0 {
                continue;
            }
            let p = boundary_path(bp, ocs, h.elevation);
            for mut sp in p.sub_paths() {
                if !sp.is_empty() {
                    sp.close();
                    loops.push(sp);
                }
            }
        }
        if loops.is_empty() {
            return;
        }
        if !h.solid {
            // pattern fill
            if h.pattern.is_empty() {
                return;
            }
            if self.draw_pattern_lines(h, ocs, &loops, props) {
                return;
            }
            // dense pattern: solid fill below
        }
        self.out_filled_paths(&loops, props);
    }

    /// Pattern lines of a hatch inside the loops. False when the pattern is too dense (more lines or dashes than
    /// `MAX_PATTERN_LINES` / `MAX_PATTERN_SEGMENTS`, or lines closer than `MIN_HATCH_LINE_DISTANCE`): nothing is drawn then
    /// and the caller fills the loops solid, as ezdxf does after its `DenseHatchingLinesError`.
    fn draw_pattern_lines(&mut self, h: &HatchData, ocs: &Ocs, loops: &[Path], props: &Properties) -> bool {
        // The pattern lines of all hatches share the budget of the dashes of lines (`DASH_SOFT_ELEMS`, lowered with the
        // memory of the computer): forty hatches that are each under their own limit are a scene of 63 million vertices, 5 GB.
        // Once the budget is used up the hatches that follow are filled solid, with the Czech note, as one that is too dense.
        if self.dash_elems >= self.rec.soft_dash() {
            self.hatch_solid += 1;
            return false;
        }
        // loops flattened in the OCS plane (xy of the WCS loop is the plane of the hatch for the default OCS)
        let rings: Vec<Vec<V2>> = loops
            .iter()
            .map(|p| {
                let pts = p.flatten(MAX_FLATTENING, 4);
                pts.iter()
                    .map(|q| {
                        let o = ocs.from_wcs(*q);
                        [o[0], o[1]]
                    })
                    .collect()
            })
            .collect();
        let mut lines: Vec<(V2, V2)> = Vec::new();
        // one time limit for all the pattern lines of the hatch (it used to start again for every pattern line, so a
        // hatch with 64 of them could take 64 times as long), inside the time limit and the cancel flag of the scene
        let ctl = self.ctl.tighter(self.hatch_timeout);
        for pl in &h.pattern {
            if pattern_lines_render(&rings, pl, &mut lines, &ctl).is_err() {
                self.hatch_solid += 1;
                return false;
            }
        }
        let need = lines.len().saturating_mul(2);
        if self.dash_elems.saturating_add(need) > self.rec.soft_dash() || need > self.rec.elems_left() {
            self.hatch_solid += 1;
            return false;
        }
        self.dash_elems = self.dash_elems.saturating_add(need);
        let mut p = props.clone();
        p.pattern = std::rc::Rc::new(Vec::new());
        let seg: Vec<(V3, V3)> = lines.iter().map(|&(a, b)| (ocs.to_wcs([a[0], a[1], h.elevation]), ocs.to_wcs([b[0], b[1], h.elevation]))).collect();
        // zero length pairs are dots
        self.out_solid_lines(&seg, &p);
        true
    }

    /// MPOLYGON (ezdxf `draw_mpolygon_entity`): every boundary path is used (no hatch style), shifted by the offset
    /// vector. The fill comes first, then the boundary paths as lines in the colour of the entity.
    fn draw_mpolygon(&mut self, h: &HatchData, ocs: &Ocs, props: &Properties) {
        let shift = h
            .offset
            .filter(|o| o.iter().any(|v| *v != 0.0))
            .map(|o| Xform::translate(ocs.to_wcs([o[0], o[1], h.elevation])));
        // (loop, is a text box)
        let mut loops: Vec<(Path, bool)> = Vec::new();
        for bp in &h.paths {
            let mut p = boundary_path(bp, ocs, h.elevation);
            if let Some(t) = &shift {
                p = p.transformed(t);
            }
            for mut sp in p.sub_paths() {
                if !sp.is_empty() {
                    sp.close();
                    loops.push((sp, bp.flags() & BOUNDARY_TEXTBOX != 0));
                }
            }
        }
        if loops.is_empty() {
            return;
        }
        let fillable: Vec<Path> = loops.iter().filter(|(_, text_box)| !text_box).map(|(p, _)| p.clone()).collect();
        if h.solid {
            // a gradient record with colours carries the fill as a true colour, else the fill colour is an ACI value
            let mut fill = props.clone();
            fill.color = match h.gradient_color1 {
                Some(tc) if h.gradient_colors > 0 => rgba(rgb_from_u32(tc), 255),
                _ => rgba(self.ctx.resolve_aci_color(h.fill_color.unwrap_or(256), props.layer), 255),
            };
            fill.role = match h.gradient_color1 {
                Some(_) if h.gradient_colors > 0 => crate::props::Role::None,
                _ => self.ctx.aci_role(h.fill_color.unwrap_or(256), props.layer),
            };
            if !fillable.is_empty() {
                self.out_filled_paths(&fillable, &fill);
            }
        } else if !h.pattern_name.is_empty() && !h.pattern_name.eq_ignore_ascii_case("SOLID") && !h.pattern.is_empty() && !fillable.is_empty() {
            if !self.draw_pattern_lines(h, ocs, &fillable, props) {
                self.out_filled_paths(&fillable, props);
            }
        }
        for (p, _) in &loops {
            self.out_path(p, props);
        }
    }
}

const MAX_FLATTENING: f64 = 0.01;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_lines_cross_a_square() {
        let sq = vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]];
        // 45 degree lines, spacing 2 along the normal
        let pl = PatternLine { angle: 45.0, base: [0.0, 0.0], offset: [-2f64.sqrt(), 2f64.sqrt()], dashes: vec![] };
        let mut out = Vec::new();
        pattern_lines_render(&sq, &pl, &mut out, &Ctl::default()).unwrap();
        assert!(out.len() >= 7 && out.len() <= 9, "{}", out.len());
        for (a, b) in &out {
            assert!(a[0] >= -1e-9 && a[0] <= 10.0 + 1e-9 && b[1] >= -1e-9 && b[1] <= 10.0 + 1e-9);
        }
    }

    #[test]
    fn dashed_pattern_lines_have_gaps() {
        let sq = vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]];
        let pl = PatternLine { angle: 0.0, base: [0.0, 0.0], offset: [0.0, 5.0], dashes: vec![2.0, -1.0] };
        let mut out = Vec::new();
        pattern_lines_render(&sq, &pl, &mut out, &Ctl::default()).unwrap();
        // lines at y = 0 (edge, half open rule), 5, 10: each 10 wide with 3 dash/gap cycles of length 3
        let at5: Vec<_> = out.iter().filter(|(a, _)| (a[1] - 5.0).abs() < 1e-9).collect();
        assert!(at5.len() >= 3 && at5.len() <= 4, "{:?}", at5);
        assert!((at5[0].1[0] - at5[0].0[0] - 2.0).abs() < 1e-9);
    }

    #[test]
    fn dense_pattern_is_refused() {
        let sq = vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]];
        let pl = PatternLine { angle: 0.0, base: [0.0, 0.0], offset: [0.0, 1e-6], dashes: vec![] };
        assert!(pattern_lines_render(&sq, &pl, &mut Vec::new(), &Ctl::default()).is_err());
    }

    #[test]
    fn edge_loop_from_arc_and_line() {
        let ocs = Ocs::new(Z_AXIS);
        let edges = vec![
            HatchEdge::Arc { center: [0.0, 0.0], radius: 5.0, start: 0.0, end: 180.0, ccw: true },
            HatchEdge::Line { start: [-5.0, 0.0], end: [5.0, 0.0] },
        ];
        let p = edge_path(&edges, &ocs, 0.0);
        assert!(p.is_closed());
        assert!(!p.has_sub_paths());
    }
}
