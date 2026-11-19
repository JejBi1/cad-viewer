//! Polygon triangulation for filled primitives (even-odd rule, holes by nesting depth).
use crate::geom::{point_in_ring, ring_area, V2};

/// Removes consecutive duplicates and a repeated closing point.
pub fn clean_ring(ring: &[V2]) -> Vec<V2> {
    let mut out: Vec<V2> = Vec::with_capacity(ring.len());
    for &p in ring {
        if !p[0].is_finite() || !p[1].is_finite() {
            continue;
        }
        if let Some(l) = out.last() {
            if l[0] == p[0] && l[1] == p[1] {
                continue;
            }
        }
        out.push(p);
    }
    while out.len() > 1 && out[0][0] == out[out.len() - 1][0] && out[0][1] == out[out.len() - 1][1] {
        out.pop();
    }
    out
}

fn earcut(rings: &[&[V2]]) -> Vec<V2> {
    // rings[0] is the outer ring, the others are holes
    let mut flat: Vec<f64> = Vec::new();
    let mut holes: Vec<usize> = Vec::new();
    for (i, r) in rings.iter().enumerate() {
        if i > 0 {
            holes.push(flat.len() / 2);
        }
        for p in r.iter() {
            flat.push(p[0]);
            flat.push(p[1]);
        }
    }
    let idx = std::panic::catch_unwind(|| earcutr::earcut(&flat, &holes, 2)).ok().and_then(|r| r.ok()).unwrap_or_default();
    let mut out = Vec::with_capacity(idx.len());
    for i in idx {
        if let (Some(&x), Some(&y)) = (flat.get(2 * i), flat.get(2 * i + 1)) {
            out.push([x, y]);
        }
    }
    out
}

/// Triangles of a polygon with holes: `rings[0]` is the outline, the others are holes inside of it.
pub fn triangulate_with_holes(rings: &[&[V2]]) -> Vec<V2> {
    earcut(rings)
}

/// Triangles (point triples) of one simple polygon.
pub fn triangulate_polygon(ring: &[V2]) -> Vec<V2> {
    let r = clean_ring(ring);
    if r.len() < 3 {
        return Vec::new();
    }
    earcut(&[&r])
}

fn bbox(r: &[V2]) -> [f64; 4] {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in r {
        b[0] = b[0].min(p[0]);
        b[1] = b[1].min(p[1]);
        b[2] = b[2].max(p[0]);
        b[3] = b[3].max(p[1]);
    }
    b
}

fn bbox_contains(a: &[f64; 4], b: &[f64; 4]) -> bool {
    a[0] <= b[0] && a[1] <= b[1] && a[2] >= b[2] && a[3] >= b[3]
}

/// Triangles of a set of rings filled with the even-odd rule: a ring nested in an even number of other rings is an
/// outline, in an odd number a hole of the smallest ring around it.
pub fn triangulate_even_odd(rings: &[Vec<V2>]) -> Vec<V2> {
    let rings: Vec<Vec<V2>> = rings.iter().map(|r| clean_ring(r)).filter(|r| r.len() >= 3).collect();
    match rings.len() {
        0 => return Vec::new(),
        1 => return earcut(&[&rings[0]]),
        _ => {}
    }
    let boxes: Vec<[f64; 4]> = rings.iter().map(|r| bbox(r)).collect();
    let areas: Vec<f64> = rings.iter().map(|r| ring_area(r).abs()).collect();
    let n = rings.len();
    // a point of ring i that is not on the boundary of ring j: the first vertex is fine for the nesting tests of
    // non-touching rings; touching rings fall back on the centroid of the first triangle
    let probe: Vec<V2> = rings.iter().map(|r| r[0]).collect();
    let mut parent: Vec<Option<usize>> = vec![None; n];
    let mut depth = vec![0u32; n];
    // The rings that can contain ring i are the ones whose bounding box contains its probe point. A grid over the
    // bounding boxes finds them without looking at every pair (the glyph outlines of a long text are tens of thousands
    // of small rings next to each other); rings that cover many cells are tested against every ring.
    let grid = NestGrid::new(&boxes);
    for i in 0..n {
        let mut best: Option<usize> = None;
        grid.candidates(probe[i], |j| {
            if i == j || areas[j] <= areas[i] || !bbox_contains(&boxes[j], &boxes[i]) {
                return;
            }
            if point_in_ring(probe[i], &rings[j]) {
                depth[i] += 1;
                if best.map(|b| areas[j] < areas[b] || (areas[j] == areas[b] && j < b)).unwrap_or(true) {
                    best = Some(j);
                }
            }
        });
        parent[i] = best;
    }
    let mut holes_of: Vec<Vec<usize>> = vec![Vec::new(); n];
    for j in 0..n {
        if depth[j] % 2 == 1 {
            if let Some(p) = parent[j] {
                holes_of[p].push(j);
            }
        }
    }
    let mut out = Vec::new();
    for i in 0..n {
        if depth[i] % 2 == 1 {
            continue; // hole, handled with its outline
        }
        let mut set: Vec<&[V2]> = vec![&rings[i]];
        set.extend(holes_of[i].iter().map(|&j| rings[j].as_slice()));
        out.extend(earcut(&set));
    }
    out
}

