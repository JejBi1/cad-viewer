//! z-order preserving level assignment (port of `native/rdxf` `assign_levels` and `fastscene.grid_cell`).
//!
//! Primitives are processed in drawing order. A primitive goes to the lowest level of its own group that lies above
//! every level already used by a different group in the grid cells its inflated bounding box touches. Primitives of
//! one group look the same, so their relative order does not matter; primitives of different groups that may
//! overlap keep their drawing order.

/// Largest grid (cells) `assign_levels` accepts: three i32 arrays of this size are allocated (rdxf `MAX_CELLS`).
pub const MAX_CELLS: usize = 4_000_000;
/// Grid cell visits of assign_levels above which cells get coarser (fastscene `VISIT_BUDGET_NATIVE`).
pub const VISIT_BUDGET: f64 = 4e8;
/// The selection overlay and the measure items stay on top of the scene (fastscene `MAX_LEVELS`).
pub const MAX_LEVELS: i32 = 400_000;

fn bounds(bb: &[[f64; 4]], pad: f64) -> (f64, f64, f64, f64) {
    let mut x0 = f64::INFINITY;
    let mut y0 = f64::INFINITY;
    let mut x1 = f64::NEG_INFINITY;
    let mut y1 = f64::NEG_INFINITY;
    for b in bb {
        x0 = x0.min(b[0]);
        y0 = y0.min(b[1]);
        x1 = x1.max(b[2]);
        y1 = y1.max(b[3]);
    }
    let x0 = x0 - pad;
    let y0 = y0 - pad;
    let w = (x1 + pad - x0).max(1e-9);
    let h = (y1 + pad - y0).max(1e-9);
    (x0, y0, w, h)
}

fn cell_ranges(b: &[f64; 4], pad: f64, cell: f64, x0: f64, y0: f64, nx: usize, ny: usize) -> (usize, usize, usize, usize) {
    let clampi = |v: f64, n: usize| -> usize {
        if v < 0.0 {
            0
        } else {
            (v as usize).min(n - 1)
        }
    };
    (
        clampi((b[0] - pad - x0) / cell, nx),
        clampi((b[1] - pad - y0) / cell, ny),
        clampi((b[2] + pad - x0) / cell, nx),
        clampi((b[3] + pad - y0) / cell, ny),
    )
}

/// Grid cell size: 1.5 x the median primitive extent (at least 1/512 of the drawing), made coarser until the cells all
/// bounding boxes touch are within `budget` (a few huge primitives must not cost P x grid) and the grid is not
/// larger than `MAX_CELLS`. Coarser cells only mean more false overlaps, i.e. less merging, never a wrong order.
pub fn grid_cell(bb: &[[f64; 4]], pad: f64, budget: f64) -> f64 {
    let (x0, y0, w, h) = bounds(bb, pad);
    let mut ext: Vec<f64> = bb.iter().map(|b| (b[2] - b[0]).max(b[3] - b[1])).collect();
    ext.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = if ext.is_empty() {
        0.0
    } else if ext.len() % 2 == 1 {
        ext[ext.len() / 2]
    } else {
        0.5 * (ext[ext.len() / 2 - 1] + ext[ext.len() / 2])
    };
    let mut cell = (median * 1.5).max(w.max(h) / 512.0);
    for _ in 0..64 {
        let nx = (w / cell) as usize + 1;
        let ny = (h / cell) as usize + 1;
        let grid_ok = (nx as f64) * (ny as f64) <= MAX_CELLS as f64;
        let mut visits = 0.0;
        if grid_ok {
            for b in bb {
                let (ix0, iy0, ix1, iy1) = cell_ranges(b, pad, cell, x0, y0, nx, ny);
                visits += ((ix1 - ix0 + 1) * (iy1 - iy0 + 1)) as f64;
            }
        }
        if grid_ok && visits <= budget {
            break;
        }
        cell *= 1.6;
    }
    cell
}

