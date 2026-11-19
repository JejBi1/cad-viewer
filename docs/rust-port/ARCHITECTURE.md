# CAD prohlížeč: Rust rewrite

Behaviour reference: `docs/rust-port/SPEC.md` and the Python sources in `stepview/`. Czech UI strings, constants, limits and
file formats are preserved exactly unless this file says otherwise. `stepview/`, `shellext/` and `native/` are not modified.

## Decisions
- GUI: egui/eframe with the wgpu backend. 3D and 2D are drawn by our own wgpu renderer.
- OCCT 8.0.1 stays as the geometry kernel, reached only through `occt-bridge` (cxx + a small C++ file).
- Explorer thumbnails: the C# shell extension stays and keeps calling `StepViewer.exe --thumbnail <src> <png> <size>`.
- DWG: `third_party/libredwg/dwg2dxf.exe` converts to DXF as today.
- Version 2.0.0. The cache key's version suffix becomes `2.0`.

## Portable variant (Google Drive)
A second distribution runs from a folder synced by Google Drive for Desktop, with no installer, registry entries or admin rights.
- Portable mode is on when a file `portable.flag` lies next to the executable. All path decisions go through `cad_core::paths`.
- The application folder is treated as read-only: settings, cache, logs, worker job files and temp files always stay in the local per-user folders, never in the synced folder.
- Models opened from the Drive may be online-only or change during reading: the source is copied to the local cache folder before loading, and the cache key still uses the original path, size and mtime.
- Portable mode never registers file associations or Explorer thumbnails.
- Outside portable mode too, a 3D model whose path is 260 characters or more (Open CASCADE reads through the old path interface of Windows) is loaded through the same local copy in `cache\sources`, in the window and for `--thumbnail` (a private short copy in the temp folder there); the tree and the title keep the name of the file that was opened. Leftover `dwg_in_*`, `oda_*`, `dwg_*` and `thumb_in_*` folders of a killed program in the temp and cache folders are removed at the start when older than 6 hours.
- PLM/PLD scope: view-only access to models and drawings stored on the Drive. The application never writes to the source files or next to them, and there is no PLM system integration or revision metadata.

## Google Drive launcher (all Rust)
Drive's "Open with" starts the locally installed viewer. There is no web viewer and no server-side code.
- `cad-drive-web` (Rust compiled to WebAssembly, static HTTPS page): reads the Drive `state` query parameter, validates it, and redirects to `cadviewer://open?fileId=<id>`. It shows a Czech help page with a download link when the app does not start.
- `cad-drive` (native library used by `cad-app`): parses the link, signs in with OAuth 2.0 for desktop apps (system browser, loopback redirect, PKCE), downloads the file through the Drive API into the local cache, and returns the local path for the normal loading path.
- Scope is `drive.file` only. Access is read-only in behaviour: the app only calls metadata get and media download. Tokens are stored in the OS credential store, never in files or logs.
- Link input is untrusted: only `open` with one `fileId` matching `^[A-Za-z0-9_-]{10,128}$` is accepted. Paths, URLs and any other parameter are rejected. File names from Drive are never used as paths; the extension must be one of the supported CAD types.
- `StepViewer.exe --register-link` and `--unregister-link` add or remove the `cadviewer` scheme under HKCU only, on explicit user action (also in portable mode). `--open-link <url>` is the registered command.
- The OAuth client id and the launcher page URL are build-time configuration; none are committed.

