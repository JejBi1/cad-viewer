//! CPU side of the 3D scene: turns `ProtoMesh` / `ModelTree` into the vertex, index and instance data the GPU draws.
//! Everything here is plain data and unit tested; no wgpu objects.
use std::collections::HashSet;

use bytemuck::{Pod, Zeroable};
use cad_types::{Aabb, ModelTree, NodeId, ProtoMesh, TreeNode};
use glam::{DMat4, DVec3, Vec3};

use crate::colors::{pack_rgba, rgb_to_srgb8, Rgb8};

/// Vertex of a face mesh. Every vertex belongs to exactly one B-rep face (`face`), so a face can be coloured, picked
/// and highlighted from the same buffers.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub face: u32,
    pub normal: [f32; 3],
    /// Face colour as RGBA bytes (sRGB); alpha 0 = the face has no colour of its own.
    pub color: u32,
}

/// Vertex of the edge polylines.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct EdgeVertex {
    pub pos: [f32; 3],
    pub edge: u32,
}

/// Face id of triangles that belong to no `FaceSpan`.
pub const NO_FACE: u32 = u32::MAX;

/// GPU-ready data of one prototype.
#[derive(Clone, Debug, Default)]
pub struct ProtoData {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
    /// `(first_index, index_count)` per face, in `FaceSpan` order (clamped to the index buffer).
    pub face_ranges: Vec<(u32, u32)>,
    pub edge_vertices: Vec<EdgeVertex>,
    /// Line list: two indices per segment.
    pub edge_indices: Vec<u32>,
    /// `(first_index, index_count)` into `edge_indices` per edge.
    pub edge_ranges: Vec<(u32, u32)>,
    /// Polyline points of the edges (CPU copy for highlights) and their spans.
    pub edge_points: Vec<[f32; 3]>,
    pub edge_spans: Vec<(u32, u32)>,
    /// B-rep vertices.
    pub points: Vec<[f32; 3]>,
    pub bounds: Option<Aabb>,
    pub mesh_only: bool,
}

fn norm_or(n: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let v = Vec3::from_array(n);
    let l = v.length();
    if l.is_finite() && l > 1e-20 { (v / l).to_array() } else { fallback }
}

/// Bounds of a point list, `None` when empty or non-finite.
pub fn bounds_of(points: &[[f32; 3]]) -> Option<Aabb> {
    let mut lo = [f64::MAX; 3];
    let mut hi = [f64::MIN; 3];
    let mut any = false;
    for p in points {
        if !(p[0].is_finite() && p[1].is_finite() && p[2].is_finite()) {
            continue;
        }
        any = true;
        for k in 0..3 {
            lo[k] = lo[k].min(f64::from(p[k]));
            hi[k] = hi[k].max(f64::from(p[k]));
        }
    }
    any.then_some(Aabb { min: lo, max: hi })
}

