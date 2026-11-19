//! Hand-over of a drawing scene to the 2D renderer.
//!
//! Scenes are built relative to the (rounded) centre of the drawing (`OriginMode::Center`), so f32 batch coordinates
//! stay exact for drawings far from the origin (survey drawings in S-JTSK have coordinates around -700 000,
//! -1 000 000). `Scene2d::origin` is the point the batch coordinates are relative to. The renderer's view, its overlays,
//! picking, snapping and everything the user sees are in drawing (world) coordinates; only the batches of a highlight
//! come back relative to the origin ([`SceneSpace::batch_to_world`]).
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use cad_2d::{Drawing, DrawingTooComplex, OriginMode, SceneError, SceneOptions};
use cad_types::{LayerId, Scene2d};

/// The origin of the batch coordinates of the scene that is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SceneSpace {
    pub origin: [f64; 2],
}

impl SceneSpace {
    /// A point of a batch as a drawing coordinate (the highlight is in the renderer, in the coordinates of its scene, so the
    /// window itself no longer needs this).
    #[cfg(test)]
    pub fn batch_to_world(&self, p: [f32; 2]) -> [f64; 2] {
        [f64::from(p[0]) + self.origin[0], f64::from(p[1]) + self.origin[1]]
    }
}

/// Scene of a layout for the renderer: batches relative to the drawing centre, and only the batches and pictures whose
/// layers are shown (a batch depends on every layer its primitives were drawn for; the renderer knows only one layer per
/// batch).
pub fn build(
    drawing: &Drawing,
    layout: &str,
    light_bg: bool,
    images: bool,
) -> Result<(Scene2d, SceneSpace), DrawingTooComplex> {
    // handed over, not copied: a thumbnail of a big drawing does not hold its scene twice
    let never = Arc::new(AtomicBool::new(false));
    let mut scene = match drawing.scene_cancellable(layout, &SceneOptions { light_bg, origin: OriginMode::Center, images }, &never) {
        Ok(s) => s,
        Err(SceneError::TooComplex(e)) => return Err(e),
        // the flag is never set, so this does not happen; the message is the one of a cancelled build
        Err(SceneError::Cancelled) => return Err(DrawingTooComplex(SceneError::Cancelled.to_string())),
    };
    let space = SceneSpace { origin: scene.origin };
    filter_hidden(&mut scene, &drawing.visible_batches(), |l| drawing.layer_visible(l));
    Ok((scene, space))
}

/// The scene for the window: every batch, hidden layers included, handed over by the drawing (it keeps no copy), relative to
/// the drawing centre. Which batches are drawn is a mask (`Drawing::visible_batches`, `Renderer2d::set_batch_mask`), so a
/// layer that is switched on or off is no new scene. Stops as soon as `cancel` is set (a newer scene was asked for,
/// another file is being opened): the result is then `SceneError::Cancelled` and nothing is kept. Looked at between
/// entities and in the long loops of the scene build, so the thread is free within a fraction of a second even for a
/// drawing that takes minutes.
pub fn build_view(
    drawing: &Drawing,
    layout: &str,
    light_bg: bool,
    images: bool,
    cancel: &Arc<AtomicBool>,
) -> Result<(Scene2d, SceneSpace), SceneError> {
    let scene = drawing.scene_cancellable(layout, &SceneOptions { light_bg, origin: OriginMode::Center, images }, cancel)?;
    let space = SceneSpace { origin: scene.origin };
    Ok((scene, space))
}

