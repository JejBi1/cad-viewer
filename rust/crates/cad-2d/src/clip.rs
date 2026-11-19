//! Clipping shapes for viewports (paper space coordinates): rectangles and polygons.
use crate::geom::*;

#[derive(Clone, Debug)]
pub enum ClipShape {
    Rect([f64; 4]),
    /// A polygon; `tris` (concave polygons only) are its triangles: every triangle is a convex clipping window.
    Poly { pts: Vec<V2>, convex: bool, bbox: [f64; 4], tris: Vec<[V2; 3]>, index: Option<EdgeIndex> },
    /// Everything inside the rectangle `outer` except the polygon `inner` (a block clipping path that was inverted, ezdxf
    /// `InvertedClippingPolygon`); `tris` triangulate that area.
    Inverted { inner: Vec<V2>, inner_bbox: [f64; 4], outer: [f64; 4], tris: Vec<[V2; 3]>, index: Option<EdgeIndex> },
}

/// The edges of a ring sorted into horizontal slabs: the edges that can touch a point or a short segment are found
/// without looking at all of them. A viewport clipped by a circle of radius 1e11 has a ring of two million edges, and
/// every line of the model space was tested against all of them.
#[derive(Clone, Debug)]
pub struct EdgeIndex {
    y0: f64,
    y1: f64,
    inv_h: f64,
    slabs: Vec<Vec<u32>>,
}

impl EdgeIndex {
    /// Rings with fewer edges are searched edge by edge.
    pub const MIN_EDGES: usize = 48;

    /// `None` for short rings and rings with a coordinate that is not a number.
    pub fn build(ring: &[V2]) -> Option<EdgeIndex> {
        let n = ring.len();
        if n < Self::MIN_EDGES {
            return None;
        }
        let (mut y0, mut y1) = (f64::INFINITY, f64::NEG_INFINITY);
        for p in ring {
            if !p[0].is_finite() || !p[1].is_finite() {
                return None;
            }
            y0 = y0.min(p[1]);
            y1 = y1.max(p[1]);
        }
        let h = y1 - y0;
        if !(h > 0.0) || !h.is_finite() {
            return None;
        }
        // about one slab per four edges; fewer when long edges would be listed in too many of them
        let mut ns = (n / 4).clamp(8, 16_384);
        loop {
            let inv = ns as f64 / h;
            let slab = |y: f64| (((y - y0) * inv) as usize).min(ns - 1);
            let mut entries = 0usize;
            for i in 0..n {
                let (a, b) = (ring[i], ring[(i + 1) % n]);
                entries += slab(a[1].max(b[1])) - slab(a[1].min(b[1])) + 1;
            }
            if entries <= 12 * n || ns <= 8 {
                break;
            }
            ns = (ns / 2).max(8);
        }
        let inv_h = ns as f64 / h;
        let slab = |y: f64| (((y - y0) * inv_h) as usize).min(ns - 1);
        let mut slabs: Vec<Vec<u32>> = vec![Vec::new(); ns];
        for i in 0..n {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            for s in slab(a[1].min(b[1]))..=slab(a[1].max(b[1])) {
                slabs[s].push(i as u32);
            }
        }
        Some(EdgeIndex { y0, y1, inv_h, slabs })
    }

    /// Edges (`i`: ring[i] to ring[i + 1]) that may reach into the band `ya..=yb`, each once; `out` is replaced.
    pub fn candidates(&self, ya: f64, yb: f64, out: &mut Vec<u32>) {
        out.clear();
        let (ya, yb) = (ya.min(yb), ya.max(yb));
        if !(yb >= self.y0 && ya <= self.y1) {
            return;
        }
        let ns = self.slabs.len();
        let slab = |y: f64| (((y - self.y0).max(0.0) * self.inv_h) as usize).min(ns - 1);
        let (s0, s1) = (slab(ya), slab(yb));
        if s0 == s1 {
            out.extend_from_slice(&self.slabs[s0]);
            return;
        }
        for s in s0..=s1 {
            out.extend_from_slice(&self.slabs[s]);
        }
        out.sort_unstable();
        out.dedup();
    }
}