/// `bb` (P, 4) x0 y0 x1 y1, `gid` (P) >= 0. Returns the level of every primitive.
pub fn assign_levels(bb: &[[f64; 4]], gid: &[i32], pad: f64, cell: f64) -> Result<Vec<i32>, String> {
    let p = gid.len();
    if bb.len() != p {
        return Err("bb must have one row per group id".into());
    }
    if p == 0 {
        return Ok(Vec::new());
    }
    if !(pad.is_finite() && pad >= 0.0 && cell.is_finite() && cell > 0.0) {
        return Err("pad / cell must be finite and positive".into());
    }
    if gid.iter().any(|&g| g < 0) {
        return Err("group ids must be >= 0".into());
    }
    if bb.iter().any(|b| !b.iter().all(|v| v.is_finite())) {
        return Err("bounding boxes must be finite".into());
    }
    let (x0, y0, w, h) = bounds(bb, pad);
    let (nxf, nyf) = (w / cell + 1.0, h / cell + 1.0);
    if !(nxf * nyf <= MAX_CELLS as f64) {
        return Err("grid too large: cell size too small for the drawing extent".into());
    }
    let (nx, ny) = (nxf as usize, nyf as usize);
    let mut best_l = vec![-1i32; nx * ny]; // highest level in the cell
    let mut best_g = vec![-1i32; nx * ny]; // ... and its group
    let mut second_l = vec![-1i32; nx * ny]; // highest level of any group other than best_g
    let gmax = gid.iter().copied().max().unwrap_or(0) as usize + 1;
    let mut glevels: Vec<Vec<i32>> = vec![Vec::new(); gmax]; // per group: its levels, ascending
    let mut level = vec![0i32; p];
    let mut nlev: i32 = 0;
    for i in 0..p {
        let g = gid[i];
        let (ix0, iy0, ix1, iy1) = cell_ranges(&bb[i], pad, cell, x0, y0, nx, ny);
        let mut m = -1i32;
        for cy in iy0..=iy1 {
            let row = cy * nx;
            for cx in ix0..=ix1 {
                let k = row + cx;
                let v = if best_g[k] != g { best_l[k] } else { second_l[k] };
                if v > m {
                    m = v;
                }
            }
        }
        let lv = &mut glevels[g as usize];
        let pos = lv.partition_point(|&c| c <= m);
        let l = if pos < lv.len() {
            lv[pos]
        } else {
            let l = nlev;
            nlev += 1;
            lv.push(l);
            l
        };
        level[i] = l;
        for cy in iy0..=iy1 {
            let row = cy * nx;
            for cx in ix0..=ix1 {
                let k = row + cx;
                let (bl, bg, sc) = (best_l[k], best_g[k], second_l[k]);
                if l > bl {
                    if bg != g {
                        second_l[k] = bl;
                    }
                    best_l[k] = l;
                    best_g[k] = g;
                } else if bg != g && l > sc {
                    second_l[k] = l;
                }
            }
        }
    }
    Ok(level)
}

/// Morton (Z-order curve) index of two 16 bit coordinates.
pub fn morton(x: u32, y: u32) -> u64 {
    fn spread(v: u32) -> u64 {
        let mut v = (v & 0xFFFF) as u64;
        v = (v | (v << 8)) & 0x00FF_00FF;
        v = (v | (v << 4)) & 0x0F0F_0F0F;
        v = (v | (v << 2)) & 0x3333_3333;
        v = (v | (v << 1)) & 0x5555_5555;
        v
    }
    spread(x) | (spread(y) << 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_group_merges_into_one_level() {
        let bb = [[0.0, 0.0, 1.0, 1.0], [5.0, 5.0, 6.0, 6.0], [0.5, 0.5, 2.0, 2.0]];
        let lv = assign_levels(&bb, &[0, 0, 0], 0.01, 1.0).unwrap();
        assert_eq!(lv, vec![0, 0, 0]);
    }

    #[test]
    fn overlapping_groups_keep_drawing_order() {
        // A, B, A over the same area: the second A has to go above B
        let bb = [[0.0, 0.0, 1.0, 1.0], [0.0, 0.0, 1.0, 1.0], [0.0, 0.0, 1.0, 1.0]];
        let lv = assign_levels(&bb, &[0, 1, 0], 0.0, 1.0).unwrap();
        assert!(lv[0] < lv[1] && lv[1] < lv[2], "{:?}", lv);
    }

    #[test]
    fn disjoint_groups_share_levels() {
        // A, B, A far apart from each other: nothing overlaps, so both A can stay in one level
        let bb = [[0.0, 0.0, 1.0, 1.0], [100.0, 100.0, 101.0, 101.0], [0.0, 0.0, 1.0, 1.0]];
        let lv = assign_levels(&bb, &[0, 1, 0], 0.0, 1.0).unwrap();
        assert_eq!(lv[0], lv[2]);
    }

    #[test]
    fn validates_input() {
        assert!(assign_levels(&[[0.0, 0.0, f64::NAN, 1.0]], &[0], 0.0, 1.0).is_err());
        assert!(assign_levels(&[[0.0, 0.0, 1.0, 1.0]], &[-1], 0.0, 1.0).is_err());
        assert!(assign_levels(&[[0.0, 0.0, 1.0, 1.0]], &[0], 0.0, 0.0).is_err());
        assert!(assign_levels(&[[0.0, 0.0, 1.0, 1.0]], &[], 0.0, 1.0).is_err());
        assert!(assign_levels(&[], &[], 0.0, 1.0).unwrap().is_empty());
    }

    #[test]
    fn grid_cell_stays_within_cell_limit() {
        let bb: Vec<[f64; 4]> = (0..10).map(|i| [i as f64, 0.0, i as f64 + 1e-9, 1e-9]).collect();
        let cell = grid_cell(&bb, 1e-12, VISIT_BUDGET);
        let (_, _, w, h) = bounds(&bb, 1e-12);
        assert!((w / cell + 1.0) * (h / cell + 1.0) <= MAX_CELLS as f64);
    }

    #[test]
    fn morton_interleaves() {
        assert_eq!(morton(0b11, 0), 0b0101);
        assert_eq!(morton(0, 0b11), 0b1010);
        assert_eq!(morton(0xFFFF, 0xFFFF), 0xFFFF_FFFF);
    }
}
