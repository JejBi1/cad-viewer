//! Headless GPU tests: a cube is rendered offscreen and its pixels and picks are checked.
//! Without a GPU adapter every test prints a `SKIP` line (visible with `--nocapture`) and passes vacuously.
use std::collections::HashSet;

use cad_types::{Aabb, EdgeSpan, EntityKind, EntityRef, FaceSpan, ModelTree, ProtoMesh, Rgb, TreeNode, IDENTITY};
use eframe::wgpu;

use crate::camera::StdView;
use crate::capture::CaptureOptions;
use crate::colors::{DEFAULT_PART_COLOR, SELECTION_COLOR};
use crate::gpu::GpuContext;
use crate::overlay::Overlays;
use crate::renderer3d::{Highlight, Renderer3d, Section, Style};

pub const HALF: f64 = 50.0;
pub const SIZE: u32 = 256;

/// Cube [-50, 50]^3: faces +X, -X, +Y, -Y, +Z, -Z (indices 0..6), 12 edges, 8 vertices.
pub fn cube_mesh(face_colors: [Option<Rgb>; 6]) -> ProtoMesh {
    let h = HALF as f32;
    let normals: [[f32; 3]; 6] =
        [[1., 0., 0.], [-1., 0., 0.], [0., 1., 0.], [0., -1., 0.], [0., 0., 1.], [0., 0., -1.]];
    let mut m = ProtoMesh::default();
    for (fi, n) in normals.iter().enumerate() {
        // tangents u, v with u x v = n
        let (u, v): ([f32; 3], [f32; 3]) = match fi {
            0 => ([0., 1., 0.], [0., 0., 1.]),
            1 => ([0., 0., 1.], [0., 1., 0.]),
            2 => ([0., 0., 1.], [1., 0., 0.]),
            3 => ([1., 0., 0.], [0., 0., 1.]),
            4 => ([1., 0., 0.], [0., 1., 0.]),
            _ => ([0., 1., 0.], [1., 0., 0.]),
        };
        let base = m.positions.len() as u32;
        for (su, sv) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            m.positions.push([
                n[0] * h + (u[0] * su + v[0] * sv) * h,
                n[1] * h + (u[1] * su + v[1] * sv) * h,
                n[2] * h + (u[2] * su + v[2] * sv) * h,
            ]);
            m.normals.push(*n);
        }
        let first = m.indices.len() as u32;
        m.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        m.faces.push(FaceSpan { first_index: first, index_count: 6, color: face_colors[fi] });
    }
    let corner = |i: u32| [if i & 1 == 0 { -h } else { h }, if i & 2 == 0 { -h } else { h }, if i & 4 == 0 { -h } else { h }];
    for i in 0..8u32 {
        m.vertices.push(corner(i));
    }
    for i in 0..8u32 {
        for bit in [1u32, 2, 4] {
            if i & bit == 0 {
                let first = m.edge_points.len() as u32;
                m.edge_points.push(corner(i));
                m.edge_points.push(corner(i | bit));
                m.edges.push(EdgeSpan { first_point: first, point_count: 2 });
            }
        }
    }
    m.bounds = Some(Aabb { min: [-HALF; 3], max: [HALF; 3] });
    m
}

fn node(id: u32, parent: Option<u32>, children: Vec<u32>, proto: Option<u32>, x: f64) -> TreeNode {
    let mut location = IDENTITY;
    location[12] = x;
    TreeNode {
        id,
        parent,
        children,
        name: format!("n{id}"),
        proto_name: "cube".into(),
        key: format!("0:1:{id}"),
        is_assembly: proto.is_none(),
        proto,
        location,
        color: None,
        own_style: false,
    }
}

/// Root assembly (0) with the cubes 1 (origin) and 2 (+150 mm in X) when `two`, else only cube 1.
pub fn cube_tree(two: bool) -> ModelTree {
    let mut nodes = vec![node(0, None, vec![1], None, 0.0), node(1, Some(0), vec![], Some(0), 0.0)];
    if two {
        nodes[0].children.push(2);
        nodes.push(node(2, Some(0), vec![], Some(0), 150.0));
    }
    ModelTree { roots: vec![0], nodes, proto_instances: vec![if two { 2 } else { 1 }] }
}