## Size budget
The smallest possible distribution is a release goal. Baseline 1.9: Windows installer 55.9 MB, Linux AppImage 100.3 MB.
- OCCT is built from source as static libraries with only the required toolkits, without FreeImage, FreeType-dependent visualisation or other optional third-party libraries, and linked into the executable.
- wgpu backends: DX12 on Windows, Vulkan on Linux, nothing else. No image codecs beyond those a feature needs. No extra fonts beyond the Czech-capable default.
- Release profile: fat LTO, one codegen unit, `strip`, size-optimised (`opt-level = "z"` or `"s"`) except measured hot crates. Every release reports the size per component.
- No executable packers such as UPX: they trigger antivirus false positives.
- Decided by the user (2026-11-19): the release ships the DWG converter in the base installer and portable zip (`cargo xtask dist`, the full variant), and keeps the embedded fonts. `--no-dwg` stays available in the tooling but is not the release.
- Hot crates (cad-2d, cad-render, cad-core) are built with `opt-level = 3` in the `dist` profile: 1.2–2× faster on large drawings for +0.7 MB raw, +0.22 MB in the installer.

## Crates (`rust/crates`)
| Crate | Owns | Python origin | May depend on |
|---|---|---|---|
| `cad-types` | shared data types | none | serde |
| `occt-bridge` | STEP/IGES/STL import, meshing, XBF, tree, mesh extraction, mass, measurement, booleans | core, analysis (OCCT part) | cad-types, cxx |
| `cad-core` | cache, worker protocol, settings, materials, number formatting, STL mesh analysis | main (non-UI), materials, meshmeasure | cad-types |
| `cad-2d` | DXF parsing, DWG conversion, scene building, level assignment | view2d (non-UI), fastscene, qtbuild, rdxf | cad-types |
| `cad-render` | wgpu renderer, camera, picking, section plane, overlays, offscreen capture | viewer3d, 2D painting | cad-types, eframe |
| `cad-app` | `StepViewer` binary: egui UI, process roles, thumbnails, 3D mouse, thumbnail settings | main (UI), collisions, thumbs, thumbsettings, spacemouse | everything |

## Crate interfaces
Implement these names; add more when needed, but do not change these without updating this file.

`occt-bridge`
- `Document::read(path: &Path, report: &mut dyn FnMut(&str)) -> Result<Document, BridgeError>`
- `Document::save_xbf(&self, path: &Path)`, `Document::load_xbf(path: &Path) -> Result<Document, BridgeError>`
- `Document::tree(&self) -> Result<ModelTree, BridgeError>`; `BridgeError::TooComplex(String)` carries the exact Czech message
- `Document::proto_count(&self) -> u32`, `Document::proto_mesh(&self, ProtoId) -> ProtoMesh`
- `Document::mass_props(&self, ProtoId) -> MassProps`, `Document::node_bounds(&self, NodeId) -> Option<Aabb>`
- `Document::measure(&self, &[EntityRef]) -> Measurement`, `Document::interferences(&self, &[NodeId], progress) -> Vec<Interference>`

`cad-core`
- `cache::{cache_dir, cache_path, part_path, prune, clear, store}`, `worker::{Job, run_worker, serve_worker, spawn, submit, job_valid}`
- `settings::Settings` (load, save, typed keys from SPEC section 14), `materials::Library`, `fmt::{fmt_len, fmt_num}`, `meshmeasure::Mesh`

`cad-2d`
- `load(path: &Path, opts: &LoadOptions, progress: &mut dyn FnMut(&str)) -> Result<Drawing, LoadError>`
- `Drawing::scene(&self, layout: &str) -> Scene2d`, `Drawing::info(&self) -> DrawingInfo`, `Drawing::pick(&self, point: [f64; 2], tolerance: f64) -> Option<EntityInfo>`, `Drawing::snap_points(...)`

- `Scene2d` points are relative to `Scene2d.origin`; bounds stay in world coordinates. `Scene2d.images` holds decoded rasters (`DrawImage.after` = index of the batch the picture is drawn before). `DrawBatch.width` is a cosmetic pen width in screen pixels, 0 = hairline; a line pair with two equal points is a dot.