/// Tolerance of "the point lies on an edge": 1e-9 of the size of the shape.
fn boundary_eps(b: &[f64; 4]) -> f64 {
    1e-9 * (b[2] - b[0]).max(b[3] - b[1]).max(1.0)
}

fn is_convex(p: &[V2]) -> bool {
    let n = p.len();
    if n < 3 {
        return false;
    }
    let mut sign = 0i32;
    for i in 0..n {
        let (a, b, c) = (p[i], p[(i + 1) % n], p[(i + 2) % n]);
        let cr = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        let s = if cr > 1e-12 {
            1
        } else if cr < -1e-12 {
            -1
        } else {
            0
        };
        if s != 0 {
            if sign != 0 && s != sign {
                return false;
            }
            sign = s;
        }
    }
    true
}

fn bounds_of(pts: &[V2]) -> [f64; 4] {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in pts {
        b[0] = b[0].min(p[0]);
        b[1] = b[1].min(p[1]);
        b[2] = b[2].max(p[0]);
        b[3] = b[3].max(p[1]);
    }
    b
}

fn boxes_overlap(a: &[f64; 4], b: &[f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

/// The segment a-b may touch the box (its bounding box overlaps it).
fn segment_may_touch(a: V2, b: V2, r: &[f64; 4]) -> bool {
    a[0].max(b[0]) >= r[0] && a[0].min(b[0]) <= r[2] && a[1].max(b[1]) >= r[1] && a[1].min(b[1]) <= r[3]
}

fn rect_ring(r: &[f64; 4]) -> Vec<V2> {
    vec![[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]]
}

/// The point lies on an edge of the ring (within 1e-9 of its size): lines along the edge of a clipping path stay (ezdxf
/// treats a segment that is collinear with an edge of the clipping polygon as inside).
fn on_boundary(p: V2, ring: &[V2], b: &[f64; 4], index: Option<&EdgeIndex>) -> bool {
    let eps = boundary_eps(b);
    let n = ring.len();
    match index {
        None => (0..n).any(|i| seg_dist2(p, ring[i], ring[(i + 1) % n]) <= eps * eps),
        Some(ix) => {
            let mut c = Vec::new();
            ix.candidates(p[1] - eps, p[1] + eps, &mut c);
            c.iter().any(|&i| seg_dist2(p, ring[i as usize], ring[(i as usize + 1) % n]) <= eps * eps)
        }
    }
}

/// `point_in_ring`, looking only at the edges that reach the height of the point when there is an index.
fn inside_ring(p: V2, ring: &[V2], index: Option<&EdgeIndex>) -> bool {
    let Some(ix) = index else { return point_in_ring(p, ring) };
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut c = Vec::new();
    ix.candidates(p[1], p[1], &mut c);
    let mut inside = false;
    for &i in &c {
        // the edge a-b of `point_in_ring`: ring[i] and the ring point before it; here the edge ring[i] -> ring[i + 1]
        let (a, b) = (ring[i as usize], ring[(i as usize + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
    }
    inside
}

fn triangles(rings: &[&[V2]]) -> Vec<[V2; 3]> {
    crate::tess::triangulate_with_holes(rings).chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect()
}

/// Sutherland-Hodgman: the ring clipped by the convex window.
fn sh_clip(ring: &[V2], window: &[V2]) -> Vec<V2> {
    let mut poly: Vec<V2> = ring.to_vec();
    let n = window.len();
    let ccw = ring_area(window) >= 0.0;
    for i in 0..n {
        let (a, b) = if ccw { (window[i], window[(i + 1) % n]) } else { (window[(i + 1) % n], window[i]) };
        if poly.is_empty() {
            break;
        }
        let inside = |p: V2| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]) >= 0.0;
        let mut out: Vec<V2> = Vec::new();
        let m = poly.len();
        for k in 0..m {
            let (cur, prev) = (poly[k], poly[(k + m - 1) % m]);
            let (ci, pi) = (inside(cur), inside(prev));
            if ci {
                if !pi {
                    if let Some(x) = line_intersection(prev, cur, a, b) {
                        out.push(x);
                    }
                }
                out.push(cur);
            } else if pi {
                if let Some(x) = line_intersection(prev, cur, a, b) {
                    out.push(x);
                }
            }
        }
        poly = out;
    }
    poly
}

/// Distance of `q` from the infinite line through `a` and `b` and the position of its foot point along a-b (0 at `a`, 1 at
/// `b`); `None` for a degenerate segment.
fn line_distance(q: V2, a: V2, b: V2) -> Option<(f64, f64)> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    if !(l2 > 0.0) || !l2.is_finite() {
        return None;
    }
    let l = l2.sqrt();
    let dist = ((q[0] - a[0]) * dy - (q[1] - a[1]) * dx).abs() / l;
    let t = ((q[0] - a[0]) * dx + (q[1] - a[1]) * dy) / l2;
    Some((dist, t))
}

/// The parts of the segment a-b that `keep` accepts, split where the segment crosses an edge of `ring` and where an edge
/// that lies along the segment begins and ends (what is along an edge is on the boundary, what is on the extension of the
/// edge beyond its ends is not: only the overlap with the edge stays, as in ezdxf). `rb` is the bounding box of the ring.
fn split_line(a: V2, b: V2, ring: &[V2], rb: &[f64; 4], index: Option<&EdgeIndex>, keep: &dyn Fn(V2) -> bool) -> Vec<(V2, V2)> {
    let mut ts: Vec<f64> = vec![0.0, 1.0];
    let n = ring.len();
    let eps = boundary_eps(rb);
    let sb = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
    let mut edge = |i: usize| {
        let (c, d) = (ring[i], ring[(i + 1) % n]);
        // edges far from the segment cannot cross it
        if c[0].max(d[0]) < sb[0] - eps || c[0].min(d[0]) > sb[2] + eps || c[1].max(d[1]) < sb[1] - eps || c[1].min(d[1]) > sb[3] + eps {
            return;
        }
        if let Some(t) = seg_intersect_param(a, b, c, d) {
            ts.push(t);
        }
        // an edge along the segment: its ends cut the segment
        if let (Some((dc, tc)), Some((dd, td))) = (line_distance(c, a, b), line_distance(d, a, b)) {
            if dc <= eps && dd <= eps {
                for t in [tc, td] {
                    if t > 0.0 && t < 1.0 {
                        ts.push(t);
                    }
                }
            }
        }
    };
    match index {
        None => (0..n).for_each(&mut edge),
        Some(ix) => {
            let mut cand = Vec::new();
            ix.candidates(sb[1] - eps, sb[3] + eps, &mut cand);
            cand.iter().for_each(|&i| edge(i as usize));
        }
    }
    ts.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<(V2, V2)> = Vec::new();
    for w in ts.windows(2) {
        if w[1] - w[0] < 1e-12 {
            continue;
        }
        let (p, q) = (lerp2(a, b, w[0]), lerp2(a, b, w[1]));
        let m = lerp2(a, b, 0.5 * (w[0] + w[1]));
        if keep(m) {
            if let Some(last) = out.last_mut() {
                if v2_close(last.1, p) {
                    last.1 = q;
                    continue;
                }
            }
            out.push((p, q));
        }
    }
    out
}

impl ClipShape {
    pub fn polygon(mut pts: Vec<V2>) -> Option<ClipShape> {
        if pts.len() > 1 && v2_close(pts[0], pts[pts.len() - 1]) {
            pts.pop();
        }
        if pts.len() < 3 {
            return None;
        }
        let b = bounds_of(&pts);
        let axis_rect = pts.len() == 4
            && ((pts[0][0] == pts[1][0] && pts[1][1] == pts[2][1] && pts[2][0] == pts[3][0] && pts[3][1] == pts[0][1])
                || (pts[0][1] == pts[1][1] && pts[1][0] == pts[2][0] && pts[2][1] == pts[3][1] && pts[3][0] == pts[0][0]));
        if axis_rect {
            return Some(ClipShape::Rect(b));
        }
        let convex = is_convex(&pts);
        let tris = if convex { Vec::new() } else { triangles(&[&pts]) };
        let index = EdgeIndex::build(&pts);
        Some(ClipShape::Poly { pts, convex, bbox: b, tris, index })
    }

    /// Everything inside `outer` ([x0, y0, x1, y1]) except the polygon `inner`. `None` for fewer than 3 points or an
    /// `outer` that is not finite. A rectangle that does not contain `inner` is enlarged to do so.
    pub fn inverted(mut inner: Vec<V2>, outer: [f64; 4]) -> Option<ClipShape> {
        if inner.len() > 1 && v2_close(inner[0], inner[inner.len() - 1]) {
            inner.pop();
        }
        if inner.len() < 3 || outer.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let inner_bbox = bounds_of(&inner);
        let outer = [outer[0].min(inner_bbox[0]), outer[1].min(inner_bbox[1]), outer[2].max(inner_bbox[2]), outer[3].max(inner_bbox[3])];
        let ring = rect_ring(&outer);
        let tris = triangles(&[&ring, &inner]);
        let index = EdgeIndex::build(&inner);
        Some(ClipShape::Inverted { inner, inner_bbox, outer, tris, index })
    }

    /// Filled shapes cannot be clipped ring by ring by this shape (a concave polygon, an inverted path): they are cut into
    /// triangles first, and each triangle is clipped (see `Frontend::out_filled_elems`).
    pub fn is_non_convex(&self) -> bool {
        matches!(self, ClipShape::Poly { convex: false, .. } | ClipShape::Inverted { .. })
    }

    /// Bounding box of what the shape lets through ([x0, y0, x1, y1]).
    pub fn bbox(&self) -> [f64; 4] {
        match self {
            ClipShape::Rect(r) => *r,
            ClipShape::Poly { bbox, .. } => *bbox,
            ClipShape::Inverted { outer, .. } => *outer,
        }
    }

    /// Nothing inside the box `b` can be seen through the shape (its bounding box does not overlap the one of the shape).
    /// Conservative: false means unknown.
    pub fn excludes_box(&self, b: [f64; 4]) -> bool {
        !boxes_overlap(&self.bbox(), &b)
    }

    /// All of the bounding box lies inside the shape (conservative: false means unknown).
    pub fn contains_box(&self, b: [f64; 4]) -> bool {
        match self {
            ClipShape::Rect(r) => b[0] >= r[0] && b[1] >= r[1] && b[2] <= r[2] && b[3] <= r[3],
            ClipShape::Poly { convex, .. } => {
                *convex && [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]].iter().all(|&p| self.contains(p))
            }
            ClipShape::Inverted { inner_bbox, outer, .. } => {
                b[0] >= outer[0] && b[1] >= outer[1] && b[2] <= outer[2] && b[3] <= outer[3] && !boxes_overlap(&b, inner_bbox)
            }
        }
    }

    pub fn contains(&self, p: V2) -> bool {
        match self {
            ClipShape::Rect(r) => p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3],
            ClipShape::Poly { pts, bbox, index, .. } => {
                p[0] >= bbox[0]
                    && p[0] <= bbox[2]
                    && p[1] >= bbox[1]
                    && p[1] <= bbox[3]
                    && (inside_ring(p, pts, index.as_ref()) || on_boundary(p, pts, bbox, index.as_ref()))
            }
            ClipShape::Inverted { inner, inner_bbox, outer, index, .. } => {
                let in_outer = p[0] >= outer[0] && p[0] <= outer[2] && p[1] >= outer[1] && p[1] <= outer[3];
                let in_inner = p[0] >= inner_bbox[0]
                    && p[0] <= inner_bbox[2]
                    && p[1] >= inner_bbox[1]
                    && p[1] <= inner_bbox[3]
                    && inside_ring(p, inner, index.as_ref())
                    && !on_boundary(p, inner, inner_bbox, index.as_ref());
                in_outer && !in_inner
            }
        }
    }

    pub fn clip_line(&self, a: V2, b: V2) -> Vec<(V2, V2)> {
        match self {
            ClipShape::Rect(r) => match liang_barsky(a, b, r) {
                Some((s, e)) => vec![(s, e)],
                None => Vec::new(),
            },
            ClipShape::Poly { pts, bbox, index, .. } => {
                // a segment that does not reach the bounding box is outside: no search along the edges at all
                if !segment_may_touch(a, b, bbox) {
                    return Vec::new();
                }
                split_line(a, b, pts, bbox, index.as_ref(), &|m| self.contains(m))
            }
            ClipShape::Inverted { inner, inner_bbox, outer, index, .. } => match liang_barsky(a, b, outer) {
                Some((s, e)) => {
                    if !segment_may_touch(s, e, inner_bbox) {
                        // far from the excluded polygon: all of it is inside
                        return vec![(s, e)];
                    }
                    split_line(s, e, inner, inner_bbox, index.as_ref(), &|m| self.contains(m))
                }
                None => Vec::new(),
            },
        }
    }

    /// Open polyline: the parts inside.
    pub fn clip_polyline(&self, pts: &[V2]) -> Vec<Vec<V2>> {
        let mut out: Vec<Vec<V2>> = Vec::new();
        let mut cur: Vec<V2> = Vec::new();
        for w in pts.windows(2) {
            let segs = self.clip_line(w[0], w[1]);
            let none = segs.is_empty();
            for (s, e) in segs {
                if let Some(last) = cur.last() {
                    if !v2_close(*last, s) {
                        out.push(std::mem::take(&mut cur));
                    }
                }
                if cur.is_empty() {
                    cur.push(s);
                }
                cur.push(e);
            }
            // an outside segment ends the current part
            if none && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out.retain(|p| p.len() > 1);
        out
    }

    /// Closed polygon (Sutherland-Hodgman; exact for convex clip shapes, concave shapes are clipped triangle by triangle).
    pub fn clip_polygon(&self, ring: &[V2]) -> Vec<Vec<V2>> {
        let single = |p: Vec<V2>| if p.len() < 3 { Vec::new() } else { vec![p] };
        let pieces = |tris: &[[V2; 3]]| -> Vec<Vec<V2>> {
            let rb = bounds_of(ring);
            tris.iter().filter(|t| boxes_overlap(&rb, &bounds_of(&t[..]))).map(|t| sh_clip(ring, &t[..])).filter(|p| p.len() >= 3).collect()
        };
        match self {
            ClipShape::Rect(r) => single(sh_clip(ring, &rect_ring(r))),
            ClipShape::Poly { pts, convex, tris, .. } => {
                if *convex || tris.is_empty() {
                    single(sh_clip(ring, pts))
                } else {
                    pieces(tris)
                }
            }
            ClipShape::Inverted { outer, tris, .. } => {
                if tris.is_empty() {
                    single(sh_clip(ring, &rect_ring(outer)))
                } else {
                    pieces(tris)
                }
            }
        }
    }
}

fn liang_barsky(a: V2, b: V2, r: &[f64; 4]) -> Option<(V2, V2)> {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let d = [b[0] - a[0], b[1] - a[1]];
    for (p, q) in [(-d[0], a[0] - r[0]), (d[0], r[2] - a[0]), (-d[1], a[1] - r[1]), (d[1], r[3] - a[1])] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                if t > t1 {
                    return None;
                }
                t0 = t0.max(t);
            } else {
                if t < t0 {
                    return None;
                }
                t1 = t1.min(t);
            }
        }
    }
    Some((lerp2(a, b, t0), lerp2(a, b, t1)))
}