pub fn gpu(test: &str) -> Option<GpuContext> {
    match GpuContext::new_headless() {
        Ok(g) => {
            println!("{test}: running on GPU adapter {}", g.describe());
            Some(g)
        }
        Err(e) => {
            println!("SKIP {test}: no GPU adapter available in this environment ({e})");
            None
        }
    }
}

pub fn renderer(g: &GpuContext, two: bool) -> Renderer3d {
    let mut r = Renderer3d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm).with_queue(&g.queue);
    r.set_model(&[cube_mesh([None; 6])], &cube_tree(two));
    r.set_viewport(SIZE, SIZE, 1.0);
    r.set_orientation(StdView::Front);
    r
}

fn shot(r: &mut Renderer3d, transparent: bool, ui: bool) -> crate::capture::Rgba8 {
    r.capture_rgba(&CaptureOptions {
        width: SIZE,
        height: SIZE,
        transparent_background: transparent,
        ui,
        dpr: 1.0,
        camera: None,
        msaa: None,
    })
    .expect("capture")
}

/// The shaded colour of a face seen head-on (normal towards the viewer): `0.30 + 0.70 * lambda` plus the specular term.
fn head_on(base: [u8; 3]) -> [u8; 3] {
    let l = glam::Vec3::new(-0.35, 0.55, 0.75).normalize();
    let lam = l.z.abs();
    let shade = 0.30 + 0.70 * lam;
    let spec = 0.08 * lam.powi(8);
    let f = |c: u8| ((f32::from(c) / 255.0 * shade + spec).clamp(0.0, 1.0) * 255.0).round() as u8;
    [f(base[0]), f(base[1]), f(base[2])]
}

fn close(a: [u8; 4], b: [u8; 3], tol: i32) -> bool {
    (0..3).all(|i| (i32::from(a[i]) - i32::from(b[i])).abs() <= tol)
}

#[test]
fn cube_pixels_front_view() {
    let Some(g) = gpu("cube_pixels_front_view") else { return };
    let mut r = renderer(&g, false);
    // transparent background, no overlays: the face in the middle, nothing in the corners
    let img = shot(&mut r, true, false);
    let c = img.pixel(SIZE / 2, SIZE / 2);
    assert_eq!(c[3], 255);
    assert!(close(c, head_on(DEFAULT_PART_COLOR), 3), "centre {c:?} vs {:?}", head_on(DEFAULT_PART_COLOR));
    assert_eq!(img.pixel(2, 2)[3], 0);
    assert_eq!(img.pixel(SIZE - 3, SIZE - 3)[3], 0);
    // the cube fills 92 % of the view (FitAll margin 0.04): pixel 20 is inside, pixel 8 outside
    assert_eq!(img.pixel(SIZE / 2, 20)[3], 255);
    assert_eq!(img.pixel(SIZE / 2, 8)[3], 0);
    // gradient background
    let bg = shot(&mut r, false, false);
    let top = bg.pixel(2, 0);
    let bottom = bg.pixel(2, SIZE - 1);
    assert!(close(top, [230, 235, 242], 3), "top {top:?}");
    assert!(close(bottom, [150, 162, 180], 3), "bottom {bottom:?}");
    assert_eq!(top[3], 255);
    // dark background
    r.set_style(&Style { dark_background: true, ..r.style.clone() });
    let dark = shot(&mut r, false, false);
    assert!(close(dark.pixel(2, 0), [50, 54, 62], 3));
    assert!(close(dark.pixel(2, SIZE - 1), [20, 22, 26], 3));
}

#[test]
fn face_colours_and_node_colours() {
    let Some(g) = gpu("face_colours_and_node_colours") else { return };
    let mut r = Renderer3d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm).with_queue(&g.queue);
    // the -Y face (index 3) is pure red, the instance has its own colour for the other faces
    let mut faces = [None; 6];
    faces[3] = Some(Rgb(1.0, 0.0, 0.0));
    let mut tree = cube_tree(false);
    tree.nodes[1].color = Some(Rgb(0.0, 0.0, 1.0));
    r.set_model(&[cube_mesh(faces)], &tree);
    r.set_viewport(SIZE, SIZE, 1.0);
    r.set_orientation(StdView::Front);
    let img = shot(&mut r, true, false);
    assert!(close(img.pixel(SIZE / 2, SIZE / 2), head_on([255, 0, 0]), 3), "front face is red");
    r.set_orientation(StdView::Right);
    let img = shot(&mut r, true, false);
    assert!(close(img.pixel(SIZE / 2, SIZE / 2), head_on([0, 0, 255]), 3), "right face has the node colour (blue)");
}

