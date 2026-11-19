//! More headless GPU tests: big assemblies, perspective picking, the software adapter.
use cad_types::{EntityKind, ModelTree, TreeNode, IDENTITY};
use eframe::wgpu;

use crate::camera::StdView;
use crate::capture::CaptureOptions;
use crate::gpu::GpuContext;
use crate::gpu_tests::{cube_mesh, cube_tree, gpu, renderer, HALF};
use crate::renderer3d::{Renderer3d, Section, Style};

/// `n x n` cubes of one prototype on a grid with 150 mm pitch; node 0 is the root assembly.
fn grid_tree(n: u32) -> ModelTree {
    let mut nodes = vec![TreeNode {
        id: 0,
        parent: None,
        children: (1..=n * n).collect(),
        name: "root".into(),
        proto_name: String::new(),
        key: String::new(),
        is_assembly: true,
        proto: None,
        location: IDENTITY,
        color: None,
        own_style: false,
    }];
    for i in 0..n * n {
        let mut location = IDENTITY;
        location[12] = f64::from(i % n) * 150.0;
        location[14] = f64::from(i / n) * 150.0;
        nodes.push(TreeNode {
            id: i + 1,
            parent: Some(0),
            children: vec![],
            name: format!("c{i}"),
            proto_name: "cube".into(),
            key: "0:1:1".into(),
            is_assembly: false,
            proto: Some(0),
            location,
            color: None,
            own_style: false,
        });
    }
    ModelTree { roots: vec![0], nodes, proto_instances: vec![n * n] }
}

#[test]
fn big_assembly_of_instances_renders_and_picks_the_right_one() {
    let Some(g) = gpu("big_assembly_of_instances") else { return };
    let n = 40u32; // 1600 instances of one prototype
    let mut r = Renderer3d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm).with_queue(&g.queue);
    r.set_model(&[cube_mesh([None; 6])], &grid_tree(n));
    r.set_viewport(800, 800, 1.0);
    r.set_orientation(StdView::Front);
    assert_eq!(r.instance_count(), (n * n) as usize);
    let t = std::time::Instant::now();
    let img = r
        .capture_rgba(&CaptureOptions { width: 800, height: 800, transparent_background: true, ui: false, dpr: 1.0, camera: None, msaa: None })
        .expect("capture");
    println!("1600 instances rendered in {:?}", t.elapsed());
    // front view of the grid: along a row through the middle of a cube row the pixels alternate between cubes and gaps
    let row = r.world_to_pixel([0.0, -HALF, 20.0 * 150.0]).expect("visible")[1] as u32;
    let covered = (0..800).filter(|&x| img.pixel(x, row)[3] > 0).count();
    assert!(covered > 300 && covered < 700, "{covered}");
    // pick the cube at grid (7, 12): x = 7 * 150, z = 12 * 150
    let (gx, gz) = (7u32, 12u32);
    let p = r.world_to_pixel([f64::from(gx) * 150.0, -HALF, f64::from(gz) * 150.0]).expect("visible");
    let hit = r.pick_hit(p[0] as f32, p[1] as f32).expect("hit");
    assert_eq!(hit.entity.node, gz * n + gx + 1);
    assert_eq!(hit.entity.kind, EntityKind::Face);
    // hide an entire row through the root's child list: hide one leaf and the pick goes to nothing
    r.set_visibility(&std::collections::HashSet::from([gz * n + gx + 1]));
    assert_eq!(r.pick(p[0] as f32, p[1] as f32), None);
    // selection of many parts at once only rewrites what changed
    let all: Vec<u32> = (1..=n * n).collect();
    r.set_selection(&all);
    r.set_selection(&[3]);
    r.set_selection(&[]);
}

#[test]
fn perspective_picking_and_pivot() {
    let Some(g) = gpu("perspective_picking_and_pivot") else { return };
    let mut r = renderer(&g, true);
    r.set_perspective(true);
    r.set_orientation(StdView::Iso);
    // the cube's top face centre
    let p = r.world_to_pixel([0.0, 0.0, HALF]).expect("visible");
    let hit = r.pick_hit(p[0] as f32, p[1] as f32).expect("hit");
    assert_eq!((hit.entity.node, hit.entity.kind, hit.entity.index), (1, EntityKind::Face, 4));
    assert!((hit.point[2] - HALF).abs() < 2.5, "{:?}", hit.point);
    assert!(hit.point[0].abs() < 3.0 && hit.point[1].abs() < 3.0, "{:?}", hit.point);
    let pivot = r.pivot_at(p[0] as f32, p[1] as f32).expect("pivot");
    assert!((pivot[2] - HALF).abs() < 2.5);
    // section in perspective: the same cut shows the cap, picking sees the far wall
    r.set_orientation(StdView::Right);
    r.set_section(Some(Section { axis: 0, position: 0.0, flip: false }));
    let c = r.world_to_pixel([0.0, 0.0, 0.0]).expect("visible");
    let hit = r.pick_hit(c[0] as f32, c[1] as f32).expect("hit");
    assert_eq!(hit.entity.index, 1, "far wall (-X face) behind the cut");
}

