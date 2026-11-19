//! B-splines for SPLINE entities and hatch spline edges.
use crate::geom::*;

#[derive(Clone, Debug)]
pub struct BSpline {
    pub degree: usize,
    pub knots: Vec<f64>,
    pub ctrl: Vec<V3>,
    pub weights: Vec<f64>,
}

fn open_uniform_knots(count: usize, order: usize) -> Vec<f64> {
    let mut k = vec![0.0; order];
    let inner = count.saturating_sub(order);
    for i in 1..=inner {
        k.push(i as f64);
    }
    let last = (inner + 1) as f64;
    k.extend(std::iter::repeat_n(last, order));
    k
}

/// Highest degree of a B-spline that is evaluated. Evaluating a point costs the square of the degree and a curve has
/// several points per control point, so a degree of 400 in a 140 KB file kept the viewer busy for many minutes.
/// AutoCAD's own limit is lower (SPLINE 10, NURBS surface curves 25).
pub const MAX_DEGREE: usize = 32;

impl BSpline {
    /// `knots` empty or of the wrong length: open uniform knot vector. `None` for a degree of 0 or above `MAX_DEGREE`
    /// and for fewer than `degree + 1` control points.
    pub fn new(ctrl: Vec<V3>, degree: usize, knots: Vec<f64>, weights: Vec<f64>) -> Option<BSpline> {
        let count = ctrl.len();
        if degree == 0 || degree > MAX_DEGREE || count < degree + 1 {
            return None;
        }
        let order = degree + 1;
        let knots = if knots.len() == count + order { knots } else { open_uniform_knots(count, order) };
        let weights = if weights.len() == count { weights } else { Vec::new() };
        Some(BSpline { degree, knots, ctrl, weights })
    }

    pub fn is_rational(&self) -> bool {
        !self.weights.is_empty() && self.weights.iter().any(|&w| (w - self.weights[0]).abs() > 1e-12 || (w - 1.0).abs() > 1e-12)
    }

    pub fn is_clamped(&self) -> bool {
        let p = self.degree;
        let k = &self.knots;
        let n = k.len();
        if n < p + 1 {
            return false;
        }
        k[..=p].windows(2).all(|w| isclose(w[0], w[1], 1e-9, 1e-12)) && k[n - p - 1..].windows(2).all(|w| isclose(w[0], w[1], 1e-9, 1e-12))
    }

    /// Parameter domain (knots[p], knots[count]).
    pub fn domain(&self) -> (f64, f64) {
        (self.knots[self.degree], self.knots[self.ctrl.len()])
    }