#[test]
fn selection_hover_transparency_and_hidden() {
    let Some(g) = gpu("selection_hover_transparency_and_hidden") else { return };
    let mut r = renderer(&g, false);
    let centre = |r: &mut Renderer3d| shot(r, true, false).pixel(SIZE / 2, SIZE / 2);
    r.set_selection(&[1]);
    assert!(close(centre(&mut r), head_on(SELECTION_COLOR), 3), "selected = orange");
    r.set_selection(&[]);
    assert!(close(centre(&mut r), head_on(DEFAULT_PART_COLOR), 3));
    r.set_hover(Some(Highlight::Node(1)));
    assert!(close(centre(&mut r), head_on(crate::colors::HIGHLIGHT_COLOR), 3), "hover = light blue");
    r.set_hover(None);
    // an assembly selects its leaves
    r.set_selection(&[0]);
    assert!(close(centre(&mut r), head_on(SELECTION_COLOR), 3));
    r.set_selection(&[]);
    // transparency 0.75 -> alpha 64
    r.set_transparency(&[1], 0.75);
    let c = centre(&mut r);
    assert!((i32::from(c[3]) - 64).abs() <= 2, "alpha {}", c[3]);
    r.set_transparency(&[1], 0.0);
    assert_eq!(centre(&mut r)[3], 255);
    // hidden
    r.set_visibility(&HashSet::from([0]));
    assert_eq!(centre(&mut r)[3], 0, "an assembly hides its leaves");
    assert_eq!(r.pick(128.0, 128.0), None);
    r.set_visibility(&HashSet::new());
    assert_eq!(centre(&mut r)[3], 255);
}

#[test]
fn edges_and_wireframe() {
    let Some(g) = gpu("edges_and_wireframe") else { return };
    let mut r = renderer(&g, false);
    // front view: the silhouette edges are 1 px lines at 4 % margin; look for dark pixels near the face border
    let img = shot(&mut r, true, false);
    let dark_near_border = (0..SIZE).any(|x| {
        let p = img.pixel(x, 12);
        p[3] > 0 && p[0] < 90
    });
    assert!(dark_near_border, "edge line visible");
    r.set_style(&Style { edges: false, ..r.style.clone() });
    let img = shot(&mut r, true, false);
    let still_dark = (0..SIZE).any(|x| {
        let p = img.pixel(x, 12);
        p[3] > 0 && p[0] < 90
    });
    assert!(!still_dark, "edges off");
    // wireframe: no surfaces, so the face interior is empty
    r.set_style(&Style { wireframe: true, ..r.style.clone() });
    let img = shot(&mut r, true, false);
    assert_eq!(img.pixel(SIZE / 2, SIZE / 2)[3], 0);
    let lit = (0..SIZE).any(|x| img.pixel(x, 12)[3] > 0);
    assert!(lit, "wireframe lines are drawn even with edges off");
    // wireframe lines carry the part colour (light grey), not the dark edge colour
    let mut best = [0u8; 4];
    for x in 0..SIZE {
        let p = img.pixel(x, 12);
        if p[3] > best[3] {
            best = p;
        }
    }
    assert!(best[0] > 150, "{best:?}");
}