/// Uniform grid over the bounding boxes of rings: which rings may contain a point.
struct NestGrid {
    x0: f64,
    y0: f64,
    inv_cell: f64,
    gx: usize,
    gy: usize,
    cells: Vec<Vec<u32>>,
    /// rings that cover too many cells to be listed in them
    big: Vec<u32>,
}

impl NestGrid {
    /// Most cells a ring is listed in.
    const MAX_CELLS_PER_RING: usize = 64;

    fn new(boxes: &[[f64; 4]]) -> NestGrid {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        let (mut sum_w, mut sum_h) = (0.0, 0.0);
        for b in boxes {
            x0 = x0.min(b[0]);
            y0 = y0.min(b[1]);
            x1 = x1.max(b[2]);
            y1 = y1.max(b[3]);
            sum_w += b[2] - b[0];
            sum_h += b[3] - b[1];
        }
        let n = boxes.len().max(1);
        let (w, h) = (x1 - x0, y1 - y0);
        // square cells about as large as a ring (a text line is a long, thin row of cells), at most a few per ring
        let mut cell = (sum_w / n as f64).max(sum_h / n as f64);
        if !(cell > 0.0) || !cell.is_finite() {
            cell = w.max(h);
        }
        let max_cells = (4 * n).max(1024) as f64;
        let cells_for = |c: f64| ((w / c).ceil().max(1.0)) * ((h / c).ceil().max(1.0));
        while cell > 0.0 && cell.is_finite() && cells_for(cell) > max_cells {
            cell *= 2.0;
        }
        let (gx, gy, inv_cell) = if cell > 0.0 && cell.is_finite() && w.is_finite() && h.is_finite() {
            ((w / cell).ceil().max(1.0) as usize, (h / cell).ceil().max(1.0) as usize, 1.0 / cell)
        } else {
            (1, 1, 0.0)
        };
        let mut grid = NestGrid { x0, y0, inv_cell, gx, gy, cells: vec![Vec::new(); gx * gy], big: Vec::new() };
        for (j, b) in boxes.iter().enumerate() {
            let (cx0, cy0) = grid.cell([b[0], b[1]]);
            let (cx1, cy1) = grid.cell([b[2], b[3]]);
            if (cx1 - cx0 + 1) * (cy1 - cy0 + 1) > Self::MAX_CELLS_PER_RING {
                grid.big.push(j as u32);
                continue;
            }
            for cy in cy0..=cy1 {
                for cx in cx0..=cx1 {
                    grid.cells[cy * gx + cx].push(j as u32);
                }
            }
        }
        grid
    }

    /// Cell of a point; monotone in both coordinates, so a box covers the cells between the cells of its corners.
    fn cell(&self, p: V2) -> (usize, usize) {
        let f = |v: f64, lo: f64, g: usize| (((v - lo) * self.inv_cell).max(0.0) as usize).min(g - 1);
        (f(p[0], self.x0, self.gx), f(p[1], self.y0, self.gy))
    }

    /// Calls `f` for every ring that may contain the point (in increasing ring order within the cell and the big rings).
    fn candidates(&self, p: V2, mut f: impl FnMut(usize)) {
        let (cx, cy) = self.cell(p);
        for &j in &self.cells[cy * self.gx + cx] {
            f(j as usize);
        }
        for &j in &self.big {
            f(j as usize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(tris: &[V2]) -> f64 {
        tris.chunks(3).map(|t| ring_area(t).abs()).sum()
    }

    #[test]
    fn square_with_hole() {
        let outer = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let hole = vec![[2.0, 2.0], [4.0, 2.0], [4.0, 4.0], [2.0, 4.0]];
        let t = triangulate_even_odd(&[outer, hole]);
        assert!((area(&t) - 96.0).abs() < 1e-9, "{}", area(&t));
    }

    #[test]
    fn island_inside_hole_is_filled_again() {
        let a = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let b = vec![[1.0, 1.0], [9.0, 1.0], [9.0, 9.0], [1.0, 9.0]];
        let c = vec![[4.0, 4.0], [6.0, 4.0], [6.0, 6.0], [4.0, 6.0]];
        let t = triangulate_even_odd(&[a, b, c]);
        assert!((area(&t) - (100.0 - 64.0 + 4.0)).abs() < 1e-9);
    }

    #[test]
    fn degenerate_input_gives_nothing() {
        assert!(triangulate_polygon(&[[0.0, 0.0], [1.0, 1.0]]).is_empty());
        assert!(triangulate_even_odd(&[]).is_empty());
        let t = triangulate_polygon(&[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0]]);
        assert!((area(&t) - 0.5).abs() < 1e-12);
    }
}
