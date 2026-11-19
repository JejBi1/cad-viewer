//! Measuring on triangle meshes (STL). Port of stepview/meshmeasure.py.
//!
//! The B-rep tools (vertices, edges, faces) do not exist in an STL, which is one triangulation. The mesh is
//! analysed here: sharp edges (dihedral angle above `FEATURE_DEG`) and open boundaries are chained into polylines,
//! and closed or long polylines get a least-squares circle, so the rim of a drilled hole measures as a diameter.
//!
//! Numbers: node coordinates and areas are f64. Facet normals and centroids are f32, as in the Python version
//! (they are only used for searching and grouping). Angle tests compare f32 dot products with cos(angle).
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use cad_types::P3;

use crate::fmt::fmt_num;

/// Dihedral angle in degrees that makes an edge sharp.
pub const FEATURE_DEG: f64 = 30.0;
/// Facets of one planar face deviate less than this (degrees) from the seed normal.
pub const FLAT_DEG: f64 = 0.5;
/// From this many facets, picking searches a facet grid instead of all facets.
pub const GRID_MIN_TRIS: usize = 40_000;
/// Scanned or organic meshes with more feature edges than this are not chained into loops.
pub const MAX_FEATURE_EDGES: usize = 400_000;
/// Bigger meshes are only displayed (measuring data would need more than 0.5 GB).
pub const MAX_TRIS: usize = 3_000_000;
/// Generous size of one ASCII facet in bytes (typically about 250); file size limit for ASCII STL.
pub const ASCII_FACET_BYTES: u64 = 400;
/// Binary STL record: normal (3 x f32), three vertices (9 x f32), attribute (u16).
pub const STL_RECORD_BYTES: usize = 50;
const STL_HEADER_BYTES: usize = 84;
const ASCII_CHUNK: usize = 1 << 24;
const WELD_BITS: u32 = 21;
const FACET_GRID_MAX_CELLS: i64 = 400;
/// Most cells the facet grid may have along one axis. A mesh that needs more (a few small facets far from the rest
/// of a model) is searched with all facets; this also keeps every cell index and key far from `i64` overflow.
const GRID_MAX_DIM: i64 = 1 << 20;

/// Cell coordinate of `v` in a grid with origin `o` and cell size `h`; `None` when it is not finite or above
/// `GRID_MAX_DIM`.
fn grid_cell(v: f64, o: f64, h: f64) -> Option<i64> {
    let c = ((v - o) / h).floor();
    (c.is_finite() && (0.0..=GRID_MAX_DIM as f64).contains(&c)).then_some(c as i64)
}

/// Name of a loop kind (`Loop.kind`): "obrys" (outline), "kružnice" (circle, coverage above 300 degrees) or
/// "oblouk" (arc).
pub const KIND_OUTLINE: &str = "obrys";
pub const KIND_CIRCLE: &str = "kružnice";
pub const KIND_ARC: &str = "oblouk";

// ---------------------------------------------------------------- small vector helpers

fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: P3, b: P3) -> P3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: P3, s: f64) -> P3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: P3, b: P3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: P3, b: P3) -> P3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: P3) -> f64 {
    dot(a, a).sqrt()
}
fn norm2(a: P3) -> f64 {
    dot(a, a)
}
fn dot32(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn to64(a: [f32; 3]) -> P3 {
    [a[0] as f64, a[1] as f64, a[2] as f64]
}

/// Eigen decomposition of a symmetric 3x3 matrix by cyclic Jacobi rotations. Returns the eigenvalues and the
/// eigenvectors as the columns of the second value (`v[row][col]`).
fn sym_eig3(mut a: [[f64; 3]; 3]) -> ([f64; 3], [[f64; 3]; 3]) {
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..50 {
        let off = a[0][1].abs() + a[0][2].abs() + a[1][2].abs();
        if off == 0.0 {
            break;
        }
        for (p, q) in [(0usize, 1usize), (0, 2), (1, 2)] {
            if a[p][q] == 0.0 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let sgn = if theta >= 0.0 { 1.0 } else { -1.0 };
            let t = sgn / (theta.abs() + (theta * theta + 1.0).sqrt());
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            for k in 0..3 {
                let (akp, akq) = (a[k][p], a[k][q]);
                a[k][p] = c * akp - s * akq;
                a[k][q] = s * akp + c * akq;
            }
            for k in 0..3 {
                let (apk, aqk) = (a[p][k], a[q][k]);
                a[p][k] = c * apk - s * aqk;
                a[q][k] = s * apk + c * aqk;
            }
            for row in v.iter_mut() {
                let (vkp, vkq) = (row[p], row[q]);
                row[p] = c * vkp - s * vkq;
                row[q] = s * vkp + c * vkq;
            }
        }
    }
    ([a[0][0], a[1][1], a[2][2]], v)
}

/// Solves a 3x3 linear system by Gaussian elimination with partial pivoting. `None` when singular.
fn solve3(mut m: [[f64; 3]; 3], mut b: [f64; 3]) -> Option<[f64; 3]> {
    let scale_ref = m.iter().flatten().fold(0.0f64, |acc, x| acc.max(x.abs()));
    if scale_ref == 0.0 {
        return None;
    }
    for col in 0..3 {
        let piv = (col..3).max_by(|&i, &j| m[i][col].abs().total_cmp(&m[j][col].abs()))?;
        if m[piv][col].abs() <= 1e-12 * scale_ref {
            return None;
        }
        m.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..3 {
            let f = m[row][col] / m[col][col];
            for k in col..3 {
                m[row][k] -= f * m[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0; 3];
    for row in (0..3).rev() {
        let mut s = b[row];
        for k in row + 1..3 {
            s -= m[row][k] * x[k];
        }
        x[row] = s / m[row][row];
    }
    Some(x)
}

// ---------------------------------------------------------------- loops and circles

/// A least-squares circle through a loop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    pub centre: P3,
    /// Unit normal of the plane of the circle. Its sign is arbitrary.
    pub normal: P3,
    pub radius: f64,
    /// RMS of the radial residuals.
    pub rms: f64,
}

/// A chain of sharp mesh edges (`Loop`). `circle` is set when the points fit a circle.
#[derive(Clone, Debug, PartialEq)]
pub struct Loop {
    pub pts: Vec<P3>,
    pub closed: bool,
    /// Polyline length, including the closing segment for closed loops.
    pub length: f64,
    pub circle: Option<Circle>,
    /// Angular coverage of the circle in degrees (0 without a circle).
    pub coverage: f64,
}

impl Loop {
    /// Builds a loop from its points and fits a circle.
    pub fn new(pts: Vec<P3>, closed: bool) -> Loop {
        let mut length: f64 = pts.windows(2).map(|w| norm(sub(w[1], w[0]))).sum();
        if closed && pts.len() > 1 {
            length += norm(sub(pts[0], pts[pts.len() - 1]));
        }
        let mut lp = Loop {
            pts,
            closed,
            length,
            circle: None,
            coverage: 0.0,
        };
        lp.fit();
        lp
    }

    /// Kind name as in the UI: `KIND_OUTLINE`, `KIND_CIRCLE` (coverage above 300 degrees) or `KIND_ARC`.
    pub fn kind(&self) -> &'static str {
        match self.circle {
            None => KIND_OUTLINE,
            Some(_) if self.coverage > 300.0 => KIND_CIRCLE,
            Some(_) => KIND_ARC,
        }
    }

    /// `_fit`: planar test, least-squares circle, radial RMS test (2 %), coverage of at least 60 degrees.
    fn fit(&mut self) {
        let pts = &self.pts;
        if pts.len() < 6 {
            return;
        }
        let n = pts.len() as f64;
        let mut c0 = [0.0; 3];
        for p in pts {
            c0 = add(c0, *p);
        }
        c0 = scale(c0, 1.0 / n);
        // covariance of the centred points; its eigenvectors are the right singular vectors
        let mut cov = [[0.0f64; 3]; 3];
        for p in pts {
            let q = sub(*p, c0);
            for i in 0..3 {
                for j in 0..3 {
                    cov[i][j] += q[i] * q[j];
                }
            }
        }
        let (lam, vecs) = sym_eig3(cov);
        let mut order = [0usize, 1, 2];
        order.sort_by(|&a, &b| lam[b].total_cmp(&lam[a]));
        let s0 = lam[order[0]].max(0.0).sqrt();
        let s2 = lam[order[2]].max(0.0).sqrt();
        if s0 <= 0.0 || s2 / s0 > 0.02 {
            return; // not planar
        }
        let col = |c: usize| [vecs[0][c], vecs[1][c], vecs[2][c]];
        let u = col(order[0]);
        let v = col(order[1]);
        let nrm = col(order[2]);
        // least squares for x^2 + y^2 = a x + b y + c, with x, y the in-plane coordinates
        let mut ata = [[0.0f64; 3]; 3];
        let mut atz = [0.0f64; 3];
        let mut xy = Vec::with_capacity(pts.len());
        for p in pts {
            let q = sub(*p, c0);
            let (x, y) = (dot(q, u), dot(q, v));
            xy.push((x, y));
            let row = [x, y, 1.0];
            let z = x * x + y * y;
            for i in 0..3 {
                for j in 0..3 {
                    ata[i][j] += row[i] * row[j];
                }
                atz[i] += row[i] * z;
            }
        }
        let Some([a, b, c]) = solve3(ata, atz) else {
            return;
        };
        let (xc, yc) = (a / 2.0, b / 2.0);
        let r2 = c + xc * xc + yc * yc;
        if r2 <= 0.0 {
            return;
        }
        let r = r2.sqrt();
        let mut ss = 0.0;
        for &(x, y) in &xy {
            let d = (x - xc).hypot(y - yc);
            ss += (d - r) * (d - r);
        }
        let rms = (ss / n).sqrt();
        if rms > 0.02 * r {
            return;
        }
        let mut ang: Vec<f64> = xy.iter().map(|&(x, y)| (y - yc).atan2(x - xc)).collect();
        ang.sort_by(|a, b| a.total_cmp(b));
        let mut max_gap = ang[0] + 2.0 * std::f64::consts::PI - ang[ang.len() - 1];
        for w in ang.windows(2) {
            max_gap = max_gap.max(w[1] - w[0]);
        }
        let coverage = 360.0 - max_gap.to_degrees();
        if coverage < 60.0 {
            return;
        }
        self.coverage = coverage;
        let centre = add(c0, add(scale(u, xc), scale(v, yc)));
        self.circle = Some(Circle {
            centre,
            normal: nrm,
            radius: r,
            rms,
        });
    }
}

/// Loop lookup tables: all loop points and all loop segments, each with its loop index.
#[derive(Clone, Debug, Default)]
struct LoopIndex {
    pts: Vec<P3>,
    pt_loop: Vec<u32>,
    seg_a: Vec<P3>,
    seg_b: Vec<P3>,
    seg_loop: Vec<u32>,
}

impl LoopIndex {
    fn new(loops: &[Loop]) -> LoopIndex {
        let mut ix = LoopIndex::default();
        for (i, lp) in loops.iter().enumerate() {
            let n = lp.pts.len();
            ix.pts.extend_from_slice(&lp.pts);
            ix.pt_loop.extend(std::iter::repeat_n(i as u32, n));
            let segs = if lp.closed { n } else { n.saturating_sub(1) };
            for k in 0..segs {
                ix.seg_a.push(lp.pts[k]);
                ix.seg_b.push(lp.pts[(k + 1) % n]);
                ix.seg_loop.push(i as u32);
            }
        }
        ix
    }
}

// ---------------------------------------------------------------- topology and patches

#[derive(Clone, Debug, Default)]
struct Topology {
    /// Facet neighbour across each of its three sides (`-1` = none). Only manifold edges.
    nbr: Vec<[i32; 3]>,
}

/// A flat patch: facets that lie in one plane, connected through shared edges (`flat_patch`).
#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    /// Facet indices, sorted.
    pub facets: Vec<u32>,
    pub area: f64,
    /// True when at least 60 % of the border sides are sharp edges or open boundary, i.e. a real planar face and
    /// not a strip of a tessellated curved surface.
    pub is_face: bool,
    pub centroid: P3,
}

#[derive(Clone, Debug)]
struct Grid {
    h: f64,
    o: P3,
    dims: [i64; 3],
    /// Cell keys, sorted; `ids[k]` is the facet of `keys[k]`.
    keys: Vec<i64>,
    ids: Vec<u32>,
    /// Facets that span more than two cells on one axis, tested by their bounding boxes.
    big: Vec<u32>,
    big_min: Vec<P3>,
    big_max: Vec<P3>,
}

impl Grid {
    /// Facets whose cells (or bounding boxes) lie within `r` of `p`. `None` when more than
    /// `FACET_GRID_MAX_CELLS` cells would have to be read, when the query is not finite, or when any index
    /// computation would overflow (the caller then searches all facets).
    fn query(&self, p: P3, r: f64) -> Option<Vec<u32>> {
        if !r.is_finite() || !p.iter().all(|c| c.is_finite()) {
            return None;
        }
        let mut lo = [0i64; 3];
        let mut hi = [0i64; 3];
        for k in 0..3 {
            // float to integer casts saturate, so even a huge `r` only clamps to the grid below
            lo[k] = (((p[k] - r - self.o[k]) / self.h).floor() as i64).max(0);
            hi[k] = (((p[k] + r - self.o[k]) / self.h).floor() as i64).min(self.dims[k].checked_sub(1)?);
        }
        let mut out: Vec<u32> = Vec::new();
        if (0..3).any(|k| hi[k] < lo[k]) {
            // nothing from the cells
        } else {
            let nx = hi[0].checked_sub(lo[0])?.checked_add(1)?;
            let ny = hi[1].checked_sub(lo[1])?.checked_add(1)?;
            if nx.checked_mul(ny)? > FACET_GRID_MAX_CELLS {
                return None;
            }
            for i in lo[0]..=hi[0] {
                for j in lo[1]..=hi[1] {
                    let k0 = i
                        .checked_mul(self.dims[1])?
                        .checked_add(j)?
                        .checked_mul(self.dims[2])?;
                    let (k_lo, k_hi) = (k0.checked_add(lo[2])?, k0.checked_add(hi[2])?);
                    let a = self.keys.partition_point(|&k| k < k_lo);
                    let b = self.keys.partition_point(|&k| k <= k_hi);
                    out.extend_from_slice(&self.ids[a..b.max(a)]);
                }
            }
        }
        for (idx, (mn, mx)) in self.big.iter().zip(self.big_min.iter().zip(self.big_max.iter())) {
            if (0..3).all(|k| mn[k] <= p[k] + r && mx[k] >= p[k] - r) {
                out.push(*idx);
            }
        }
        out.sort_unstable();
        out.dedup();
        Some(out)
    }
}

#[derive(Clone, Debug, Default)]
enum GridState {
    #[default]
    Unbuilt,
    /// Mesh small enough to search all facets.
    Brute,
    Ready(Box<Grid>),
}

/// Triangle mesh in world coordinates. `nodes` are the measured vertices, `tris` index into them.
///
/// Methods that build caches (topology, loops, grid, patches) take `&mut self`.
#[derive(Clone, Debug)]
pub struct Mesh {
    pub nodes: Vec<P3>,
    pub tris: Vec<[u32; 3]>,
    /// Total area in mm².
    pub area: f64,
    /// Unit facet normals (f32).
    pub normals: Vec<[f32; 3]>,
    /// Facet centroids (f32).
    pub centroids: Vec<[f32; 3]>,
    /// Edges with one facet (set by the topology pass).
    pub boundary_edges: usize,
    /// Edges with more than two facets (set by the topology pass).
    pub nonmanifold_edges: usize,
    topo: Option<Topology>,
    sharp: Option<Vec<[u32; 2]>>,
    loops: Option<Vec<Loop>>,
    loop_index: Option<LoopIndex>,
    grid: GridState,
    region_of: Vec<i32>,
    patches: Vec<Patch>,
    plane_tol: Option<f64>,
}

impl Mesh {
    /// Builds a mesh from node coordinates and facets. Every index must be below `nodes.len()`. Memory for the
    /// facet data is taken the ordinary way (a failure ends the program); loading an STL uses [`Mesh::try_new`].
    pub fn new(nodes: Vec<P3>, tris: Vec<[u32; 3]>) -> Mesh {
        match Mesh::try_new(nodes, tris) {
            Some(m) => m,
            None => std::alloc::handle_alloc_error(std::alloc::Layout::new::<[f32; 3]>()),
        }
    }

    /// [`Mesh::new`] that gives `None` instead of aborting when the memory for the facet data cannot be had.
    pub fn try_new(nodes: Vec<P3>, tris: Vec<[u32; 3]>) -> Option<Mesh> {
        let mut normals = try_vec(tris.len())?;
        let mut centroids = try_vec(tris.len())?;
        let mut total = 0.0;
        for t in &tris {
            let v0 = nodes[t[0] as usize];
            let v1 = nodes[t[1] as usize];
            let v2 = nodes[t[2] as usize];
            let n = cross(sub(v1, v0), sub(v2, v0));
            let ln = norm(n);
            total += ln;
            let unit = scale(n, 1.0 / ln.max(1e-300));
            normals.push([unit[0] as f32, unit[1] as f32, unit[2] as f32]);
            let c = scale(add(add(v0, v1), v2), 1.0 / 3.0);
            centroids.push([c[0] as f32, c[1] as f32, c[2] as f32]);
        }
        Some(Mesh {
            nodes,
            tris,
            area: 0.5 * total,
            normals,
            centroids,
            boundary_edges: 0,
            nonmanifold_edges: 0,
            topo: None,
            sharp: None,
            loops: None,
            loop_index: None,
            grid: GridState::Unbuilt,
            region_of: Vec::new(),
            patches: Vec::new(),
            plane_tol: None,
        })
    }

    // ------------------------------------------------------------ loading

    /// Binary or ASCII STL to a mesh. `Ok(None)` when the file has more than `max_tris` facets (known before reading:
    /// binary from the header count, ASCII from the file size, or while parsing) or no valid facet.
    /// Malformed numbers in an ASCII file give `InvalidData`.
    pub fn from_stl(path: &Path, max_tris: usize) -> io::Result<Option<Mesh>> {
        let size = fs::metadata(path)?.len();
        let mut f = File::open(path)?;
        let mut head = [0u8; STL_HEADER_BYTES];
        let got = read_up_to(&mut f, &mut head)?;
        let binary = got == STL_HEADER_BYTES && {
            let count = u32::from_le_bytes([head[80], head[81], head[82], head[83]]) as u64;
            STL_HEADER_BYTES as u64 + STL_RECORD_BYTES as u64 * count == size
        };
        let corners: Vec<P3> = if binary {
            let count = u32::from_le_bytes([head[80], head[81], head[82], head[83]]) as usize;
            if count > max_tris {
                return Ok(None);
            }
            read_binary_corners(&mut f, count)?
        } else {
            if size > max_tris as u64 * ASCII_FACET_BYTES {
                return Ok(None);
            }
            f.seek(SeekFrom::Start(0))?;
            match read_ascii(&mut f, ASCII_CHUNK, Some(max_tris))? {
                Some(c) => c,
                None => return Ok(None),
            }
        };
        Ok(Mesh::from_corners(&corners))
    }

    /// Facet corners (three consecutive points per facet, as STL stores them) to a mesh with shared nodes.
    /// Facets with a non-finite coordinate are dropped, vertices are welded on a 21-bit grid per axis, and
    /// facets with two equal nodes are dropped. `None` when nothing is left.
    ///
    /// Every large buffer is reserved with `try_reserve`: a huge STL on a computer that is short of memory gives
    /// `None` (the model is shown, but cannot be measured) instead of ending the program in an allocation failure.
    pub fn from_corners(corners: &[P3]) -> Option<Mesh> {
        let mut verts: Vec<P3> = try_vec(corners.len())?;
        for facet in corners.chunks_exact(3) {
            if facet.iter().all(|v| v.iter().all(|c| c.is_finite())) {
                verts.extend_from_slice(facet);
            }
        }
        if verts.is_empty() {
            return None;
        }
        let mut lo = verts[0];
        let mut hi = verts[0];
        for v in &verts {
            for k in 0..3 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
        let span = (0..3).map(|k| hi[k] - lo[k]).fold(f64::MIN, f64::max);
        let max_code = ((1i64 << WELD_BITS) - 2) as f64;
        let factor = max_code / span.max(1e-9);
        let shifts = [42u32, 21, 0];
        let mut keys: Vec<i64> = try_vec(verts.len())?;
        for v in &verts {
            let mut key = 0i64;
            for k in 0..3 {
                let code = ((v[k] - lo[k]) * factor).round_ties_even() as i64;
                key |= code << shifts[k];
            }
            keys.push(key);
        }
        // sorted unique keys; each node takes the first vertex (in input order) with that key
        let mut order: Vec<usize> = try_vec(verts.len())?;
        order.extend(0..verts.len());
        order.sort_by_key(|&i| keys[i]);
        let mut nodes: Vec<P3> = Vec::new();
        let mut inv: Vec<u32> = try_vec(verts.len())?;
        inv.resize(verts.len(), 0);
        let mut prev: Option<i64> = None;
        for &i in &order {
            if prev != Some(keys[i]) {
                if nodes.len() == nodes.capacity() && nodes.try_reserve(nodes.len().max(4096)).is_err() {
                    return None;
                }
                nodes.push(verts[i]);
                prev = Some(keys[i]);
            }
            inv[i] = (nodes.len() - 1) as u32;
        }
        drop((keys, order));
        let mut tris: Vec<[u32; 3]> = try_vec(verts.len() / 3)?;
        for f in 0..verts.len() / 3 {
            let t = [inv[3 * f], inv[3 * f + 1], inv[3 * f + 2]];
            if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
                tris.push(t);
            }
        }
        if tris.is_empty() {
            return None;
        }
        Mesh::try_new(nodes, tris)
    }

    // ------------------------------------------------------------ picking

    /// Facet nearest to `p` (a picked point on the surface). With a grid the candidates come from the cells around
    /// `p`, widened when empty; the 64 candidates with the nearest centroids are tested by exact distance.
    pub fn nearest_triangle(&mut self, p: P3) -> usize {
        let mut cand: Option<Vec<u32>> = None;
        if self.prepare() {
            let h = match &self.grid {
                GridState::Ready(g) => g.h,
                _ => 0.0,
            };
            for r in [0.25 * h, h, 4.0 * h] {
                cand = self.grid_query(p, r);
                match &cand {
                    None => break,
                    Some(c) if !c.is_empty() => break,
                    _ => {}
                }
            }
        }
        let mut cand = match cand {
            Some(c) if !c.is_empty() => c,
            _ => (0..self.tris.len() as u32).collect(),
        };
        if cand.is_empty() {
            return 0;
        }
        if cand.len() > 64 {
            let mut keyed: Vec<(f64, u32)> = cand
                .iter()
                .map(|&t| (norm2(sub(to64(self.centroids[t as usize]), p)), t))
                .collect();
            keyed.select_nth_unstable_by(63, |a, b| a.0.total_cmp(&b.0));
            keyed.truncate(64);
            cand = keyed.into_iter().map(|(_, t)| t).collect();
        }
        let mut best = cand[0] as usize;
        let mut bd = f64::INFINITY;
        for &t in &cand {
            let tri = self.tris[t as usize];
            let q = closest_on_triangle(
                p,
                self.nodes[tri[0] as usize],
                self.nodes[tri[1] as usize],
                self.nodes[tri[2] as usize],
            );
            let dd = norm2(sub(q, p));
            if dd < bd {
                best = t as usize;
                bd = dd;
            }
        }
        best
    }

    /// Mesh vertex nearest to `p` and its distance. With `r` only vertices of facets near `p` are searched (big
    /// meshes). `None` when there is no vertex in range.
    pub fn nearest_vertex(&mut self, p: P3, r: Option<f64>) -> Option<(P3, f64)> {
        let mut idx: Option<Vec<u32>> = None;
        if let Some(r) = r {
            if self.prepare() {
                if let Some(cand) = self.grid_query(p, r) {
                    if cand.is_empty() {
                        return None;
                    }
                    let mut ids: Vec<u32> = cand
                        .iter()
                        .flat_map(|&t| self.tris[t as usize])
                        .collect();
                    ids.sort_unstable();
                    ids.dedup();
                    idx = Some(ids);
                }
            }
        }
        let it: Box<dyn Iterator<Item = u32>> = match idx {
            Some(v) => Box::new(v.into_iter()),
            None => Box::new(0..self.nodes.len() as u32),
        };
        let mut best: Option<(u32, f64)> = None;
        for i in it {
            let d = norm2(sub(self.nodes[i as usize], p));
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        best.map(|(i, d)| (self.nodes[i as usize], d.sqrt()))
    }

    /// Builds the facet grid for big meshes. `true` when the grid is ready.
    fn prepare(&mut self) -> bool {
        if matches!(self.grid, GridState::Unbuilt) {
            self.grid = if self.tris.len() >= GRID_MIN_TRIS {
                // a mesh the grid cannot index (non-finite coordinates, extent far beyond the facet size) is
                // searched facet by facet
                match self.build_grid() {
                    Some(g) => GridState::Ready(Box::new(g)),
                    None => GridState::Brute,
                }
            } else {
                GridState::Brute
            };
        }
        matches!(self.grid, GridState::Ready(_))
    }

    fn grid_query(&self, p: P3, r: f64) -> Option<Vec<u32>> {
        match &self.grid {
            GridState::Ready(g) => g.query(p, r),
            _ => None,
        }
    }

    /// The facet grid, or `None` when it cannot be built safely: a non-finite coordinate or cell size, an extent of
    /// more than `GRID_MAX_DIM` cells on an axis (a few small facets far from the rest), or a cell count whose key
    /// does not fit an `i64`. The mesh is then searched with all facets.
    fn build_grid(&self) -> Option<Grid> {
        let m = self.tris.len();
        if m == 0 {
            return None;
        }
        let mut bmin: Vec<P3> = Vec::with_capacity(m);
        let mut bmax: Vec<P3> = Vec::with_capacity(m);
        let mut extent: Vec<f64> = Vec::with_capacity(m);
        for t in &self.tris {
            let v = [
                self.nodes[t[0] as usize],
                self.nodes[t[1] as usize],
                self.nodes[t[2] as usize],
            ];
            // f64::min / max skip a NaN, so every corner is checked itself
            if !v.iter().all(|w| w.iter().all(|c| c.is_finite())) {
                return None;
            }
            let mut mn = v[0];
            let mut mx = v[0];
            for w in &v[1..] {
                for k in 0..3 {
                    mn[k] = mn[k].min(w[k]);
                    mx[k] = mx[k].max(w[k]);
                }
            }
            extent.push((0..3).map(|k| mx[k] - mn[k]).fold(f64::MIN, f64::max));
            bmin.push(mn);
            bmax.push(mx);
        }
        let h = (2.0 * median(&mut extent)).max(1e-9); // cell about two typical facets
        if !h.is_finite() {
            return None;
        }
        let mut o = [f64::INFINITY; 3];
        for b in &bmin {
            for k in 0..3 {
                o[k] = o[k].min(b[k]);
            }
        }
        let mut lo = vec![[0i64; 3]; m];
        let mut span = vec![[0i64; 3]; m];
        for i in 0..m {
            for k in 0..3 {
                // every cell coordinate is at most GRID_MAX_DIM, so the sums below cannot overflow
                let a = grid_cell(bmin[i][k], o[k], h)?;
                let b = grid_cell(bmax[i][k], o[k], h)?;
                lo[i][k] = a;
                span[i][k] = b - a + 1;
            }
        }
        let mut dims = [0i64; 3];
        for k in 0..3 {
            let max_lo = lo.iter().map(|l| l[k]).max().unwrap_or(0);
            let max_span = span.iter().map(|s| s[k]).max().unwrap_or(0);
            dims[k] = max_lo.checked_add(max_span)?.checked_add(1)?;
        }
        // the cell key is (x * dims[1] + y) * dims[2] + z < dims[0] * dims[1] * dims[2]: it must fit an i64
        dims[0].checked_mul(dims[1]).and_then(|v| v.checked_mul(dims[2]))?;
        let small: Vec<bool> = span.iter().map(|s| s.iter().all(|&x| x <= 2)).collect();
        let mut entries: Vec<(i64, u32)> = Vec::new();
        for dx in 0..2i64 {
            for dy in 0..2i64 {
                for dz in 0..2i64 {
                    for i in 0..m {
                        if !small[i] || !(dx < span[i][0] && dy < span[i][1] && dz < span[i][2]) {
                            continue;
                        }
                        let c = [lo[i][0] + dx, lo[i][1] + dy, lo[i][2] + dz];
                        let key = c[0]
                            .checked_mul(dims[1])
                            .and_then(|v| v.checked_add(c[1]))
                            .and_then(|v| v.checked_mul(dims[2]))
                            .and_then(|v| v.checked_add(c[2]))?;
                        entries.push((key, i as u32));
                    }
                }
            }
        }
        entries.sort_by_key(|e| e.0);
        let big: Vec<u32> = (0..m).filter(|&i| !small[i]).map(|i| i as u32).collect();
        Some(Grid {
            h,
            o,
            dims,
            keys: entries.iter().map(|e| e.0).collect(),
            ids: entries.iter().map(|e| e.1).collect(),
            big_min: big.iter().map(|&i| bmin[i as usize]).collect(),
            big_max: big.iter().map(|&i| bmax[i as usize]).collect(),
            big,
        })
    }

    // ------------------------------------------------------------ topology

    /// Facet neighbours, boundary and non-manifold edge counts, and the sharp edges (node pairs, sorted).
    fn build_topology(&mut self) {
        if self.topo.is_some() {
            return;
        }
        let m = self.tris.len();
        let n = self.nodes.len() as u64;
        let mut ent: Vec<(u64, usize)> = Vec::with_capacity(3 * m);
        for side in 0..3 {
            for f in 0..m {
                let t = self.tris[f];
                let a = t[side];
                let b = t[(side + 1) % 3];
                let (lo, hi) = (a.min(b) as u64, a.max(b) as u64);
                ent.push((lo * n + hi, side * m + f));
            }
        }
        ent.sort_by_key(|e| e.0); // stable: equal keys keep position order
        let cos_t = (FEATURE_DEG.to_radians().cos()) as f32;
        let mut nbr = vec![[-1i32; 3]; m];
        let mut sharp: Vec<[u32; 2]> = Vec::new();
        self.boundary_edges = 0;
        self.nonmanifold_edges = 0;
        let mut i = 0;
        while i < ent.len() {
            let key = ent[i].0;
            let mut j = i;
            while j < ent.len() && ent[j].0 == key {
                j += 1;
            }
            let cnt = j - i;
            let lo = (key / n) as u32;
            let hi = (key % n) as u32;
            if cnt == 1 {
                self.boundary_edges += 1;
                sharp.push([lo, hi]);
            } else if cnt == 2 {
                let p1 = ent[i].1;
                let p2 = ent[i + 1].1;
                let (f1, f2) = (p1 % m, p2 % m);
                if dot32(self.normals[f1], self.normals[f2]) < cos_t {
                    sharp.push([lo, hi]);
                }
                nbr[f1][p1 / m] = f2 as i32;
                nbr[f2][p2 / m] = f1 as i32;
            } else {
                self.nonmanifold_edges += 1;
                sharp.push([lo, hi]);
            }
            i = j;
        }
        self.topo = Some(Topology { nbr });
        self.sharp = Some(sharp);
    }

    /// Sharp-edge loops: chained polylines of sharp edges, each with its circle fit. Cached.
    pub fn loops(&mut self) -> &[Loop] {
        if self.loops.is_none() {
            let lps = self.build_loops();
            self.loop_index = Some(LoopIndex::new(&lps));
            self.loops = Some(lps);
        }
        self.loops.as_deref().unwrap_or(&[])
    }

    fn build_loops(&mut self) -> Vec<Loop> {
        self.build_topology();
        let fe = self.sharp.take().unwrap_or_default();
        if fe.is_empty() || fe.len() > MAX_FEATURE_EDGES {
            return Vec::new();
        }
        let mut adj: HashMap<u32, Vec<u32>> = HashMap::new();
        for &[a, b] in &fe {
            adj.entry(a).or_default().push(b);
            adj.entry(b).or_default().push(a);
        }
        let mut used: HashSet<(u32, u32)> = HashSet::new();
        let mut loops = Vec::new();
        for &[a, b] in &fe {
            if used.contains(&(a, b)) || used.contains(&(b, a)) {
                continue;
            }
            used.insert((a, b));
            let mut chain = vec![a, b];
            let mut closed = false;
            for direction in 0..2 {
                if direction == 1 {
                    chain.reverse();
                }
                loop {
                    let cur = chain[chain.len() - 1];
                    let prev = chain[chain.len() - 2];
                    let nb = adj.get(&cur).map(Vec::as_slice).unwrap_or(&[]);
                    if nb.len() != 2 {
                        break; // junction or end
                    }
                    let nxt = if nb[1] == prev { nb[0] } else { nb[1] };
                    if used.contains(&(cur, nxt)) || used.contains(&(nxt, cur)) {
                        if nxt == chain[0] {
                            closed = true;
                        }
                        break;
                    }
                    used.insert((cur, nxt));
                    chain.push(nxt);
                    if nxt == chain[0] {
                        closed = true;
                        break;
                    }
                }
                if closed {
                    break;
                }
            }
            if closed && chain.len() > 1 && chain[chain.len() - 1] == chain[0] {
                chain.pop();
            }
            if chain.len() >= 2 {
                let pts: Vec<P3> = chain.iter().map(|&i| self.nodes[i as usize]).collect();
                loops.push(Loop::new(pts, closed));
            }
        }
        loops
    }

    /// Loop whose polyline passes closest to `p` (for example near a picked feature).
    pub fn loop_near(&mut self, p: P3) -> Option<&Loop> {
        self.loops();
        let li = {
            let ix = self.loop_index.as_ref()?;
            if ix.pts.is_empty() {
                return None;
            }
            let mut best = 0usize;
            let mut bd = f64::INFINITY;
            for (i, q) in ix.pts.iter().enumerate() {
                let d = norm2(sub(*q, p));
                if d < bd {
                    best = i;
                    bd = d;
                }
            }
            ix.pt_loop[best] as usize
        };
        self.loops.as_deref().and_then(|l| l.get(li))
    }

    /// Loop with a segment within `tol` of `p` (the cursor is on a sharp edge), else `None`.
    pub fn loop_at(&mut self, p: P3, tol: f64) -> Option<&Loop> {
        self.loops();
        if tol <= 0.0 {
            return None;
        }
        let li = {
            let ix = self.loop_index.as_ref()?;
            if ix.seg_a.is_empty() {
                return None;
            }
            let mut best = 0usize;
            let mut bd = f64::INFINITY;
            for i in 0..ix.seg_a.len() {
                let (a, b) = (ix.seg_a[i], ix.seg_b[i]);
                let ab = sub(b, a);
                let l2 = dot(ab, ab);
                let t = (dot(sub(p, a), ab) / l2.max(1e-300)).clamp(0.0, 1.0);
                let q = add(a, scale(ab, t));
                let d = norm2(sub(q, p));
                if d < bd {
                    best = i;
                    bd = d;
                }
            }
            if bd > tol * tol {
                return None;
            }
            ix.seg_loop[best] as usize
        };
        self.loops.as_deref().and_then(|l| l.get(li))
    }

    // ------------------------------------------------------------ flat patches

    /// Plane offset tolerance of flat patches: `max(1e-3, 1e-4 * ptp(nodes))`, where ptp is over all coordinates.
    fn plane_tolerance(&mut self) -> f64 {
        if let Some(t) = self.plane_tol {
            return t;
        }
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for v in &self.nodes {
            for &c in v {
                lo = lo.min(c);
                hi = hi.max(c);
            }
        }
        let t = 1e-3f64.max(1e-4 * (hi - lo));
        self.plane_tol = Some(t);
        t
    }

    /// Flat patch that contains facet `t`, as an index for `patch`. Computed once per patch.
    pub fn flat_patch(&mut self, t: usize) -> usize {
        if self.region_of.len() != self.tris.len() {
            self.region_of = vec![-1; self.tris.len()];
        }
        let rid = self.region_of[t];
        if rid >= 0 {
            return rid as usize;
        }
        self.build_topology();
        let dtol = self.plane_tolerance();
        let patch = {
            let nbr: &[[i32; 3]] = match &self.topo {
                Some(tp) => &tp.nbr,
                None => &[],
            };
            compute_patch(
                t,
                nbr,
                &self.normals,
                &self.centroids,
                &self.nodes,
                &self.tris,
                dtol,
            )
        };
        let id = self.patches.len();
        for &f in &patch.facets {
            self.region_of[f as usize] = id as i32;
        }
        self.patches.push(patch);
        id
    }

    /// Patch with the given index from `flat_patch`.
    pub fn patch(&self, id: usize) -> Option<&Patch> {
        self.patches.get(id)
    }

    // ------------------------------------------------------------ info

    /// Rows of the STL info panel: label and value. Labels and values are the UI strings of meshmeasure.py.
    pub fn info_rows(&mut self) -> Vec<(String, String)> {
        let n_loops = self.loops().len();
        let radii: Vec<f64> = self
            .loops()
            .iter()
            .filter(|l| l.kind() == KIND_CIRCLE)
            .filter_map(|l| l.circle.map(|c| c.radius))
            .collect();
        let n_holes = radii.len();
        let (b, nm) = (self.boundary_edges, self.nonmanifold_edges);
        let mut rows = vec![
            (
                "Síť".to_string(),
                format!(
                    "{} trojúhelníků, {} vrcholů",
                    fmt_num(self.tris.len() as f64, 0),
                    fmt_num(self.nodes.len() as f64, 0)
                ),
            ),
            (
                "Uzavřená síť".to_string(),
                if b == 0 && nm == 0 {
                    "ano".to_string()
                } else {
                    format!(
                        "ne ({b} otevřených hran{})",
                        if nm > 0 {
                            format!(", {nm} nemanifoldních")
                        } else {
                            String::new()
                        }
                    )
                },
            ),
            (
                "Ostré hrany".to_string(),
                format!("{n_loops} obrysů, {n_holes} kružnic (otvory / čepy)"),
            ),
        ];
        if !radii.is_empty() {
            let mut ds: Vec<f64> = radii
                .iter()
                .map(|r| round_places(2.0 * r, 2))
                .collect();
            ds.sort_by(|a, b| a.total_cmp(b));
            ds.dedup();
            let mut text = ds
                .iter()
                .take(12)
                .map(|d| fmt_num(*d, 2))
                .collect::<Vec<_>>()
                .join(", ");
            if ds.len() > 12 {
                text.push_str(" …");
            }
            rows.push(("Průměry kružnic".to_string(), text));
        }
        rows
    }
}

/// `round(x, places)` of Python: the nearest double to the correctly rounded decimal (ties to even on the exact
/// binary value, which is what Rust's `{:.*}` formatting does).
fn round_places(x: f64, places: usize) -> f64 {
    format!("{:.*}", places, x).parse().unwrap_or(x)
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

fn compute_patch(
    t: usize,
    nbr: &[[i32; 3]],
    normals: &[[f32; 3]],
    centroids: &[[f32; 3]],
    nodes: &[P3],
    tris: &[[u32; 3]],
    dtol: f64,
) -> Patch {
    let m = tris.len();
    let n0 = normals[t];
    let d0 = dot32(n0, centroids[t]);
    let cmin = (FLAT_DEG.to_radians().cos()) as f32;
    let dtol32 = dtol as f32;
    let mut inside = vec![false; m];
    inside[t] = true;
    let mut facets: Vec<u32> = vec![t as u32];
    let mut queue: VecDeque<usize> = VecDeque::from([t]);
    while let Some(f) = queue.pop_front() {
        for s in 0..3 {
            let g = nbr[f][s];
            if g < 0 || inside[g as usize] {
                continue;
            }
            let g = g as usize;
            if dot32(normals[g], n0) >= cmin && (dot32(centroids[g], n0) - d0).abs() <= dtol32 {
                inside[g] = true;
                facets.push(g as u32);
                queue.push_back(g);
            }
        }
    }
    facets.sort_unstable();
    let mut area = 0.0;
    let mut weighted = [0.0f64; 3];
    let mut facet_area = Vec::with_capacity(facets.len());
    for &f in &facets {
        let tri = tris[f as usize];
        let v0 = nodes[tri[0] as usize];
        let v1 = nodes[tri[1] as usize];
        let v2 = nodes[tri[2] as usize];
        let a = 0.5 * norm(cross(sub(v1, v0), sub(v2, v0)));
        area += a;
        facet_area.push(a);
        let c = to64(centroids[f as usize]);
        for k in 0..3 {
            weighted[k] += c[k] * a;
        }
    }
    let centroid = scale(weighted, 1.0 / area.max(1e-300));
    let cos_t = (FEATURE_DEG.to_radians().cos()) as f32;
    let mut nb = 0usize;
    let mut border_sharp = 0usize;
    for &f in &facets {
        for s in 0..3 {
            let g = nbr[f as usize][s];
            let is_border = g < 0 || !inside[g as usize];
            if !is_border {
                continue;
            }
            nb += 1;
            let sharp = g < 0 || dot32(normals[f as usize], normals[g as usize]) < cos_t;
            if sharp {
                border_sharp += 1;
            }
        }
    }
    let is_face = facets.len() >= 2 && nb > 0 && (border_sharp as f64) / (nb as f64) >= 0.6;
    Patch {
        facets,
        area,
        is_face,
        centroid,
    }
}

// ---------------------------------------------------------------- geometry

/// Closest point to `p` on triangle `abc` (Ericson, Real-Time Collision Detection 5.1.5).
pub fn closest_on_triangle(p: P3, a: P3, b: P3, c: P3) -> P3 {
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return add(a, scale(ab, d1 / (d1 - d3)));
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return add(a, scale(ac, d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return add(b, scale(sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let den = 1.0 / (va + vb + vc);
    add(add(a, scale(ab, vb * den)), scale(ac, vc * den))
}

// ---------------------------------------------------------------- STL reading

/// Reads until `buf` is full or the file ends. Returns the number of bytes read.
fn read_up_to<R: Read>(f: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match f.read(&mut buf[got..])? {
            0 => break,
            n => got += n,
        }
    }
    Ok(got)
}

/// An empty vector with room for `n` items, or `None` when that much memory cannot be had (or `n` is absurd).
fn try_vec<T>(n: usize) -> Option<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).ok()?;
    Some(v)
}

fn out_of_memory() -> io::Error {
    io::Error::new(io::ErrorKind::OutOfMemory, "not enough memory for the STL")
}

fn read_binary_corners<R: Read>(f: &mut R, count: usize) -> io::Result<Vec<P3>> {
    let bytes = count.checked_mul(STL_RECORD_BYTES).ok_or_else(out_of_memory)?;
    let corners = count.checked_mul(3).ok_or_else(out_of_memory)?;
    let mut buf: Vec<u8> = try_vec(bytes).ok_or_else(out_of_memory)?;
    buf.resize(bytes, 0);
    f.read_exact(&mut buf)?;
    let mut out: Vec<P3> = try_vec(corners).ok_or_else(out_of_memory)?;
    let float = |b: &[u8], o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as f64;
    for rec in buf.chunks_exact(STL_RECORD_BYTES) {
        for v in 0..3 {
            let o = 12 + 12 * v;
            out.push([float(rec, o), float(rec, o + 4), float(rec, o + 8)]);
        }
    }
    Ok(out)
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// The vertex lines of an ASCII STL, like `re.findall(rb"vertex\s+(\S+)\s+(\S+)\s+(\S+)")`.
fn vertex_matches(buf: &[u8]) -> Vec<[&[u8]; 3]> {
    const KW: &[u8] = b"vertex";
    let mut out = Vec::new();
    let mut i = 0;
    while i + KW.len() <= buf.len() {
        if &buf[i..i + KW.len()] != KW {
            i += 1;
            continue;
        }
        match match_vertex_at(buf, i + KW.len()) {
            Some((toks, end)) => {
                out.push(toks);
                i = end;
            }
            None => i += 1,
        }
    }
    out
}

fn match_vertex_at(buf: &[u8], mut j: usize) -> Option<([&[u8]; 3], usize)> {
    let mut toks: [&[u8]; 3] = [&[], &[], &[]];
    for tok in toks.iter_mut() {
        let ws = j;
        while j < buf.len() && is_ws(buf[j]) {
            j += 1;
        }
        if j == ws {
            return None;
        }
        let st = j;
        while j < buf.len() && !is_ws(buf[j]) {
            j += 1;
        }
        if j == st {
            return None;
        }
        *tok = &buf[st..j];
    }
    Some((toks, j))
}

fn parse_coord(tok: &[u8]) -> io::Result<f64> {
    std::str::from_utf8(tok)
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad number in STL"))
}

/// Vertex coordinates of an ASCII STL, parsed in pieces of `chunk` bytes. With `max_tris` the vertex count is
/// checked while parsing: `None` as soon as it exceeds `3 * max_tris`. Vertices are truncated to a multiple of 3.
fn read_ascii<R: Read>(f: &mut R, chunk: usize, max_tris: Option<usize>) -> io::Result<Option<Vec<P3>>> {
    let mut parts: Vec<P3> = Vec::new();
    let mut tail: Vec<u8> = Vec::new();
    let mut nverts = 0usize;
    let mut data = vec![0u8; chunk];
    loop {
        let got = read_up_to(f, &mut data)?;
        let mut buf = std::mem::take(&mut tail);
        buf.extend_from_slice(&data[..got]);
        if got > 0 {
            let cut = buf
                .iter()
                .rposition(|&b| b == b'\n' || b == b'\r')
                .map_or(0, |i| i + 1);
            if cut == 0 {
                if buf.len() > 4 * chunk {
                    return Ok(Some(Vec::new())); // no line breaks at all: not an STL
                }
                tail = buf;
                continue;
            }
            tail = buf.split_off(cut);
        }
        let matches = vertex_matches(&buf);
        if !matches.is_empty() {
            nverts += matches.len();
            if let Some(m) = max_tris {
                if nverts > 3 * m {
                    return Ok(None);
                }
            }
            parts.try_reserve(matches.len()).map_err(|_| out_of_memory())?;
            for toks in &matches {
                parts.push([
                    parse_coord(toks[0])?,
                    parse_coord(toks[1])?,
                    parse_coord(toks[2])?,
                ]);
            }
        }
        if got == 0 {
            break;
        }
    }
    let keep = parts.len() / 3 * 3;
    parts.truncate(keep);
    Ok(Some(parts))
}

/// Mesh of an STL file for measuring: `None` when unreadable, invalid or too big. Not cached.
pub fn mesh_for_file(path: &Path, max_tris: usize) -> Option<Mesh> {
    Mesh::from_stl(path, max_tris).ok().flatten()
}

#[cfg(test)]
mod tests;