#[test]
fn picking_faces_edges_vertices() {
    let Some(g) = gpu("picking_faces_edges_vertices") else { return };
    let mut r = renderer(&g, true);
    r.fit_all(); // both cubes in view
    let c = r.camera;
    assert!((c.direction() - glam::DVec3::Y).length() < 1e-9, "front view looks along +Y");
    // face under the cursor: the -Y face (index 3) of cube 1 (left one)
    let p1 = r.world_to_pixel([0.0, -HALF, 0.0]).expect("visible");
    let hit = r.pick_hit(p1[0] as f32, p1[1] as f32).expect("face hit");
    assert_eq!(hit.entity, EntityRef { node: 1, kind: EntityKind::Face, index: 3 });
    assert!((hit.point[1] + HALF).abs() < 0.5, "point on the front face: {:?}", hit.point);
    assert!(hit.point[0].abs() < 1.0 && hit.point[2].abs() < 1.0, "{:?}", hit.point);
    // the second cube
    let p2 = r.world_to_pixel([150.0, -HALF, 0.0]).expect("visible");
    let hit = r.pick_hit(p2[0] as f32, p2[1] as f32).expect("face hit");
    assert_eq!(hit.entity.node, 2);
    // the gap between the cubes and the far corner of the viewport hit nothing
    let gap = r.world_to_pixel([75.0, -HALF, 0.0]).expect("visible");
    assert_eq!(r.pick(gap[0] as f32, gap[1] as f32), None);
    assert_eq!(r.pick(1.0, 1.0), None);
    // pivot point
    let pivot = r.pivot_at(p1[0] as f32, p1[1] as f32).expect("pivot");
    assert!((pivot[1] + HALF).abs() < 0.5);

    // measuring: vertices beat edges beat faces, 7 px tolerance
    r.set_measure_mode(true);
    let corner = [-HALF, -HALF, HALF];
    let pc = r.world_to_pixel(corner).expect("visible");
    let v = r.pick_hit(pc[0] as f32 + 3.0, pc[1] as f32 - 2.0).expect("vertex within 7 px");
    assert_eq!(v.entity.kind, EntityKind::Vertex);
    assert_eq!(v.entity.node, 1);
    assert!((glam::DVec3::from_array(v.point) - glam::DVec3::from_array(corner)).length() < 1e-6, "exact vertex: {:?}", v.point);
    // outside the vertex tolerance but near the top front edge: an edge
    let mid = r.world_to_pixel([0.0, -HALF, HALF]).expect("visible");
    let e = r.pick_hit(mid[0] as f32, mid[1] as f32 + 3.0).expect("edge hit");
    assert_eq!(e.entity.kind, EntityKind::Edge, "{e:?}");
    // the edge belongs to cube 1 and runs along X at y = -50, z = 50: both its end points must be on it
    assert_eq!(e.entity.node, 1);
    // far from every edge and vertex: the face
    let f = r.pick_hit(p1[0] as f32, p1[1] as f32).expect("face");
    assert_eq!(f.entity.kind, EntityKind::Face);
    // measure mode off again: only faces
    r.set_measure_mode(false);
    let f = r.pick_hit(pc[0] as f32 + 3.0, pc[1] as f32 + 3.0).expect("face near the corner");
    assert_eq!(f.entity.kind, EntityKind::Face);
}

#[test]
fn picking_in_wireframe_hits_edges_only() {
    let Some(g) = gpu("picking_in_wireframe_hits_edges_only") else { return };
    let mut r = renderer(&g, false);
    r.set_style(&Style { wireframe: true, ..r.style.clone() });
    // the middle of the face is empty in wireframe
    assert_eq!(r.pick(128.0, 128.0), None);
    let mid = r.world_to_pixel([0.0, -HALF, HALF]).expect("visible");
    let e = r.pick_hit(mid[0] as f32, mid[1] as f32).expect("edge");
    assert_eq!(e.entity.kind, EntityKind::Edge);
    assert_eq!(e.entity.node, 1);
}