/// Keeps the batches flagged as shown and the pictures of visible layers, and marks every layer visible (the renderer
/// then draws what is left). A picture is drawn after the first `after` batches: dropping batches moves it along.
pub fn filter_hidden(scene: &mut Scene2d, shown: &[bool], layer_visible: impl Fn(LayerId) -> bool) {
    if shown.len() == scene.batches.len() {
        // kept batches before each position, for the z-order of the pictures
        let mut kept_before = Vec::with_capacity(shown.len() + 1);
        let mut n = 0u32;
        kept_before.push(0);
        for s in shown {
            n += u32::from(*s);
            kept_before.push(n);
        }
        for img in scene.images.iter_mut() {
            let at = (img.after as usize).min(shown.len());
            img.after = kept_before[at];
        }
        let mut i = 0;
        scene.batches.retain(|_| {
            let keep = shown[i];
            i += 1;
            keep
        });
    }
    scene.images.retain(|img| layer_visible(img.layer));
    for l in scene.layers.iter_mut() {
        l.visible = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_types::DrawBatch;

    #[test]
    fn batch_points_plus_origin_are_world_coordinates() {
        let s = SceneSpace { origin: [-700_000.0, -1_000_000.0] };
        assert_eq!(s.batch_to_world([10.5, -20.25]), [-699_989.5, -1_000_020.25]);
    }

    #[test]
    fn hidden_batches_are_dropped_and_layers_marked_visible() {
        let mut sc = Scene2d::default();
        sc.batches = (0..4).map(|i| DrawBatch { layer: i, ..DrawBatch::default() }).collect();
        sc.layers = vec![cad_types::Layer { id: 0, visible: false, ..Default::default() }];
        filter_hidden(&mut sc, &[true, false, true, false], |_| true);
        assert_eq!(sc.batches.iter().map(|b| b.layer).collect::<Vec<_>>(), vec![0, 2]);
        assert!(sc.layers[0].visible);
        // a list of the wrong length is ignored instead of dropping everything
        let mut sc2 = Scene2d::default();
        sc2.batches = vec![DrawBatch::default(); 3];
        filter_hidden(&mut sc2, &[true], |_| true);
        assert_eq!(sc2.batches.len(), 3);
    }

    fn picture(layer: LayerId, after: u32) -> cad_types::DrawImage {
        cad_types::DrawImage {
            layer,
            after,
            width: 1,
            height: 1,
            rgba: std::sync::Arc::from(vec![0u8, 0, 0, 255]),
            corners: [[0.0; 2]; 4],
        }
    }

    #[test]
    fn pictures_keep_their_place_between_the_batches_and_follow_their_layer() {
        let mut sc = Scene2d::default();
        sc.batches = vec![DrawBatch::default(); 4];
        // drawn after batch 0, after batch 2 and on top of everything
        sc.images = vec![picture(1, 1), picture(2, 3), picture(1, 4)];
        // batches 0 and 2 stay, 1 and 3 go; layer 2 is hidden
        filter_hidden(&mut sc, &[true, false, true, false], |l| l != 2);
        assert_eq!(sc.batches.len(), 2);
        assert_eq!(sc.images.iter().map(|i| (i.layer, i.after)).collect::<Vec<_>>(), vec![(1, 1), (1, 2)]);
    }
}

#[cfg(test)]
mod drawing_tests {
    use super::*;

    #[test]
    fn the_scene_of_the_window_has_every_batch_and_the_mask_says_which_are_drawn_and_it_stops_when_asked() {
        let d = drawing();
        let off = Arc::new(AtomicBool::new(false));
        let (a, sa) = build(&d, "Model", false, false).expect("scene");
        let (b, sb) = build_view(&d, "Model", false, false, &off).expect("scene");
        // the thumbnail scene has the batches of the shown layers only, the window's scene all of them and the mask
        let mask = d.visible_batches();
        assert_eq!(mask.len(), b.batches.len());
        assert_eq!(mask.iter().filter(|m| **m).count(), a.batches.len());
        assert!(b.batches.len() > a.batches.len(), "the layer that is off in the file has batches");
        assert_eq!((a.bounds, sa), (b.bounds, sb));
        // the layers of the scene say which are off (the renderer takes pictures from them)
        assert!(b.layers.iter().any(|l| l.name == "Skrytá" && !l.visible));
        // a flag that is set: nothing is built (the scene was handed over, the drawing builds it again)
        let on = Arc::new(AtomicBool::new(true));
        assert!(matches!(build_view(&drawing(), "Model", false, false, &on), Err(cad_2d::SceneError::Cancelled)));
    }

    fn drawing() -> Drawing {
        cad_2d::load_bytes(crate::testgen::dxf_drawing().as_bytes(), cad_2d::FontOptions { system_fonts: false, dirs: Vec::new() })
            .expect("drawing")
    }

    /// Batch coordinates plus the origin are drawing coordinates; the bounds are world coordinates.
    #[test]
    fn scene_is_relative_to_the_drawing_centre() {
        let d = drawing();
        let (scene, space) = build(&d, "Model", false, false).expect("scene");
        assert!(space.origin[0].abs() > 1.0 && space.origin[1].abs() > 1.0, "origin {:?}", space.origin);
        assert_eq!(space.origin, scene.origin);
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for b in &scene.batches {
            for p in b.lines.iter().chain(&b.fills) {
                let w = space.batch_to_world(*p);
                for k in 0..2 {
                    lo[k] = lo[k].min(w[k]);
                    hi[k] = hi[k].max(w[k]);
                }
            }
        }
        // the plate is 200 x 120 at the origin, the dimension line is 20 below it, the axes stick out by 10
        assert!(lo[0] <= 0.0 && lo[0] > -15.0, "{lo:?}");
        assert!(hi[0] >= 200.0 && hi[0] < 215.0, "{hi:?}");
        assert!(lo[1] <= -20.0 && lo[1] > -40.0, "{lo:?}");
        let (b0, b1) = scene.bounds.expect("bounds");
        assert!(b0[0] <= 0.0 && b1[0] >= 200.0, "bounds are world coordinates: {b0:?} {b1:?}");
        // the layer that is off in the file draws nothing
        assert!(scene.layers.iter().any(|l| l.name == "Skrytá" && l.off_in_file));
    }
}

#[cfg(test)]
mod renderer_tests {
    use super::*;
    use cad_render::{wgpu, GpuContext, Renderer2d};

    #[test]
    fn fit_centres_the_view_on_the_drawing_in_world_coordinates() {
        let Ok(g) = GpuContext::new_headless() else {
            eprintln!("SKIP: no graphics adapter");
            return;
        };
        let d = cad_2d::load_bytes(crate::testgen::dxf_drawing().as_bytes(), cad_2d::FontOptions { system_fonts: false, dirs: Vec::new() }).unwrap();
        let (scene, _space) = build(&d, "Model", false, false).unwrap();
        let mut r = Renderer2d::new(&g.device, wgpu::TextureFormat::Rgba8Unorm).with_queue(&g.queue);
        r.set_viewport(800, 600, 1.0);
        r.set_scene(&scene);
        let c = r.view.center;
        // the drawing spans x -10..210 and y -20..130
        assert!((c[0] - 100.0).abs() < 5.0 && (c[1] - 55.0).abs() < 8.0, "view centre in world coordinates: {c:?}");
    }
}