- `load_cancellable` and `Drawing::scene_cancellable` take the cancel flag of the load; `SceneError`, `scene_notes` carry the Czech notes about reduced detail. Limits: INSERT expansion 300 000 as in 1.9; DIMENSION / ACAD_TABLE / MLEADER content 6 000 000 (cycle and depth checked); 30 s per HATCH, 120 s per scene stage; output vertices as the last-resort cap: at most 64 M (2.9 GB, what an 8 GB computer can spare) and what half of the free physical memory holds (`cad_2d::memory`), detail reduced from half of it. Detail levels for filled paths only (text, fills): full, simplified, box (a bar per text line, the outline of other fills); stroked lines and curves always keep full quality. A scene is refused only when strokes plus boxes do not fit; such a refusal is not cached. Strokes are counted exactly (flattened once at the drawing tolerance before planning), and an overflow retries with fills reduced, up to 4 passes. The thumbnail role uses small fixed limits (soft 2 M, max 16 M vertices) capped by the job memory headroom, so it fits the 2 GiB Explorer sandbox. Warning rows come first in the drawing panel and the status bar says when a drawing was simplified.
- The DWG converter is found only in `<exe dir>\third_party\libredwg`, runs on a private ASCII copy of the input and in a kill-on-close job object; only the last 16 KiB of its stderr are kept and the job is closed before the reader is joined.
- `LoadOptions::original_dir`: the folder of the original drawing, so relative raster IMAGE files still resolve when a local copy is loaded (portable mode); the remote-path rules apply unchanged.
- Private temporary folders are named `StepViewer-<kind>-<random>` and carry a `.stepviewer-temp` marker. The window removes marked folders older than 6 h from TEMP, the cache folder and `LocalLow\StepViewerThumbs`; the legacy `dwg_` names only in the cache folder.

`cad-core` (additions)
- Local source copies live in `cache\sources\<key>\<original name>`; Drive downloads in `cache\drive`; both count toward the 3 GiB prune, which never follows links.
- 1.9 settings are imported once, read-only, from `HKCU\Software\CadProhlizec\StepViewer` when no settings file exists (not in portable mode). An unparsable settings file is kept as `settings.json.bad`.

`cad-render`
- GPU vertex data is split into buffers of at most 128 MiB (`gpubuf`); an allocation failure becomes a Czech warning through `take_warning()`, never a panic.
- `Renderer3d::new(device, format)`, `set_model(&[ProtoMesh], &ModelTree)`, `set_visibility`, `set_style`, `set_selection`, `set_section`, `set_overlays`, `paint(...)`, `pick(x, y) -> Option<EntityRef>`
- `Camera` (orbit, pan, zoom at cursor, fit, standard views, ortho/perspective), `Renderer2d::{set_scene, set_scene_owned, set_batch_mask, set_palette, set_highlight, set_layer_visible, paint}`, `capture_png(...)` for screenshots and headless thumbnails
- 2D scene lifecycle: `Drawing::scene_cancellable` hands the batches over (the drawing keeps one build for `pick`/`highlight`/`snap`, not the geometry); `Renderer2d::set_scene_owned` consumes them while uploading. Layers are a batch mask (`Drawing::visible_batches`), the white background option a palette (`Drawing::batch_roles`, `layout_foreground`, `layout_background`): neither makes a new scene (unless a picture was blended with the background, `colors_follow_background`). Paper space is always white with black pens. The highlight of a pick is made once (`set_highlight`, at most `scene::HIGHLIGHT_MAX_POINTS` outline points) and drawn from its own buffers.
- Every buffer made per frame (`DynBuf`) and per pick is made under error scopes: a refusal by the device is `take_warning()` (Czech), never a panic.

## Build
PowerShell, from the repository root: `. .\rust\tools\devenv.ps1 <suffix>` then `cargo check --manifest-path rust\Cargo.toml -p <crate>`. Give each parallel worker its own suffix. Never put the target folder inside SynologyDrive.

## Rules for workers
- Edit only the crate you own plus its `Cargo.toml`. Need a shared-type change? Report it in your final message instead of editing `cad-types`.
- Every ported algorithm carries its constants verbatim and has unit tests. No `unwrap` on file or user data. No stubs or `todo!()` in finished work; list anything unfinished in the final report.
- Do not commit, do not touch git state.