    fn find_span(&self, u: f64) -> usize {
        let n = self.ctrl.len();
        let p = self.degree;
        if u >= self.knots[n] {
            return n - 1;
        }
        let mut lo = p;
        let mut hi = n;
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if u < self.knots[mid] {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        lo
    }

    fn basis(&self, span: usize, u: f64) -> Vec<f64> {
        let p = self.degree;
        let k = &self.knots;
        let mut nb = vec![0.0; p + 1];
        let mut left = vec![0.0; p + 1];
        let mut right = vec![0.0; p + 1];
        nb[0] = 1.0;
        for j in 1..=p {
            left[j] = u - k[span + 1 - j];
            right[j] = k[span + j] - u;
            let mut saved = 0.0;
            for r in 0..j {
                let d = right[r + 1] + left[j - r];
                let temp = if d != 0.0 { nb[r] / d } else { 0.0 };
                nb[r] = saved + right[r + 1] * temp;
                saved = left[j - r] * temp;
            }
            nb[j] = saved;
        }
        nb
    }

    pub fn point(&self, u: f64) -> V3 {
        let (u0, u1) = self.domain();
        let u = u.clamp(u0.min(u1), u1.max(u0));
        let span = self.find_span(u);
        let nb = self.basis(span, u);
        let p = self.degree;
        let mut acc = [0.0; 3];
        let mut wsum = 0.0;
        for (j, b) in nb.iter().enumerate() {
            let i = span - p + j;
            let w = if self.weights.is_empty() { 1.0 } else { self.weights[i] };
            let bw = b * w;
            wsum += bw;
            for a in 0..3 {
                acc[a] += self.ctrl[i][a] * bw;
            }
        }
        if wsum != 0.0 && !self.weights.is_empty() {
            mul(acc, 1.0 / wsum)
        } else {
            acc
        }
    }

    /// Cubic Bezier segments of a non-rational clamped B-spline of any degree (NURBS book A5.6); `degree + 1` points each.
    /// `None` when the algorithm does not apply, and the caller samples the curve instead: a rational or unclamped
    /// spline, inconsistent lengths, or an interior knot of a multiplicity above the degree (the curve is broken there,
    /// which the book's algorithm does not handle).
    pub fn bezier_decomposition(&self) -> Option<Vec<Vec<V3>>> {
        if self.is_rational() || !self.is_clamped() {
            return None;
        }
        if self.ctrl.len() < self.degree + 1 || self.knots.len() != self.ctrl.len() + self.degree + 1 {
            return None;
        }
        let n = self.ctrl.len() - 1;
        let p = self.degree;
        let u = &self.knots;
        let m = n + p + 1;
        let mut a = p;
        let mut b = p + 1;
        let mut bez: Vec<V3> = self.ctrl[0..=p].to_vec();
        let mut alphas = vec![0.0; p + 1];
        let mut out = Vec::new();
        while b < m {
            let mut next: Vec<V3> = vec![NULL3; p + 1];
            let i = b;
            while b < m && isclose(u[b + 1], u[b], 1e-9, 1e-12) {
                b += 1;
            }
            let mult = b - i + 1;
            if mult > p && b < m {
                // an interior knot repeated more than `degree` times (`p - mult` below would underflow); the end knots
                // (`b == m`) are repeated `degree + 1` times in a clamped spline and are fine
                return None;
            }
            if mult < p {
                let numer = u[b] - u[a];
                for j in ((mult + 1)..=p).rev() {
                    let d = u[a + j] - u[a];
                    alphas[j - mult - 1] = if d != 0.0 { numer / d } else { 0.0 };
                }
                let r = p - mult;
                for j in 1..=r {
                    let save = r - j;
                    let s = mult + j;
                    for k in (s..=p).rev() {
                        let alpha = alphas[k - s];
                        bez[k] = add(mul(bez[k], alpha), mul(bez[k - 1], 1.0 - alpha));
                    }
                    if b < m {
                        next[save] = bez[p];
                    }
                }
            }
            out.push(bez.clone());
            if b < m {
                // `mult <= p` here (checked above)
                for (idx, slot) in next.iter_mut().enumerate().take(p + 1).skip(p.saturating_sub(mult)) {
                    let ci = b - p + idx;
                    if ci < self.ctrl.len() {
                        *slot = self.ctrl[ci];
                    }
                }
                a = b;
                b += 1;
                bez = next;
            }
        }
        Some(out)
    }

    /// Points on the curve, `per_span` samples in every non-empty knot span.
    pub fn sample(&self, per_span: usize) -> Vec<V3> {
        let (u0, u1) = self.domain();
        if !(u1 > u0) {
            return vec![self.point(u0)];
        }
        let mut breaks: Vec<f64> = vec![u0];
        for &k in &self.knots {
            if k > u0 + 1e-12 && k < u1 - 1e-12 && (k - breaks[breaks.len() - 1]).abs() > 1e-12 {
                breaks.push(k);
            }
        }
        breaks.push(u1);
        let mut out = vec![self.point(u0)];
        for w in breaks.windows(2) {
            for i in 1..=per_span {
                let t = w[0] + (w[1] - w[0]) * i as f64 / per_span as f64;
                out.push(self.point(t));
            }
        }
        out
    }
}

/// Cubic Bezier segments of the natural cubic spline through the fit points (see `fit_points_curve`).
pub fn fit_points_beziers(points: &[V3], tangents: Option<(V3, V3)>) -> Vec<[V3; 4]> {
    let (t, m) = match fit_spline_moments(points, tangents) {
        Some(x) => x,
        None => return Vec::new(),
    };
    let n = points.len();
    let mut out = Vec::with_capacity(n - 1);
    for i in 0..n - 1 {
        let h = t[i + 1] - t[i];
        let mut d0 = [0.0; 3];
        let mut d1 = [0.0; 3];
        for a in 0..3 {
            let slope = (points[i + 1][a] - points[i][a]) / h;
            d0[a] = slope - h * (2.0 * m[i][a] + m[i + 1][a]) / 6.0;
            d1[a] = slope + h * (m[i][a] + 2.0 * m[i + 1][a]) / 6.0;
        }
        out.push([points[i], add(points[i], mul(d0, h / 3.0)), sub(points[i + 1], mul(d1, h / 3.0)), points[i + 1]]);
    }
    out
}

/// Natural cubic spline (chord length parametrisation) through fit points: samples of the curve.
/// With `tangents` the first derivatives at the ends are prescribed (direction scaled by the chord / parameter ratio).
pub fn fit_points_curve(points: &[V3], tangents: Option<(V3, V3)>, per_segment: usize) -> Vec<V3> {
    let n = points.len();
    if n < 2 {
        return points.to_vec();
    }
    if n == 2 {
        return points.to_vec();
    }
    let Some((t, m)) = fit_spline_moments(points, tangents) else { return vec![points[0]] };
    let mut out = vec![points[0]];
    for i in 0..n - 1 {
        let h = t[i + 1] - t[i];
        for s in 1..=per_segment {
            let x = s as f64 / per_segment as f64 * h;
            let a_ = (h - x) / h;
            let b_ = x / h;
            let mut p = [0.0; 3];
            for axis in 0..3 {
                let yi = points[i][axis];
                let yj = points[i + 1][axis];
                p[axis] = a_ * yi + b_ * yj + ((a_.powi(3) - a_) * m[i][axis] + (b_.powi(3) - b_) * m[i + 1][axis]) * h * h / 6.0;
            }
            out.push(p);
        }
    }
    out
}

/// Chord length parameters and second derivatives (moments) of the natural cubic spline through the points.
fn fit_spline_moments(points: &[V3], tangents: Option<(V3, V3)>) -> Option<(Vec<f64>, Vec<[f64; 3]>)> {
    let n = points.len();
    if n < 3 {
        return None;
    }
    let mut t = vec![0.0; n];
    for i in 1..n {
        t[i] = t[i - 1] + dist(points[i], points[i - 1]);
    }
    if t[n - 1] <= 0.0 || t.windows(2).any(|w| w[1] <= w[0]) {
        return None;
    }
    let mut m = vec![[0.0f64; 3]; n];
    for axis in 0..3 {
        let y: Vec<f64> = points.iter().map(|p| p[axis]).collect();
        let mut a = vec![0.0; n];
        let mut b = vec![0.0; n];
        let mut c = vec![0.0; n];
        let mut d = vec![0.0; n];
        for i in 1..n - 1 {
            let h0 = t[i] - t[i - 1];
            let h1 = t[i + 1] - t[i];
            a[i] = h0;
            b[i] = 2.0 * (h0 + h1);
            c[i] = h1;
            d[i] = 6.0 * ((y[i + 1] - y[i]) / h1 - (y[i] - y[i - 1]) / h0);
        }
        match tangents {
            None => {
                b[0] = 1.0;
                b[n - 1] = 1.0;
            }
            Some((ts, te)) => {
                let h0 = t[1] - t[0];
                let hn = t[n - 1] - t[n - 2];
                // end derivative: tangent direction times the chord speed of the first / last segment
                let d0 = normalize(ts)[axis] * (dist(points[1], points[0]) / h0);
                let dn = normalize(te)[axis] * (dist(points[n - 1], points[n - 2]) / hn);
                b[0] = 2.0 * h0;
                c[0] = h0;
                d[0] = 6.0 * ((y[1] - y[0]) / h0 - d0);
                a[n - 1] = hn;
                b[n - 1] = 2.0 * hn;
                d[n - 1] = 6.0 * (dn - (y[n - 1] - y[n - 2]) / hn);
            }
        }
        // Thomas algorithm
        for i in 1..n {
            let w = a[i] / b[i - 1];
            b[i] -= w * c[i - 1];
            d[i] -= w * d[i - 1];
        }
        let mut sol = vec![0.0; n];
        sol[n - 1] = d[n - 1] / b[n - 1];
        for i in (0..n - 1).rev() {
            sol[i] = (d[i] - c[i] * sol[i + 1]) / b[i];
        }
        for i in 0..n {
            m[i][axis] = sol[i];
        }
    }
    Some((t, m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cubic_decomposition_endpoints() {
        let ctrl = vec![[0.0, 0.0, 0.0], [1.0, 2.0, 0.0], [3.0, 2.0, 0.0], [4.0, 0.0, 0.0]];
        let bs = BSpline::new(ctrl, 3, vec![], vec![]).unwrap();
        assert!(bs.is_clamped());
        let segs = bs.bezier_decomposition().unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].len(), 4);
        assert_eq!(segs[0][0], [0.0, 0.0, 0.0]);
        assert_eq!(segs[0][3], [4.0, 0.0, 0.0]);
    }

    #[test]
    fn decomposition_matches_evaluation() {
        let ctrl = vec![[0.0, 0.0, 0.0], [1.0, 3.0, 0.0], [2.0, -1.0, 0.0], [3.0, 2.0, 0.0], [5.0, 0.0, 0.0], [6.0, 4.0, 0.0]];
        let bs = BSpline::new(ctrl, 3, vec![], vec![]).unwrap();
        let segs = bs.bezier_decomposition().unwrap();
        assert_eq!(segs.len(), 3);
        // segment joints lie on the curve
        let (u0, _) = bs.domain();
        assert!(dist(segs[0][0], bs.point(u0)) < 1e-9);
        assert!(dist(segs[0][3], bs.point(u0 + 1.0)) < 1e-9);
        assert!(dist(segs[1][3], bs.point(u0 + 2.0)) < 1e-9);
    }

    /// Degree 3, 8 control points, the interior knot 0.5 repeated 4 times (= degree + 1): two Bezier curves that do not
    /// touch. Every coordinate is far from the origin, so a point at (0, 0, 0) is a wrong one.
    fn broken_spline_data() -> (Vec<V3>, Vec<f64>) {
        let ctrl: Vec<V3> = (0..8).map(|i| [10.0 + i as f64, 20.0 + (i % 3) as f64, 0.0]).collect();
        let knots = vec![0.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.5, 0.5, 1.0, 1.0, 1.0, 1.0];
        (ctrl, knots)
    }

    #[test]
    fn an_interior_knot_repeated_more_than_the_degree_is_not_decomposed() {
        let (ctrl, knots) = broken_spline_data();
        let bs = BSpline::new(ctrl, 3, knots, vec![]).unwrap();
        assert!(bs.is_clamped());
        // used to underflow `p - mult` (a panic in debug, a segment at the origin in release)
        assert!(bs.bezier_decomposition().is_none());
        // the sampling that the callers fall back to stays on the control polygon's box
        for p in bs.sample(16) {
            assert!((10.0 - 1e-9..=17.0 + 1e-9).contains(&p[0]) && (20.0 - 1e-9..=22.0 + 1e-9).contains(&p[1]), "{p:?}");
        }
    }

    #[test]
    fn a_broken_spline_entity_keeps_its_bounds() {
        let (control_points, knots) = broken_spline_data();
        let s = crate::dxf::model::SplineData { degree: 3, knots, control_points, ..Default::default() };
        let path = crate::frontend::spline_path(&s).expect("a path");
        assert!(!path.cmds.is_empty());
        let mut pts = vec![path.start];
        for c in &path.cmds {
            match *c {
                crate::geom::Cmd::Line(p) | crate::geom::Cmd::Move(p) => pts.push(p),
                crate::geom::Cmd::Curve3 { end, ctrl } => pts.extend([end, ctrl]),
                crate::geom::Cmd::Curve4 { end, c1, c2 } => pts.extend([end, c1, c2]),
            }
        }
        for p in pts {
            assert!((10.0 - 1e-9..=17.0 + 1e-9).contains(&p[0]) && (20.0 - 1e-9..=22.0 + 1e-9).contains(&p[1]), "{p:?}");
        }
    }

    #[test]
    fn an_interior_knot_repeated_degree_times_still_decomposes() {
        // multiplicity 3 = degree: two Bezier curves that share ctrl[3]
        let ctrl: Vec<V3> = (0..7).map(|i| [10.0 + i as f64, 20.0 + (i % 3) as f64, 0.0]).collect();
        let knots = vec![0.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.5, 1.0, 1.0, 1.0, 1.0];
        let bs = BSpline::new(ctrl.clone(), 3, knots, vec![]).unwrap();
        let segs = bs.bezier_decomposition().expect("decomposed");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0], ctrl[0..4].to_vec());
        assert_eq!(segs[1], ctrl[3..7].to_vec());
    }

    #[test]
    fn inconsistent_lengths_are_not_decomposed() {
        let ctrl: Vec<V3> = (0..4).map(|i| [i as f64, 0.0, 0.0]).collect();
        let bs = BSpline { degree: 3, knots: vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0], ctrl, weights: vec![] };
        assert!(bs.bezier_decomposition().is_none());
        let short = BSpline { degree: 3, knots: vec![0.0, 1.0], ctrl: vec![[0.0; 3]; 4], weights: vec![] };
        assert!(!short.is_clamped());
    }

    #[test]
    fn rational_quadratic_is_a_circle_arc() {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let ctrl = vec![[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
        let bs = BSpline::new(ctrl, 2, vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0], vec![1.0, s, 1.0]).unwrap();
        assert!(bs.is_rational());
        for p in bs.sample(8) {
            assert!(((p[0] * p[0] + p[1] * p[1]).sqrt() - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn fit_curve_interpolates() {
        let pts = vec![[0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [2.0, 0.0, 0.0], [3.0, 1.0, 0.0]];
        let c = fit_points_curve(&pts, None, 8);
        assert_eq!(c.len(), 1 + 3 * 8);
        for (i, p) in pts.iter().enumerate() {
            assert!(dist(c[i * 8], *p) < 1e-9);
        }
    }
}