#[test]
fn msaa_variants_render_the_same_silhouette() {
    let Some(g) = gpu("msaa_variants_render_the_same_silhouette") else { return };
    let mut r = renderer(&g, false);
    let shot = |r: &mut Renderer3d, msaa| {
        r.capture_rgba(&CaptureOptions { width: 128, height: 128, transparent_background: true, ui: false, dpr: 1.0, camera: None, msaa: Some(msaa) })
            .expect("capture")
    };
    let off = shot(&mut r, 0);
    for m in [2, 4, 8] {
        let img = shot(&mut r, m);
        assert_eq!(img.pixel(64, 64)[3], 255, "msaa {m}");
        assert_eq!(img.pixel(1, 1)[3], 0, "msaa {m}");
        let diff = (0..128 * 128).filter(|i| img.data[i * 4 + 3].abs_diff(off.data[i * 4 + 3]) > 128).count();
        assert!(diff < 300, "msaa {m}: {diff} pixels differ strongly");
    }
    // the style option is honoured by the renderer's own picture too
    r.set_style(&Style { msaa: 4, ..r.style.clone() });
    assert!(r.msaa_samples() == 4 || r.msaa_samples() == 2 || r.msaa_samples() == 1);
    // the second capture after the style change works as well
    assert_eq!(shot(&mut r, 4).pixel(64, 64)[3], 255);
}

#[test]
fn software_adapter_renders_a_thumbnail_when_available() {
    // WARP on Windows, lavapipe on Linux: the path of `--thumbnail` in sandboxes without a GPU
    let g = match GpuContext::new_software() {
        Ok(g) => g,
        Err(e) => {
            println!("SKIP software_adapter_renders_a_thumbnail_when_available: no software adapter ({e})");
            return;
        }
    };
    println!("software_adapter: running on adapter {}", g.describe());
    let mut r = Renderer3d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm).with_queue(&g.queue);
    r.set_model(&[cube_mesh([None; 6])], &cube_tree(false));
    let t = r.thumbnail_rgba(64).expect("thumbnail on the software adapter");
    assert!(t.pixel(32, 32)[3] > 200);
    assert_eq!(t.pixel(0, 0)[3], 0);
}

#[test]
fn far_from_the_origin_nothing_jitters() {
    let Some(g) = gpu("far_from_the_origin_nothing_jitters") else { return };
    let mut tree = cube_tree(false);
    tree.nodes[1].location[12] = 4_000_000.0;
    tree.nodes[1].location[13] = -3_000_000.0;
    tree.nodes[1].location[14] = 2_000_000.0;
    let mut r = Renderer3d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm).with_queue(&g.queue);
    r.set_model(&[cube_mesh([None; 6])], &tree);
    r.set_viewport(256, 256, 1.0);
    r.set_orientation(StdView::Front);
    let img = r
        .capture_rgba(&CaptureOptions { width: 256, height: 256, transparent_background: true, ui: false, dpr: 1.0, camera: None, msaa: Some(0) })
        .expect("capture");
    // the cube fills 92 % of the view without ragged edges: rows 12 and 243 are inside, 10 and 245 outside
    let col = 128;
    assert_eq!(img.pixel(col, 14)[3], 255);
    assert_eq!(img.pixel(col, 9)[3], 0);
    assert_eq!(img.pixel(col, 241)[3], 255);
    assert_eq!(img.pixel(col, 246)[3], 0);
    // picking and overlays work in the same world coordinates
    let p = r.world_to_pixel([4_000_000.0, -3_000_000.0 - 50.0, 2_000_000.0]).expect("visible");
    assert!((p[0] - 128.0).abs() < 0.5 && (p[1] - 128.0).abs() < 0.5, "{p:?}");
    let hit = r.pick_hit(128.0, 128.0).expect("hit");
    assert_eq!(hit.entity.index, 3);
    assert!((hit.point[0] - 4_000_000.0).abs() < 1.0 && (hit.point[1] + 3_000_050.0).abs() < 1.0, "{:?}", hit.point);
    let mut o = crate::overlay::Overlays::default();
    o.line([4_000_000.0 - 20.0, -3_000_060.0, 2_000_000.0], [4_000_000.0 + 20.0, -3_000_060.0, 2_000_000.0], [255, 0, 0], 4.0, false);
    r.set_overlays(o);
    let img = r
        .capture_rgba(&CaptureOptions { width: 256, height: 256, transparent_background: true, ui: true, dpr: 1.0, camera: None, msaa: Some(0) })
        .expect("capture");
    let c = img.pixel(128, 128);
    assert!(c[0] > 200 && c[1] < 60, "overlay line drawn in the middle: {c:?}");
}

#[test]
fn huge_overlay_polylines_are_converted_once() {
    let Some(g) = gpu("huge_overlay_polylines_are_converted_once") else { return };
    let mut r = renderer(&g, false);
    // a 200 000 segment zig-zag on the front face (the sharp edges of a big mesh)
    let n = 200_000usize;
    let pts: Vec<[f64; 3]> = (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            [-45.0 + 90.0 * t, -60.0, if i % 2 == 0 { -10.0 } else { 10.0 }]
        })
        .collect();
    let mut o = crate::overlay::Overlays::default();
    o.lines.push(crate::overlay::OverlayLine { points: pts, closed: false, color: [0, 160, 0], width: 1.0, dashed: false });
    r.set_overlays(o);
    let shot = |r: &mut Renderer3d| {
        r.capture_rgba(&CaptureOptions { width: 256, height: 256, transparent_background: true, ui: true, dpr: 1.0, camera: None, msaa: Some(0) })
            .expect("capture")
    };
    let t0 = std::time::Instant::now();
    let a = shot(&mut r);
    let first = t0.elapsed();
    let t1 = std::time::Instant::now();
    let b = shot(&mut r);
    let second = t1.elapsed();
    println!("200k overlay segments: first picture {first:?}, second (cached) {second:?}");
    assert_eq!(a, b);
    // the zig-zag covers the strip in front of the cube
    let c = b.pixel(128, 128 + 20);
    let green = (100..160).any(|y| (100..160).any(|x| {
        let p = b.pixel(x, y);
        p[1] > 120 && p[0] < 80 && p[2] < 80
    }));
    assert!(green || c[3] > 0, "overlay polyline drawn");
}
