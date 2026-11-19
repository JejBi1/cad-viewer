//! Resolution of the id-buffer window read back around the cursor (GPU picking). The GPU renders faces, edges and
//! vertices into three small `Rg32Uint` targets (instance id + 1, entity code); this module decides what was hit.
use cad_types::EntityKind;

/// Top two bits of the entity code.
pub const CODE_FACE: u32 = 0;
pub const CODE_EDGE: u32 = 1;
pub const CODE_VERTEX: u32 = 2;
pub const INDEX_MASK: u32 = 0x3fff_ffff;

/// What may be picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PickFilter {
    pub faces: bool,
    pub edges: bool,
    pub vertices: bool,
}

impl PickFilter {
    /// Plain selection of parts by clicking on their surface.
    pub const FACES: PickFilter = PickFilter { faces: true, edges: false, vertices: false };
    /// Wireframe display: only edges can be hit.
    pub const EDGES: PickFilter = PickFilter { faces: false, edges: true, vertices: false };
    /// Measuring: vertices before edges before faces.
    pub const ALL: PickFilter = PickFilter { faces: true, edges: true, vertices: true };
}

/// Square window of the id buffers centred on the cursor, `size = 2 * radius + 1`, row-major, row 0 = upper row.
#[derive(Clone, Debug, Default)]
pub struct PickWindow {
    pub size: u32,
    pub faces: Vec<[u32; 2]>,
    pub edges: Vec<[u32; 2]>,
    pub vertices: Vec<[u32; 2]>,
    pub depth: Vec<f32>,
}

/// Winning pixel of a pick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawHit {
    /// Index into the instance list (id - 1).
    pub instance: u32,
    pub kind: EntityKind,
    pub index: u32,
    /// Offset of the pixel from the cursor.
    pub dx: i32,
    pub dy: i32,
    /// NDC depth of the pixel (0 near, 1 far).
    pub depth: f32,
}

impl RawHit {
    pub fn distance(&self) -> f32 {
        ((self.dx * self.dx + self.dy * self.dy) as f32).sqrt()
    }
}

fn decode(cell: [u32; 2], expect: u32) -> Option<(u32, u32)> {
    if cell[0] == 0 || cell[1] >> 30 != expect {
        return None;
    }
    Some((cell[0] - 1, cell[1] & INDEX_MASK))
}

fn nearest(
    w: &PickWindow,
    cells: &[[u32; 2]],
    expect: u32,
    kind: EntityKind,
    radius: u32,
) -> Option<RawHit> {
    let size = w.size as i32;
    if size <= 0 || cells.len() < (size * size) as usize {
        return None;
    }
    let r = (size - 1) / 2;
    let limit = (radius as i32).min(r);
    let mut best: Option<(i32, f32, RawHit)> = None;
    for j in 0..size {
        for i in 0..size {
            let (dx, dy) = (i - r, j - r);
            let d2 = dx * dx + dy * dy;
            if d2 > limit * limit {
                continue;
            }
            let cell = cells[(j * size + i) as usize];
            let Some((instance, index)) = decode(cell, expect) else { continue };
            let depth = w.depth.get((j * size + i) as usize).copied().unwrap_or(1.0);
            let better = match &best {
                None => true,
                Some((bd2, bdepth, _)) => d2 < *bd2 || (d2 == *bd2 && depth < *bdepth),
            };
            if better {
                best = Some((d2, depth, RawHit { instance, kind, index, dx, dy, depth }));
            }
        }
    }
    best.map(|b| b.2)
}

/// Vertices win over edges, edges over faces; inside a class the pixel closest to the cursor (then the closest to
/// the camera) wins. Only pixels within `radius` of the cursor count.
pub fn resolve(w: &PickWindow, filter: PickFilter, radius: u32) -> Option<RawHit> {
    if filter.vertices {
        if let Some(h) = nearest(w, &w.vertices, CODE_VERTEX, EntityKind::Vertex, radius) {
            return Some(h);
        }
    }
    if filter.edges {
        if let Some(h) = nearest(w, &w.edges, CODE_EDGE, EntityKind::Edge, radius) {
            return Some(h);
        }
    }
    if filter.faces {
        return nearest(w, &w.faces, CODE_FACE, EntityKind::Face, radius);
    }
    None
}