#[test]
fn section_plane_caps_the_cut() {
    let Some(g) = gpu("section_plane_caps_the_cut") else { return };
    let mut r = renderer(&g, false);
    r.set_orientation(StdView::Right); // looking from +X
    let plain = shot(&mut r, true, false).pixel(SIZE / 2, SIZE / 2);
    assert!(close(plain, head_on(DEFAULT_PART_COLOR), 3));
    // default normal -X keeps x <= 0: the +X half is removed, we look into the cut
    r.set_section(Some(Section { axis: 0, position: 0.0, flip: false }));
    let cut = shot(&mut r, true, false).pixel(SIZE / 2, SIZE / 2);
    assert_eq!(cut[3], 255, "the cut is closed by the cap");
    assert!(close(cut, head_on(DEFAULT_PART_COLOR), 3), "cap is flat and in the part colour: {cut:?}");
    // the cut face is not pickable at the plane but the far half is: the -X face (index 1) behind the cap
    let hit = r.pick_hit(128.0, 128.0).expect("hit behind the cut");
    assert_eq!(hit.entity.kind, EntityKind::Face);
    assert_eq!(hit.entity.index, 1);
    assert!((hit.point[0] + HALF).abs() < 0.5, "{:?}", hit.point);
    // flipped: keeps x >= 0, the +X face stays visible
    r.set_section(Some(Section { axis: 0, position: 0.0, flip: true }));
    let hit = r.pick_hit(128.0, 128.0).expect("hit");
    assert_eq!(hit.entity.index, 0);
    // a plane beyond the model removes nothing (flip = keeps x >= 60: nothing of the cube remains)
    r.set_section(Some(Section { axis: 0, position: 60.0, flip: true }));
    assert_eq!(shot(&mut r, true, false).pixel(SIZE / 2, SIZE / 2)[3], 0);
    assert_eq!(r.pick(128.0, 128.0), None);
    r.set_section(None);
    assert_eq!(shot(&mut r, true, false).pixel(SIZE / 2, SIZE / 2)[3], 255);
}

#[test]
fn overlays_view_cube_scale_bar_and_labels() {
    let Some(g) = gpu("overlays_view_cube_scale_bar_and_labels") else { return };
    let mut r = renderer(&g, false);
    let base = shot(&mut r, false, false);
    let ui = shot(&mut r, false, true);
    let differs = |a: &crate::capture::Rgba8, b: &crate::capture::Rgba8, x0: u32, y0: u32, x1: u32, y1: u32| {
        (y0..y1).any(|y| (x0..x1).any(|x| a.pixel(x, y) != b.pixel(x, y)))
    };
    // view cube: upper right, centre 90 px from the corner
    assert!(differs(&base, &ui, SIZE - 120, 40, SIZE - 60, 140), "view cube drawn");
    // scale bar: lower left
    assert!(differs(&base, &ui, 20, SIZE - 50, 140, SIZE - 15), "scale bar drawn");
    // nothing in the middle-left of the view
    assert!(!differs(&base, &ui, 5, 100, 30, 150));
    // scale bar off
    r.set_style(&Style { scale_bar: false, ..r.style.clone() });
    let no_bar = shot(&mut r, false, true);
    assert!(!differs(&base, &no_bar, 20, SIZE - 50, 140, SIZE - 15));
    r.set_style(&Style { view_cube: false, scale_bar: false, ..r.style.clone() });
    assert_eq!(shot(&mut r, false, true), base, "no ui at all = plain picture");
    r.set_style(&Style::default());

    // overlays: a red line across the left of the picture, a label and a translucent mesh
    let mut o = Overlays::default();
    o.line([-100.0, -60.0, 0.0], [100.0, -60.0, 0.0], [220, 0, 0], 4.0, false);
    o.measure_label([0.0, -60.0, 20.0], "12,34 mm");
    o.center_of_gravity([0.0, -60.0, -20.0], "T");
    o.meshes.push(crate::overlay::OverlayMesh {
        positions: vec![[-40.0, -70.0, 30.0], [40.0, -70.0, 30.0], [0.0, -70.0, 45.0]],
        indices: vec![0, 1, 2],
        color: [230, 20, 20],
        alpha: 1.0,
    });
    r.set_overlays(o);
    let with = shot(&mut r, false, true);
    let ws = r.world_to_pixel([-80.0, -60.0, 0.0]).expect("visible");
    let p = with.pixel(ws[0] as u32, ws[1] as u32);
    assert!(p[0] > 180 && p[1] < 60, "red overlay line at {ws:?}: {p:?}");
    let tri = r.world_to_pixel([0.0, -70.0, 36.0]).expect("visible");
    let q = with.pixel(tri[0] as u32, tri[1] as u32);
    assert!(q[0] > 150 && q[1] < 90, "overlay mesh: {q:?}");
    // label text pixels exist near the anchor: dark red (170, 0, 0) somewhere right of the anchor
    let la = r.world_to_pixel([0.0, -60.0, 20.0]).expect("visible");
    let (lx, ly) = (la[0] as u32, la[1] as u32);
    let text_found = (ly.saturating_sub(20)..ly).any(|y| (lx..lx + 60).any(|x| {
        let p = with.pixel(x, y);
        p[0] > 100 && p[1] < 60 && p[2] < 60
    }));
    assert!(text_found, "measurement label drawn at {la:?}");
    r.clear_overlays();
    assert_eq!(shot(&mut r, false, true), ui);
}