/// Converts a prototype mesh. Never panics on inconsistent input: triangles with out-of-range indices are dropped,
/// a trailing partial triangle is ignored, missing normals produce flat shading.
pub fn build_proto(mesh: &ProtoMesh) -> ProtoData {
    let n_pos = mesh.positions.len();
    let tri_count = mesh.indices.len() / 3;
    let n_idx = tri_count * 3;
    let has_normals = mesh.normals.len() == n_pos && n_pos > 0;

    // face id of every index position
    let mut face_of = vec![NO_FACE; n_idx];
    let mut face_ranges = Vec::with_capacity(mesh.faces.len());
    let mut face_color = Vec::with_capacity(mesh.faces.len());
    for (fi, f) in mesh.faces.iter().enumerate() {
        let first = (f.first_index as usize).min(n_idx);
        let end = first.saturating_add(f.index_count as usize).min(n_idx);
        for slot in &mut face_of[first..end] {
            *slot = fi as u32;
        }
        face_ranges.push((first as u32, (end - first) as u32));
        face_color.push(f.color.map(|c| pack_rgba(rgb_to_srgb8(c), 255)).unwrap_or(0));
    }

    let mut out = ProtoData {
        face_ranges,
        mesh_only: mesh.mesh_only,
        ..ProtoData::default()
    };
    out.indices = Vec::with_capacity(n_idx);
    out.vertices = Vec::with_capacity(if has_normals { n_pos } else { n_idx });

    // remap[old] = new vertex id inside the current run of one face
    let mut remap = vec![0u32; n_pos];
    let mut stamp = vec![0u32; n_pos];
    let mut run = 0u32;
    let mut run_face = u32::MAX - 1;
    for t in 0..tri_count {
        let tri = [mesh.indices[3 * t], mesh.indices[3 * t + 1], mesh.indices[3 * t + 2]];
        let valid = tri.iter().all(|&i| (i as usize) < n_pos);
        let face = face_of[3 * t];
        if !valid {
            // keep the index buffer aligned with FaceSpan offsets: emit a degenerate triangle on a dummy vertex
            if out.vertices.is_empty() {
                out.vertices.push(MeshVertex { face, ..MeshVertex::default() });
            }
            out.indices.extend_from_slice(&[0, 0, 0]);
            continue;
        }
        let color = if face == NO_FACE { 0 } else { face_color[face as usize] };
        if has_normals {
            if face != run_face {
                run += 1;
                run_face = face;
            }
            for &i in &tri {
                let iu = i as usize;
                if stamp[iu] != run {
                    stamp[iu] = run;
                    remap[iu] = out.vertices.len() as u32;
                    out.vertices.push(MeshVertex {
                        pos: mesh.positions[iu],
                        face,
                        normal: norm_or(mesh.normals[iu], [0.0, 0.0, 1.0]),
                        color,
                    });
                }
                out.indices.push(remap[iu]);
            }
        } else {
            let p = tri.map(|i| Vec3::from_array(mesh.positions[i as usize]));
            let n = norm_or((p[1] - p[0]).cross(p[2] - p[0]).to_array(), [0.0, 0.0, 1.0]);
            for &i in &tri {
                out.indices.push(out.vertices.len() as u32);
                out.vertices.push(MeshVertex { pos: mesh.positions[i as usize], face, normal: n, color });
            }
        }
    }

    // edges
    out.edge_points = mesh.edge_points.clone();
    let n_pts = mesh.edge_points.len();
    out.edge_vertices = mesh.edge_points.iter().map(|p| EdgeVertex { pos: *p, edge: NO_FACE }).collect();
    for (ei, e) in mesh.edges.iter().enumerate() {
        let first = (e.first_point as usize).min(n_pts);
        let end = first.saturating_add(e.point_count as usize).min(n_pts);
        out.edge_spans.push((first as u32, (end - first) as u32));
        let start = out.edge_indices.len() as u32;
        for k in first..end {
            out.edge_vertices[k].edge = ei as u32;
            if k + 1 < end {
                out.edge_indices.push(k as u32);
                out.edge_indices.push(k as u32 + 1);
            }
        }
        out.edge_ranges.push((start, out.edge_indices.len() as u32 - start));
    }
    out.points = mesh.vertices.clone();
    out.bounds = mesh.bounds.or_else(|| bounds_of(&mesh.positions)).or_else(|| bounds_of(&mesh.edge_points));
    out
}

/// One placed instance of a prototype (a leaf node of the tree).
#[derive(Clone, Debug)]
pub struct InstanceInfo {
    pub node: NodeId,
    pub proto: u32,
    pub model: DMat4,
    /// Colour of the node (sRGB bytes), `None` = default part colour.
    pub color: Option<Rgb8>,
}

/// Leaves of the tree that carry a prototype in `0..proto_count`.
pub fn collect_instances(tree: &ModelTree, proto_count: usize) -> Vec<InstanceInfo> {
    tree.nodes
        .iter()
        .filter_map(|n| {
            let p = n.proto?;
            if (p as usize) >= proto_count {
                return None;
            }
            Some(InstanceInfo {
                node: n.id,
                proto: p,
                model: DMat4::from_cols_array(&n.location),
                color: n.color.map(rgb_to_srgb8),
            })
        })
        .collect()
}