/// Parameter on segment a-b where it crosses segment c-d.
fn seg_intersect_param(a: V2, b: V2, c: V2, d: V2) -> Option<f64> {
    let r = [b[0] - a[0], b[1] - a[1]];
    let s = [d[0] - c[0], d[1] - c[1]];
    let den = r[0] * s[1] - r[1] * s[0];
    if den.abs() < 1e-300 {
        return None;
    }
    let t = ((c[0] - a[0]) * s[1] - (c[1] - a[1]) * s[0]) / den;
    let u = ((c[0] - a[0]) * r[1] - (c[1] - a[1]) * r[0]) / den;
    if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
        Some(t)
    } else {
        None
    }
}

fn line_intersection(p1: V2, p2: V2, a: V2, b: V2) -> Option<V2> {
    let r = [p2[0] - p1[0], p2[1] - p1[1]];
    let s = [b[0] - a[0], b[1] - a[1]];
    let den = r[0] * s[1] - r[1] * s[0];
    if den.abs() < 1e-300 {
        return None;
    }
    let t = ((a[0] - p1[0]) * s[1] - (a[1] - p1[1]) * s[0]) / den;
    Some([p1[0] + r[0] * t, p1[1] + r[1] * t])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_clips_lines() {
        let c = ClipShape::Rect([0.0, 0.0, 10.0, 10.0]);
        let s = c.clip_line([-5.0, 5.0], [15.0, 5.0]);
        assert_eq!(s.len(), 1);
        assert!((s[0].0[0]).abs() < 1e-12 && (s[0].1[0] - 10.0).abs() < 1e-12);
        assert!(c.clip_line([-5.0, -5.0], [-1.0, -1.0]).is_empty());
    }

    #[test]
    fn polygon_clip_splits_lines() {
        // an L shaped region: the line through the notch is split in two
        let c = ClipShape::polygon(vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [6.0, 10.0], [6.0, 4.0], [4.0, 4.0], [4.0, 10.0], [0.0, 10.0]]).unwrap();
        let s = c.clip_line([-1.0, 8.0], [11.0, 8.0]);
        assert_eq!(s.len(), 2, "{:?}", s);
    }

    #[test]
    fn polygon_clip_of_polygon() {
        let c = ClipShape::Rect([0.0, 0.0, 10.0, 10.0]);
        let r = c.clip_polygon(&[[5.0, 5.0], [15.0, 5.0], [15.0, 15.0], [5.0, 15.0]]);
        assert_eq!(r.len(), 1);
        assert!((ring_area(&r[0]).abs() - 25.0).abs() < 1e-9);
    }

    #[test]
    fn polyline_clip_gives_parts() {
        let c = ClipShape::Rect([0.0, 0.0, 10.0, 10.0]);
        let parts = c.clip_polyline(&[[-5.0, 5.0], [5.0, 5.0], [5.0, 15.0], [8.0, 15.0], [8.0, 5.0], [20.0, 5.0]]);
        assert_eq!(parts.len(), 2, "{:?}", parts);
    }

    fn length(s: &[(V2, V2)]) -> f64 {
        s.iter().map(|(a, b)| ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()).sum()
    }

    /// The square 20..60 turned by `deg` degrees about (5, 7) and moved there: a convex polygon that is not a rectangle
    /// parallel to the axes, so the clipping uses the polygon code.
    fn rotated_square(deg: f64) -> (ClipShape, impl Fn(V2) -> V2) {
        let (s, c) = deg.to_radians().sin_cos();
        let xf = move |p: V2| -> V2 { [p[0] * c - p[1] * s + 5.0, p[0] * s + p[1] * c + 7.0] };
        let pts: Vec<V2> = [[20.0, 20.0], [60.0, 20.0], [60.0, 60.0], [20.0, 60.0]].iter().map(|&p| xf(p)).collect();
        (ClipShape::polygon(pts).unwrap(), xf)
    }

    #[test]
    fn a_line_along_an_edge_keeps_only_the_overlap_with_the_edge_at_any_angle() {
        // the grid of the XCLIP drawings: lines 0..100 on y = 20 and y = 60 lie along the edges of the clip square 20..60
        for deg in [0.0, 15.0, 30.0, 33.3, 45.0, 90.0, 123.4, 200.0] {
            let (shape, xf) = rotated_square(deg);
            let mut total = 0.0;
            for k in 0..=10 {
                let y = k as f64 * 10.0;
                total += length(&shape.clip_line(xf([0.0, y]), xf([100.0, y])));
                total += length(&shape.clip_line(xf([y, 0.0]), xf([y, 100.0])));
            }
            // five horizontal and five vertical lines cross the square (y = 20 .. 60), each 40 long inside it; the lines
            // on the edges (y = 20 and y = 60) count only where they overlap the edge: also 40
            assert!((total - 400.0).abs() < 1e-6, "{deg} degrees: {total}");
            for y in [20.0, 60.0] {
                let l = length(&shape.clip_line(xf([0.0, y]), xf([100.0, y])));
                assert!((l - 40.0).abs() < 1e-6, "{deg} degrees, line on the edge y = {y}: {l}");
                let l = length(&shape.clip_line(xf([y, 0.0]), xf([y, 100.0])));
                assert!((l - 40.0).abs() < 1e-6, "{deg} degrees, line on the edge x = {y}: {l}");
            }
        }
    }

    #[test]
    fn a_line_beyond_the_end_of_the_edge_it_lies_on_is_outside() {
        let (shape, xf) = rotated_square(30.0);
        // along the extension of the edge, never touching the square
        assert!(shape.clip_line(xf([61.0, 20.0]), xf([100.0, 20.0])).is_empty());
        assert!(shape.clip_line(xf([0.0, 60.0]), xf([19.0, 60.0])).is_empty());
        // the same segment with a bit of overlap keeps only the overlap
        let l = length(&shape.clip_line(xf([50.0, 20.0]), xf([100.0, 20.0])));
        assert!((l - 10.0).abs() < 1e-6, "{l}");
    }

    #[test]
    fn a_line_along_an_edge_of_an_inverted_clip_is_kept() {
        // everything outside the inner square is kept, so is its boundary (ezdxf: collinear is inside)
        let inv = ClipShape::inverted(vec![[4.0, 4.0], [6.0, 4.0], [6.0, 6.0], [4.0, 6.0]], [0.0, 0.0, 10.0, 10.0]).unwrap();
        let l = length(&inv.clip_line([0.0, 4.0], [10.0, 4.0]));
        assert!((l - 10.0).abs() < 1e-9, "{l}");
        let l = length(&inv.clip_line([0.0, 5.0], [10.0, 5.0]));
        assert!((l - 8.0).abs() < 1e-9, "{l}");
    }

    /// A ring of `n` points of a circle: convex, with an edge index.
    fn circle(n: usize, cx: f64, cy: f64, r: f64) -> Vec<V2> {
        (0..n).map(|i| {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            [cx + r * a.cos(), cy + r * a.sin()]
        }).collect()
    }

    #[test]
    fn the_edge_index_gives_the_same_answers_as_the_search_along_all_edges() {
        let ring = circle(4000, 3.0, -2.0, 50.0);
        let ix = EdgeIndex::build(&ring).expect("index");
        let fast = ClipShape::polygon(ring.clone()).unwrap();
        let mut slow = fast.clone();
        if let ClipShape::Poly { index, .. } = &mut slow {
            *index = None;
        }
        let mut seed = 12345u64;
        let mut rnd = |lo: f64, hi: f64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            lo + (hi - lo) * ((seed >> 11) as f64 / (1u64 << 53) as f64)
        };
        for _ in 0..300 {
            let (a, b) = ([rnd(-70.0, 70.0), rnd(-70.0, 70.0)], [rnd(-70.0, 70.0), rnd(-70.0, 70.0)]);
            let (f, s) = (fast.clip_line(a, b), slow.clip_line(a, b));
            assert_eq!(f.len(), s.len(), "{a:?} {b:?}");
            assert!((length(&f) - length(&s)).abs() < 1e-9);
            let p = [rnd(-70.0, 70.0), rnd(-70.0, 70.0)];
            assert_eq!(fast.contains(p), slow.contains(p), "{p:?}");
            assert_eq!(inside_ring(p, &ring, Some(&ix)), point_in_ring(p, &ring), "{p:?}");
        }
    }

    #[test]
    fn a_huge_clipping_ring_is_searched_in_time_and_far_segments_are_rejected_at_once() {
        // radius 1e9 around (1e9, 0): the origin lies on the circle; 400 000 edges
        let ring = circle(400_000, 1e9, 0.0, 1e9);
        let t0 = std::time::Instant::now();
        let shape = ClipShape::polygon(ring).unwrap();
        let mut inside = 0.0;
        for i in 0..3000 {
            let x = 1.0 + i as f64 * 0.001;
            inside += length(&shape.clip_line([x, 0.0], [x, 1.0]));
        }
        assert!((inside - 3000.0).abs() < 1.0, "{inside}");
        // segments beyond the bounding box never get to the edges
        for i in 0..3000 {
            assert!(shape.clip_line([-5.0 - i as f64, 0.0], [-1.0, 1.0]).is_empty());
        }
        assert!(t0.elapsed() < std::time::Duration::from_secs(5), "{:?}", t0.elapsed());
    }
}