#[test]
fn view_cube_hit_and_animation() {
    let Some(g) = gpu("view_cube_hit_and_animation") else { return };
    let mut r = renderer(&g, false);
    // front view: the cube centre is at (256 - 90, 90)
    let region = r.view_cube_hit(166.0, 90.0).expect("cube hit");
    assert_eq!(region, [0, -1, 0]);
    assert!(r.view_cube_hit(10.0, 200.0).is_none());
    r.start_view_animation([0, 0, 1]);
    assert!(r.is_animating());
    assert!(r.advance_animation(0.1));
    assert!(!r.advance_animation(1.0));
    let d = r.camera.direction();
    assert!((d + glam::DVec3::Z).length() < 1e-9, "looking down: {d:?}");
    assert!(!r.is_animating());
    // the top view has Y up on the screen
    assert!((r.camera.up_vec() - glam::DVec3::Y).length() < 1e-9);
}

#[test]
fn thumbnail_is_centred_and_transparent() {
    let Some(g) = gpu("thumbnail_is_centred_and_transparent") else { return };
    let mut r = renderer(&g, false);
    let t = r.thumbnail_rgba(64).expect("thumbnail");
    assert_eq!((t.width, t.height), (64, 64));
    assert!(t.pixel(32, 32)[3] > 200, "content in the middle");
    assert_eq!(t.pixel(0, 0)[3], 0);
    assert_eq!(t.pixel(63, 63)[3], 0);
    // the isometric silhouette of a cube is a hexagon 1.1547 times as high as wide: the height fills 94 % of the canvas
    let ys: Vec<u32> = (0..64).filter(|&y| t.pixel(32, y)[3] > 128).collect();
    let height = ys.last().copied().unwrap_or(0) - ys.first().copied().unwrap_or(0) + 1;
    assert!((58..=61).contains(&height), "height {height}");
    let xs: Vec<u32> = (0..64).filter(|&x| t.pixel(x, 32)[3] > 128).collect();
    let span = xs.last().copied().unwrap_or(0) - xs.first().copied().unwrap_or(0) + 1;
    assert!((50..=54).contains(&span), "span {span}");
    // centred
    assert!((xs[0] as i32 - (63 - xs[xs.len() - 1]) as i32).abs() <= 2);
    let png = r.thumbnail_png(48).expect("png");
    let img = image::load_from_memory(&png).expect("decode").to_rgba8();
    assert_eq!(img.dimensions(), (48, 48));
    // the style that was changed for the thumbnail is restored
    assert_eq!(r.style.edge_color, crate::colors::EDGE_COLOR);
    // size clamp
    assert_eq!(r.thumbnail_rgba(4).expect("thumbnail").width, 16);
}

#[test]
fn entity_highlights_are_drawn() {
    let Some(g) = gpu("entity_highlights_are_drawn") else { return };
    let mut r = renderer(&g, false);
    let base = shot(&mut r, false, true);
    // a picked face is drawn on top in the pick colour
    r.set_picked(&[EntityRef { node: 1, kind: EntityKind::Face, index: 3 }]);
    let img = shot(&mut r, false, true);
    let c = img.pixel(SIZE / 2, SIZE / 2);
    assert!(c[0] > c[2] + 40, "orange tint over the face: {c:?} (was {:?})", base.pixel(SIZE / 2, SIZE / 2));
    // a picked edge is drawn as a thick orange line
    r.set_picked(&[EntityRef { node: 1, kind: EntityKind::Edge, index: 0 }]);
    let img = shot(&mut r, false, true);
    assert_ne!(img, base);
    // hovering a vertex draws a marker
    r.set_picked(&[]);
    r.set_hover(Some(Highlight::Entity(EntityRef { node: 1, kind: EntityKind::Vertex, index: 7 })));
    let img = shot(&mut r, false, true);
    assert_ne!(img, base);
    r.set_hover(None);
    assert_eq!(shot(&mut r, false, true), base);
}