/// `hidden[node]` for every node: the node itself or any ancestor is in `set`. Nodes are indexed by position; ids that
/// do not match their position are tolerated by walking `parent` links with a depth limit.
pub fn resolve_hidden(nodes: &[TreeNode], set: &HashSet<NodeId>) -> Vec<bool> {
    let mut state: Vec<Option<bool>> = vec![None; nodes.len()];
    let mut out = vec![false; nodes.len()];
    for start in 0..nodes.len() {
        if state[start].is_some() {
            continue;
        }
        // walk up collecting unresolved nodes
        let mut chain = Vec::new();
        let mut cur = Some(start);
        let mut inherited = false;
        let mut depth = 0usize;
        while let Some(i) = cur {
            if let Some(v) = state[i] {
                inherited = v;
                break;
            }
            chain.push(i);
            depth += 1;
            if depth > nodes.len() {
                break; // cycle in bad input
            }
            cur = nodes[i].parent.map(|p| p as usize).filter(|p| *p < nodes.len());
        }
        for &i in chain.iter().rev() {
            inherited = inherited || set.contains(&nodes[i].id);
            state[i] = Some(inherited);
            out[i] = inherited;
        }
    }
    out
}

/// Node ids of all leaves below (and including) a node; assemblies expand, leaves map to themselves.
pub fn leaves_below(nodes: &[TreeNode], node: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![node];
    let mut seen = 0usize;
    while let Some(n) = stack.pop() {
        seen += 1;
        if seen > nodes.len() + 1 {
            break; // cycle guard
        }
        let Some(tn) = nodes.get(n as usize) else { continue };
        if tn.proto.is_some() {
            out.push(n);
        }
        stack.extend(tn.children.iter().copied());
    }
    out
}

/// Bounds of an instance in world coordinates.
pub fn instance_bounds(model: &DMat4, local: &Aabb) -> Aabb {
    let mut lo = DVec3::splat(f64::MAX);
    let mut hi = DVec3::splat(f64::MIN);
    for i in 0..8 {
        let p = DVec3::new(
            if i & 1 == 0 { local.min[0] } else { local.max[0] },
            if i & 2 == 0 { local.min[1] } else { local.max[1] },
            if i & 4 == 0 { local.min[2] } else { local.max[2] },
        );
        let w = model.transform_point3(p);
        lo = lo.min(w);
        hi = hi.max(w);
    }
    Aabb { min: lo.to_array(), max: hi.to_array() }
}

pub fn union(a: Option<Aabb>, b: &Aabb) -> Option<Aabb> {
    Some(match a {
        None => *b,
        Some(a) => Aabb {
            min: [a.min[0].min(b.min[0]), a.min[1].min(b.min[1]), a.min[2].min(b.min[2])],
            max: [a.max[0].max(b.max[0]), a.max[1].max(b.max[1]), a.max[2].max(b.max[2])],
        },
    })
}

pub fn center_of(b: &Aabb) -> DVec3 {
    (DVec3::from_array(b.min) + DVec3::from_array(b.max)) * 0.5
}

pub fn radius_of(b: &Aabb) -> f64 {
    (DVec3::from_array(b.max) - DVec3::from_array(b.min)).length() * 0.5
}

/// Per-instance data of the instance buffer.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct InstanceRaw {
    /// Model matrix relative to the scene origin, column-major.
    pub model: [[f32; 4]; 4],
    /// Own colour (RGBA bytes, alpha 0 = none).
    pub color: u32,
    /// Highlight colour (RGBA bytes, alpha 0 = none).
    pub tint: u32,
    /// Instance index + 1 (0 = background in the id buffers).
    pub id: u32,
    /// Opacity 0..255 in the lowest byte.
    pub opacity: u32,
}

/// Model matrix relative to `origin` as f32.
pub fn model_relative(model: &DMat4, origin: DVec3) -> [[f32; 4]; 4] {
    let mut m = *model;
    m.w_axis -= origin.extend(0.0);
    m.as_mat4().to_cols_array_2d()
}

/// A run of instances of one prototype in the instance buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawRange {
    pub proto: u32,
    pub first: u32,
    pub count: u32,
}

/// Order of the visible instances in the instance buffer: opaque ones by prototype, then transparent ones by
/// prototype.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstanceLayout {
    /// Instance index per slot.
    pub order: Vec<usize>,
    /// Slot per instance index, `u32::MAX` when hidden.
    pub slot_of: Vec<u32>,
    pub opaque: Vec<DrawRange>,
    pub transparent: Vec<DrawRange>,
}

