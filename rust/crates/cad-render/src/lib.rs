//! wgpu renderer of the CAD viewer (replaces OCCT V3d): see docs/rust-port/ARCHITECTURE.md.
//!
//! * [`Renderer3d`] draws the model (instanced meshes, edges, section, overlays, view cube, scale bar), picks entities
//!   with an id buffer and takes screenshots; [`Renderer3d::paint_callback`] integrates it into an egui view.
//! * [`Camera`] holds the camera maths as plain functions.
//! * [`Renderer2d`] draws `Scene2d` batches with layer visibility, pan and zoom.
//!
//! # Using it from the egui application
//!
//! ```ignore
//! // once, e.g. in the eframe creation context
//! let rs = cc.wgpu_render_state.as_ref().expect("wgpu backend");
//! let view3d = Renderer3d::from_render_state(rs).shared();      // Arc<Mutex<Renderer3d>>
//!
//! // on loading: upload the model, then frame it
//! { let mut r = view3d.lock().unwrap(); r.set_model(&protos, &tree); r.fit_all(); }
//!
//! // every frame, in the central panel
//! let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
//! ui.painter().add(Renderer3d::paint_callback(&view3d, rect));
//! // input: positions are physical pixels relative to `rect.min` (multiply by `ui.ctx().pixels_per_point()`);
//! // r.camera.orbit(..), r.camera.pan_pixels(..), r.camera.zoom_at(..), r.pick(x, y), r.view_cube_hit(x, y), ...
//! ```
//!
//! The picture is rendered offscreen in the callback's `prepare` step and copied into egui's pass in `paint`, so the
//! renderer does not depend on the depth buffer or MSAA setup of the egui pass (pass them to
//! [`Renderer3d::with_target`] when they are not the defaults). Everything is shaded in gamma space and the same code
//! serves `--thumbnail` through [`GpuContext`] and [`Renderer3d::thumbnail_png`].
pub use eframe::wgpu;

pub mod camera;
pub mod capture;
pub mod colors;
mod frame;
pub mod gpu;
#[cfg(test)]
mod gpu_tests;
#[cfg(test)]
mod gpu_tests2d;
#[cfg(test)]
mod gpu_tests_chunks;
#[cfg(test)]
mod gpu_tests_egui;
#[cfg(test)]
mod gpu_tests_hl;
#[cfg(test)]
mod gpu_tests_more;
pub mod gpubuf;
mod gpu_types;
pub mod interaction;
pub mod overlay;
pub mod overlay2d;
pub mod pick;
mod pickpass;
mod pipelines;
pub mod renderer2d;
pub mod renderer3d;
pub mod scalebar;
pub mod scene3d;
pub mod text;
pub mod view2d;
pub mod viewcube;

pub use camera::{Camera, CameraAnimation, StdView, Viewport};
pub use capture::{CaptureOptions, Rgba8};
pub use gpu::{GpuContext, RenderError};
pub use overlay::{OverlayLabel, OverlayLine, OverlayMesh, OverlayPoint, Overlays};
pub use overlay2d::{Dot2d, Line2d, Overlays2d, Square2d, Text2d, Tris2d};
pub use pick::PickFilter;
pub use renderer2d::{LineWidthMode, Renderer2d, Renderer2dCallback};
pub use view2d::View2d;
pub use renderer3d::{Highlight, PickHit, Renderer3d, Renderer3dCallback, Section, Style, TargetInfo};