/// Window size for a pick radius (odd).
pub fn window_size(radius: u32) -> u32 {
    2 * radius + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty(radius: u32) -> PickWindow {
        let n = (window_size(radius) * window_size(radius)) as usize;
        PickWindow { size: window_size(radius), faces: vec![[0, 0]; n], edges: vec![[0, 0]; n], vertices: vec![[0, 0]; n], depth: vec![1.0; n] }
    }

    fn put(cells: &mut [[u32; 2]], size: u32, dx: i32, dy: i32, id: u32, kind: u32, index: u32) {
        let r = (size as i32 - 1) / 2;
        cells[((dy + r) * size as i32 + dx + r) as usize] = [id, (kind << 30) | index];
    }

    #[test]
    fn nothing_hit_in_an_empty_window() {
        assert_eq!(resolve(&empty(3), PickFilter::ALL, 3), None);
        assert_eq!(resolve(&PickWindow::default(), PickFilter::ALL, 3), None);
    }

    #[test]
    fn face_under_the_cursor() {
        let mut w = empty(3);
        let s = w.size;
        put(&mut w.faces, s, 0, 0, 5, CODE_FACE, 7);
        let h = resolve(&w, PickFilter::FACES, 3).expect("hit");
        assert_eq!((h.instance, h.kind, h.index, h.dx, h.dy), (4, EntityKind::Face, 7, 0, 0));
    }

    #[test]
    fn nearest_pixel_wins_then_nearest_depth() {
        let mut w = empty(7);
        let s = w.size;
        put(&mut w.faces, s, 3, 0, 1, CODE_FACE, 10);
        put(&mut w.faces, s, -2, 0, 2, CODE_FACE, 20);
        let h = resolve(&w, PickFilter::FACES, 7).expect("hit");
        assert_eq!(h.index, 20);
        // equal distance: the pixel with the smaller depth
        let mut w = empty(7);
        let s = w.size;
        put(&mut w.faces, s, 2, 0, 1, CODE_FACE, 10);
        put(&mut w.faces, s, -2, 0, 2, CODE_FACE, 20);
        w.depth[(7 * s as usize + 5) as usize] = 0.2; // pixel at dx = -2, dy = 0 is index 7*15 + 5
        let h = resolve(&w, PickFilter::FACES, 7).expect("hit");
        assert_eq!(h.index, 20);
    }

    #[test]
    fn radius_limits_the_search() {
        let mut w = empty(7);
        let s = w.size;
        put(&mut w.faces, s, 5, 0, 1, CODE_FACE, 10);
        assert!(resolve(&w, PickFilter::FACES, 3).is_none());
        assert!(resolve(&w, PickFilter::FACES, 5).is_some());
        // the corner of the square window is farther than the radius
        let mut w = empty(7);
        let s = w.size;
        put(&mut w.faces, s, 5, 5, 1, CODE_FACE, 10);
        assert!(resolve(&w, PickFilter::FACES, 7).is_none());
        // the window itself limits the search
        assert!(resolve(&w, PickFilter::FACES, 8).is_none());
    }

    #[test]
    fn vertices_beat_edges_beat_faces() {
        let mut w = empty(7);
        let s = w.size;
        put(&mut w.faces, s, 0, 0, 1, CODE_FACE, 1);
        put(&mut w.edges, s, 3, 0, 1, CODE_EDGE, 2);
        put(&mut w.vertices, s, 5, 0, 1, CODE_VERTEX, 3);
        assert_eq!(resolve(&w, PickFilter::ALL, 7).expect("hit").kind, EntityKind::Vertex);
        let no_vertex = PickFilter { vertices: false, ..PickFilter::ALL };
        assert_eq!(resolve(&w, no_vertex, 7).expect("hit").kind, EntityKind::Edge);
        assert_eq!(resolve(&w, PickFilter::FACES, 7).expect("hit").kind, EntityKind::Face);
        // the snap tolerance applies to vertices: out of reach -> falls back to the edge
        assert_eq!(resolve(&w, PickFilter::ALL, 4).expect("hit").kind, EntityKind::Edge);
        assert_eq!(resolve(&w, PickFilter::ALL, 2).expect("hit").kind, EntityKind::Face);
        assert_eq!(resolve(&w, PickFilter::EDGES, 7).expect("hit").index, 2);
    }

    #[test]
    fn wrong_kind_codes_are_ignored() {
        let mut w = empty(3);
        let s = w.size;
        // an edge code in the face target must not count
        put(&mut w.faces, s, 0, 0, 1, CODE_EDGE, 1);
        assert!(resolve(&w, PickFilter::FACES, 3).is_none());
        // id 0 is the background even with a code
        put(&mut w.faces, s, 1, 0, 0, CODE_FACE, 5);
        assert!(resolve(&w, PickFilter::FACES, 3).is_none());
    }

    #[test]
    fn short_buffers_do_not_panic() {
        let mut w = empty(3);
        w.faces.truncate(3);
        assert!(resolve(&w, PickFilter::FACES, 3).is_none());
        let mut w = empty(3);
        w.depth.clear();
        let s = w.size;
        put(&mut w.faces, s, 0, 0, 1, CODE_FACE, 1);
        assert_eq!(resolve(&w, PickFilter::FACES, 3).expect("hit").depth, 1.0);
    }

    #[test]
    fn distance_of_a_hit() {
        let h = RawHit { instance: 0, kind: EntityKind::Edge, index: 0, dx: 3, dy: 4, depth: 0.5 };
        assert!((h.distance() - 5.0).abs() < 1e-6);
        assert_eq!(window_size(7), 15);
    }
}