/// `entries[i] = (proto, transparent, hidden)` of instance `i`.
pub fn layout_instances(entries: &[(u32, bool, bool)]) -> InstanceLayout {
    let mut idx: Vec<usize> = (0..entries.len()).filter(|&i| !entries[i].2).collect();
    idx.sort_by_key(|&i| (entries[i].1, entries[i].0, i));
    let mut slot_of = vec![u32::MAX; entries.len()];
    let mut opaque: Vec<DrawRange> = Vec::new();
    let mut transparent: Vec<DrawRange> = Vec::new();
    for (slot, &i) in idx.iter().enumerate() {
        slot_of[i] = slot as u32;
        let (proto, tr, _) = entries[i];
        let list = if tr { &mut transparent } else { &mut opaque };
        match list.last_mut() {
            Some(r) if r.proto == proto && r.first + r.count == slot as u32 => r.count += 1,
            _ => list.push(DrawRange { proto, first: slot as u32, count: 1 }),
        }
    }
    InstanceLayout { order: idx, slot_of, opaque, transparent }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_types::{EdgeSpan, FaceSpan, Rgb};

    /// Two quads sharing no faces: face 0 red, face 1 uncoloured. Vertices 0..4 and 4..8; one shared corner position.
    fn two_quads() -> ProtoMesh {
        ProtoMesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 1.0],
                [1.0, 1.0, 1.0],
                [0.0, 1.0, 1.0],
            ],
            normals: vec![[0.0, 0.0, -1.0]; 4].into_iter().chain(vec![[0.0, 0.0, 2.0]; 4]).collect(),
            indices: vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
            faces: vec![
                FaceSpan { first_index: 0, index_count: 6, color: Some(Rgb(1.0, 0.0, 0.0)) },
                FaceSpan { first_index: 6, index_count: 6, color: None },
            ],
            edge_points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [5.0, 5.0, 5.0], [6.0, 6.0, 6.0]],
            edges: vec![EdgeSpan { first_point: 0, point_count: 3 }, EdgeSpan { first_point: 3, point_count: 2 }],
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]],
            bounds: None,
            mesh_only: false,
        }
    }

    #[test]
    fn proto_keeps_index_order_and_face_ranges() {
        let d = build_proto(&two_quads());
        assert_eq!(d.indices.len(), 12);
        assert_eq!(d.face_ranges, vec![(0, 6), (6, 6)]);
        assert_eq!(d.vertices.len(), 8);
        // each triangle refers to vertices of its own face
        for (fi, &(first, count)) in d.face_ranges.iter().enumerate() {
            for k in first..first + count {
                assert_eq!(d.vertices[d.indices[k as usize] as usize].face, fi as u32);
            }
        }
        // colours: face 0 is pure red (sRGB 255,0,0), face 1 has none (alpha 0)
        assert_eq!(d.vertices[0].color.to_le_bytes(), [255, 0, 0, 255]);
        assert_eq!(d.vertices[7].color >> 24, 0);
        // normals are normalised
        assert!((Vec3::from_array(d.vertices[5].normal).length() - 1.0).abs() < 1e-6);
        let b = d.bounds.expect("bounds");
        assert_eq!(b.min, [0.0; 3]);
        assert_eq!(b.max, [1.0; 3]);
    }

    #[test]
    fn shared_vertices_between_faces_are_duplicated() {
        let mut m = two_quads();
        // second face uses vertex 0 and 2 of the first face
        m.indices = vec![0, 1, 2, 0, 2, 3, 0, 2, 5, 0, 5, 4];
        let d = build_proto(&m);
        for &(first, count) in &d.face_ranges[1..] {
            for k in first..first + count {
                assert_eq!(d.vertices[d.indices[k as usize] as usize].face, 1);
            }
        }
        // shared position appears in both faces
        let faces_with_origin: HashSet<u32> =
            d.vertices.iter().filter(|v| v.pos == [0.0, 0.0, 0.0]).map(|v| v.face).collect();
        assert_eq!(faces_with_origin, HashSet::from([0, 1]));
    }

    #[test]
    fn vertices_within_a_face_are_shared() {
        let d = build_proto(&two_quads());
        // the quad has two triangles sharing two corners: 4 vertices, not 6
        let face0: Vec<_> = d.vertices.iter().filter(|v| v.face == 0).collect();
        assert_eq!(face0.len(), 4);
    }

    #[test]
    fn missing_normals_give_flat_shading() {
        let mut m = two_quads();
        m.normals.clear();
        let d = build_proto(&m);
        assert_eq!(d.vertices.len(), 12); // unshared
        assert_eq!(d.indices, (0..12).collect::<Vec<u32>>());
        // CCW quad in the z = 0 plane: +Z normal
        assert!((Vec3::from_array(d.vertices[0].normal) - Vec3::Z).length() < 1e-6);
    }

    #[test]
    fn bad_input_is_tolerated() {
        let mut m = two_quads();
        m.indices = vec![0, 1, 2, 0, 2, 99, 4, 5, 6, 4, 6]; // out of range + partial triangle
        m.faces[1].index_count = 1000; // overlong span
        m.faces[0].first_index = 4000; // beyond the buffer
        let d = build_proto(&m);
        assert_eq!(d.indices.len(), 9);
        assert!(d.face_ranges[0].0 as usize <= d.indices.len());
        assert!((d.face_ranges[1].0 + d.face_ranges[1].1) as usize <= d.indices.len());
        for &i in &d.indices {
            assert!((i as usize) < d.vertices.len());
        }
        let empty = build_proto(&ProtoMesh::default());
        assert!(empty.vertices.is_empty() && empty.bounds.is_none());
    }

    #[test]
    fn triangles_outside_every_face_get_no_face() {
        let mut m = two_quads();
        m.faces.truncate(1);
        let d = build_proto(&m);
        assert!(d.vertices.iter().any(|v| v.face == NO_FACE));
        assert!(d.vertices.iter().filter(|v| v.face == NO_FACE).all(|v| v.color >> 24 == 0));
    }

    #[test]
    fn edges_become_line_lists() {
        let d = build_proto(&two_quads());
        assert_eq!(d.edge_indices, vec![0, 1, 1, 2, 3, 4]);
        assert_eq!(d.edge_ranges, vec![(0, 4), (4, 2)]);
        assert_eq!(d.edge_vertices[2].edge, 0);
        assert_eq!(d.edge_vertices[4].edge, 1);
        assert_eq!(d.edge_spans, vec![(0, 3), (3, 2)]);
        assert_eq!(d.points.len(), 2);
    }

    #[test]
    fn bounds_fall_back_to_edges_for_wire_only_protos() {
        let m = ProtoMesh {
            edge_points: vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]],
            edges: vec![EdgeSpan { first_point: 0, point_count: 2 }],
            ..ProtoMesh::default()
        };
        let d = build_proto(&m);
        assert_eq!(d.bounds.expect("bounds").max, [4.0, 5.0, 6.0]);
        assert_eq!(bounds_of(&[[f32::NAN; 3]]), None);
    }

    fn node(id: NodeId, parent: Option<NodeId>, children: Vec<NodeId>, proto: Option<u32>) -> TreeNode {
        TreeNode {
            id,
            parent,
            children,
            name: format!("n{id}"),
            proto_name: String::new(),
            key: String::new(),
            is_assembly: proto.is_none(),
            proto,
            location: cad_types::IDENTITY,
            color: None,
            own_style: false,
        }
    }

    fn tree() -> ModelTree {
        // 0 root assembly -> 1 sub assembly -> 3, 4 leaves; 2 leaf
        let mut t = ModelTree {
            roots: vec![0],
            nodes: vec![
                node(0, None, vec![1, 2], None),
                node(1, Some(0), vec![3, 4], None),
                node(2, Some(0), vec![], Some(0)),
                node(3, Some(1), vec![], Some(1)),
                node(4, Some(1), vec![], Some(1)),
            ],
            proto_instances: vec![1, 2],
        };
        t.nodes[3].location[12] = 10.0;
        t.nodes[4].color = Some(Rgb(1.0, 1.0, 1.0));
        t
    }

    #[test]
    fn instances_are_the_leaves() {
        let t = tree();
        let inst = collect_instances(&t, 2);
        assert_eq!(inst.iter().map(|i| i.node).collect::<Vec<_>>(), vec![2, 3, 4]);
        assert_eq!(inst[1].model.w_axis.x, 10.0);
        assert_eq!(inst[2].color, Some([255, 255, 255]));
        assert_eq!(inst[0].color, None);
        // a proto id beyond the uploaded meshes is skipped instead of crashing
        assert_eq!(collect_instances(&t, 1).len(), 1);
    }

    #[test]
    fn hidden_assembly_hides_its_leaves() {
        let t = tree();
        let h = resolve_hidden(&t.nodes, &HashSet::from([1]));
        assert_eq!(h, vec![false, true, false, true, true]);
        let h = resolve_hidden(&t.nodes, &HashSet::from([0]));
        assert!(h.iter().all(|x| *x));
        let h = resolve_hidden(&t.nodes, &HashSet::from([4]));
        assert_eq!(h, vec![false, false, false, false, true]);
        assert!(resolve_hidden(&t.nodes, &HashSet::new()).iter().all(|x| !*x));
    }

    #[test]
    fn leaves_below_expands_assemblies() {
        let t = tree();
        let mut l = leaves_below(&t.nodes, 0);
        l.sort();
        assert_eq!(l, vec![2, 3, 4]);
        assert_eq!(leaves_below(&t.nodes, 3), vec![3]);
        assert!(leaves_below(&t.nodes, 99).is_empty());
    }

    #[test]
    fn instance_bounds_transform_the_corners() {
        let local = Aabb { min: [0.0; 3], max: [1.0, 2.0, 3.0] };
        let m = DMat4::from_translation(DVec3::new(10.0, 0.0, 0.0)) * DMat4::from_rotation_z(std::f64::consts::FRAC_PI_2);
        let b = instance_bounds(&m, &local);
        assert!((b.min[0] - 8.0).abs() < 1e-9 && (b.max[0] - 10.0).abs() < 1e-9);
        assert!((b.min[1] - 0.0).abs() < 1e-9 && (b.max[1] - 1.0).abs() < 1e-9);
        assert_eq!(union(None, &b), Some(b));
        let u = union(Some(b), &Aabb { min: [-1.0; 3], max: [0.0; 3] }).expect("union");
        assert_eq!(u.min[0], -1.0);
        assert_eq!(u.max[0], 10.0);
        assert!((radius_of(&Aabb { min: [0.0; 3], max: [2.0, 0.0, 0.0] }) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn model_is_made_relative_to_the_origin() {
        let m = DMat4::from_translation(DVec3::new(1_000_000.5, 2.0, 3.0));
        let r = model_relative(&m, DVec3::new(1_000_000.0, 0.0, 0.0));
        assert_eq!(r[3], [0.5, 2.0, 3.0, 1.0]);
        assert_eq!(r[0], [1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn layout_sorts_opaque_before_transparent_and_groups_protos() {
        // (proto, transparent, hidden)
        let e = [(1, false, false), (0, false, false), (1, false, false), (0, true, false), (0, false, true), (1, true, false)];
        let l = layout_instances(&e);
        assert_eq!(l.order, vec![1, 0, 2, 3, 5]);
        assert_eq!(l.slot_of, vec![1, 0, 2, 3, u32::MAX, 4]);
        assert_eq!(
            l.opaque,
            vec![DrawRange { proto: 0, first: 0, count: 1 }, DrawRange { proto: 1, first: 1, count: 2 }]
        );
        assert_eq!(
            l.transparent,
            vec![DrawRange { proto: 0, first: 3, count: 1 }, DrawRange { proto: 1, first: 4, count: 1 }]
        );
        assert_eq!(layout_instances(&[]), InstanceLayout::default());
    }

    #[test]
    fn raw_layouts_have_the_sizes_the_shaders_expect() {
        assert_eq!(std::mem::size_of::<MeshVertex>(), 32);
        assert_eq!(std::mem::size_of::<EdgeVertex>(), 16);
        assert_eq!(std::mem::size_of::<InstanceRaw>(), 80);
    }
}