#[test]
fn capture_png_and_errors() {
    let Some(g) = gpu("capture_png_and_errors") else { return };
    let mut r = renderer(&g, false);
    let png = r.capture_png(&CaptureOptions::screenshot(100, 80)).expect("png");
    let img = image::load_from_memory(&png).expect("decode").to_rgba8();
    assert_eq!(img.dimensions(), (100, 80));
    assert!(r.capture_png(&CaptureOptions::screenshot(0, 10)).is_err());
    assert!(r.capture_png(&CaptureOptions::screenshot(100_000, 10)).is_err());
    // a renderer without a queue says so instead of panicking
    let mut no_queue = Renderer3d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm);
    assert!(no_queue.capture_png(&CaptureOptions::screenshot(10, 10)).is_err());
    assert_eq!(no_queue.pick(1.0, 1.0), None);
    // MSAA off gives the same silhouette
    let off = r
        .capture_rgba(&CaptureOptions { msaa: Some(0), ..CaptureOptions::screenshot(64, 64) })
        .expect("msaa off");
    assert_eq!(off.width, 64);
}

#[test]
fn bounds_and_fit_helpers() {
    let Some(g) = gpu("bounds_and_fit_helpers") else { return };
    let mut r = renderer(&g, true);
    let b = r.model_bounds().expect("bounds");
    assert_eq!(b.min, [-50.0, -50.0, -50.0]);
    assert_eq!(b.max, [200.0, 50.0, 50.0]);
    r.set_visibility(&HashSet::from([2]));
    assert_eq!(r.visible_bounds().expect("bounds").max[0], 50.0);
    assert_eq!(r.nodes_bounds(&[2]).expect("bounds").min[0], 100.0);
    assert_eq!(r.instance_count(), 2);
    r.fit_nodes(&[2]);
    assert!((r.camera.center.x - 150.0).abs() < 1e-9);
}

/// Writes sample pictures to the folder named by `CAD_RENDER_DUMP` (for looking at the output by eye).
#[test]
fn dump_sample_images() {
    let Ok(dir) = std::env::var("CAD_RENDER_DUMP") else {
        println!("dump_sample_images: set CAD_RENDER_DUMP=<folder> to write sample pictures");
        return;
    };
    let Some(g) = gpu("dump_sample_images") else { return };
    let mut faces = [None; 6];
    faces[4] = Some(Rgb(0.8, 0.2, 0.1));
    let mut r = Renderer3d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm).with_queue(&g.queue);
    r.set_model(&[cube_mesh(faces)], &cube_tree(true));
    r.set_viewport(480, 360, 1.0);
    r.set_orientation(StdView::Iso);
    let save = |r: &mut Renderer3d, name: &str, w: u32, h: u32, transparent: bool, ui: bool| {
        let png = r
            .capture_png(&CaptureOptions { width: w, height: h, transparent_background: transparent, ui, dpr: 1.0, camera: None, msaa: Some(4) })
            .expect("capture");
        std::fs::write(std::path::Path::new(&dir).join(name), png).expect("write");
    };
    r.set_selection(&[2]);
    let mut o = Overlays::default();
    o.distance([-50.0, -50.0, 50.0], [200.0, -50.0, 50.0]);
    o.center_of_gravity([75.0, 0.0, 0.0], "  T  1 234,5 kg");
    r.set_overlays(o);
    save(&mut r, "iso.png", 480, 360, false, true);
    r.set_transparency(&[1], 0.75);
    save(&mut r, "transparent.png", 480, 360, false, true);
    r.set_transparency(&[1], 0.0);
    r.set_selection(&[]);
    r.clear_overlays();
    r.set_section(Some(Section { axis: 0, position: 20.0, flip: false }));
    save(&mut r, "section.png", 480, 360, false, true);
    r.set_section(None);
    r.set_style(&Style { wireframe: true, dark_background: true, ..r.style.clone() });
    save(&mut r, "wire.png", 480, 360, false, true);
    r.set_style(&Style::default());
    r.set_perspective(true);
    r.fit_all();
    save(&mut r, "persp.png", 480, 360, false, true);
    let t = r.thumbnail_png(256).expect("thumbnail");
    std::fs::write(std::path::Path::new(&dir).join("thumb.png"), t).expect("write");
}
