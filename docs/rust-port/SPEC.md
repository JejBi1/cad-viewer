# STEP_viewer porting spec (CAD prohlížeč 1.9)

This is a read-only pass. I read every `.py` file in `stepview/` in full, plus `native/rdxf/src/lib.rs`, `shellext/StepThumb.cs`, `linux/install.sh` and `installer/StepViewer.iss`. I also read `shellext/Sandbox.cs`, because StepThumb.cs delegates process launch to it. Nothing was executed. Line numbers refer to the files on disk. Czech strings are quoted exactly.

## 0. Cross-cutting facts

- App name `APP_NAME = "CAD prohlížeč"`, `VERSION = "1.9"` (`/.../stepview/main.py:15-16`). Windows executable `StepViewer.exe` (`installer/StepViewer.iss:7`); Linux `cad-viewer` (`linux/install.sh:67`).
- One code base runs in three process roles: the GUI (`main.py:main`, line 2313), a load worker (`--worker-serve`, one job per process), and a thumbnail renderer (`--thumbnail`). See section 5.
- Number formatting is used everywhere: space as thousands separator, comma as decimal separator (`analysis.py:28-29`, `main.py:315-321`, `view2d.py:522-523`, `meshmeasure.py:24-25`). Keep it.
- Units: mm, mm³, mm², cm³ (display), g/cm³ (materials), kg.

---

## 1. `stepview/core.py` (415 lines): STEP/IGES/STL loading, meshing, XBF, tree

**1. Purpose and public API**
- `application() -> XCAFApp_Application` (line 40). Singleton. Defines format `"BinXCAF"`, description `"Binary XCAF"`, extension `xbf` (lines 43-48).
- `new_doc() -> TDocStd_Document` (52).
- `box_limits(box) -> (x0,y0,z0,x1,y1,z1)` (58).
- `relax_shapefix(reader)` (72). Sets FixShape parameters on the reader. Only valid after `ReadFile` and before `Transfer`.
- `read_cad(path, report=lambda msg: None) -> TDocStd_Document` (81). Dispatches on extension: `.stl` goes to `read_stl`; `.iges/.igs` uses `IGESCAFControl_Reader`; everything else uses `STEPCAFControl_Reader`. Runs `count_instances`, then `mesh_doc`.
- `read_stl(path, doc, report)` (118).
- `is_mesh_only(shape) -> bool` (132). True when the first face has no surface.
- `face_count(shapes) -> int` (143).
- `_mesh_params(defl, delabella)` (150), `_unmeshed_faces(shape)` (161).
- `mesh_doc(doc, report)` (174).
- `save_xbf(doc, path)` (225), `load_xbf(path) -> doc` (231).
- `length_unit(doc)` (240). Defined but never called (dead code).
- `label_name(lab)` (251), `label_color(ctool, lab)` (258), `entry(lab)` (266).
- `class Node` (272). `__slots__`: idx, parent, label, proto, key, name, proto_name, is_asm, children, loc, parent_loc, color, own_style, ais, item.
- `class ModelTooComplex(RuntimeError)` (298).
- `count_instances(doc, limit=MAX_INSTANCES) -> int` (302). Raises `ModelTooComplex`.
- `build_tree(doc, check=True) -> (roots, nodes, proto_count)` (366).

**2. Features and UI strings**
- No widgets. Progress strings passed to `report()`: `"Čtu soubor…"` (101), `"Převádím geometrii…"` (109), `"Kontroluji strukturu sestavy…"` (112), `"Čtu STL…"` (120), `f"Síťuji {n} těles…"` (197).
- Error strings: `"Soubor se nepodařilo přečíst (neplatný nebo poškozený STEP/IGES)."` (103), `"Převod geometrie selhal."` (111), `"STL soubor se nepodařilo přečíst nebo je prázdný."` (123), `"Uložení mezivýsledku selhalo: {st}"` (228), `"Načtení mezivýsledku selhalo: {status}"` (236).
- Assembly limit messages: `"Sestava obsahuje sama sebe (cyklický odkaz mezi díly) – nelze zobrazit."` (328), `"Sestava je vnořená příliš hluboko (více než 200 úrovní) – nelze zobrazit."` (330), `"Sestava by se rozvinula na více než {lim} instancí dílů a sestav – … Soubor se neotevře."` (353-356), and the "bomba" message with `{stored}` and `{total}` (359-362).
- Tree name fallback `"(bez názvu)"` (399).

**3. OCCT used:** see Appendix A groups A, B, C, D, E, F, G, H (`core.py` imports at lines 5-30).

**4. Algorithms and constants (must be preserved)**
- `FAST_FIX_PARAMS = {"FixShape.FixSelfIntersectionMode": "0", "FixShape.FixIntersectingWiresMode": "0"}` (69). Applied to STEP only, and skipped when env `STEPVIEW_FULL_FIX=1` (104). `FixFaceMode` must stay on (docstring 65-68).
- STEP reader modes (93-100): Color True, Name True, Layer False, Props False, GDT False, Mat False, View False. IGES (88-91): Color True, Name True, Layer False.
- STL (121-128): `RWStl.ReadFile_s(path, radians(30))`; a single mesh-only face; name = file base name without extension.
- Meshing (174-222):
  - Shapes with no B-rep surface (mesh-only) are skipped (185).
  - Box via `BRepBndLib.Add_s(shape, box, False)` (188).
  - `size = max extent`, `defl = min(max(size*0.001, 0.002), 0.5)` (192-193).
  - Half-octave buckets: `key = round(log2(defl)*2)`, bucket deflection `2**(key/2)` (194, 206).
  - Angle 0.35 rad, `Relative=False`, `InParallel=True` (153-155).
  - Delabella only when total faces > `DELABELLA_MIN_FACES = 2000` (140, 198-199) and env `STEPVIEW_NO_DELABELLA != "1"`.
  - Delabella pass over the compound (210). Then `BRepTools.Triangulation_s(comp, defl*1.001, False)` (213). Any faces still unmeshed are re-meshed with the default mesher (214-222).
- Instance-bomb guard:
  - `MAX_INSTANCES = 300_000` (290).
  - `AMPLIFY_MIN = 50_000`, `AMPLIFY_RATIO = 100` (294-295). Refuses when `total > 50 000 and total > 100 × stored`, where `stored = memo + component refs + free shapes` (357-362).
  - Depth limit 200 (329). Cycle detection with the `active` set (327). Per-prototype memo (309, 343).
- XBF: `store.SetWithTriangles(app.MessageDriver(), True)` (45). Triangulation is kept in the file.
- `Node.name`: instance name, unless it is empty or starts with `"=>["`, in which case the prototype name is used (397).
- Colour: instance colour wins, otherwise prototype colour (400-402). Surface colour first, then generic (260).
- Placement: `loc = parent_loc.Multiplied(GetLocation(lab))` for references (389).
- `proto_count` counts non-assembly nodes per prototype key (411).

**5. Process model:** none of its own. `read_cad` runs in the worker process for big files, in the GUI process for small ones (`main.py:912-927`), and in the thumbnail process. OCCT holds the GIL while reading (`main.py:18-19`).

**6. Dependencies:** no stepview imports. Used by `main.py` (`count_instances`, `read_cad`, `load_xbf`, `save_xbf`, `build_tree`, `box_limits`, `is_mesh_only`, `ModelTooComplex`) and `thumbs.py` (`read_cad`, `build_tree`).

---

## 2. `stepview/qtbuild.py` (119 lines): numpy to QPainterPath

**1. Purpose and API**
- `qpath_from_elements(types, xy, fill_rule=0) -> QPainterPath` (20). Builds a path from a numpy element stream through `QDataStream` (`ds.__rshift__(path)`). Raises `ValueError` if Qt refuses the element count.
- `elements_from_path(vertices, commands) -> (types, xy)` (78). Input is an ezdxf NumpyPath2d.
- `elements_from_paths(paths) -> (types, xy)` (104). One vectorised pass for many paths.
- Constants: `M_MOVE, M_LINE, M_CURVE, M_CDATA = 0, 1, 2, 3` (14). Big-endian record dtype (15). ezdxf command codes `C_LINE=1, C_CURVE3=2, C_CURVE4=3, C_MOVE=4` (42).

**2. UI:** none.

**3. OCCT:** none. Qt only (`QByteArray`, `QDataStream`, `QPainterPath`).

**4. Algorithms (preserve exactly)**
- Quadratic to cubic: control points `p0 + (q-p0)*2/3` and `e + (q-e)*2/3` (70-74), same as `QPainterPath.quadTo`.
- Fill rule default 0 = OddEvenFill (22).
- The stream trailer is `(last_move, fill_rule)` (33).

**5. Process model:** none.

**6. Dependencies:** imported by `fastscene.py` (line 30).

---

## 3. `stepview/materials.py` (143 lines): density library and per-file assignments

**1. Purpose and API**
- `LIBRARY` (10-18), `LIB = dict(LIBRARY)` (19), `CUSTOM = "Vlastní hustota…"` (20).
- `_valid(m)` (23): `[name, density]` with `0 < density < 100`.
- `store_path(cache_dir, model_path)` (32): `cache_dir/materials/<sha1(normcase(abspath(model)))>.json`.
- `class Assignments(cache_dir, model_path)` (37). Attributes `by_name` and `default`. Methods `save()` (55), `get(proto_name, use_default=True)` (60) returning `(name, density, explicit?)` or None, and `set(proto_names, name, density)` (69). `set` with `name=None` removes the assignment.
- `class MaterialDialog(parent, title, current=None, show_default=False, default=None)` (78). Methods `result_material()` (130) and `result_default()` (139).

**2. UI strings**
- Dialog title: `f"Materiál – {n} dílů"`. Empty-selection title: `"Výchozí materiál pro díly bez materiálu"` (main.py:1652).
- Labels: `"Materiál:"`, `"Hustota:"`, `"Díly bez materiálu:"`.
- Combo items: `"(bez materiálu)"` (data None), each library entry as `f"{name}  –  {density with comma} g/cm³"`, then `"Vlastní hustota…"`.
- Default combo: `"(žádný – nepočítat hmotnost)"`.
- Hint: `"Výchozí materiál se použije pro všechny díly, kterým nebyl přiřazen žádný."`
- Custom name: `f"Vlastní ({value:g} g/cm³)"` with `.` replaced by `,` (136).
- Spin box: decimals 3, range 0.001 to 30.0, suffix `" g/cm³"`, default 7.85 (89-93).

**3. OCCT:** none.

**4. Constants and data (exact densities, g/cm³, lines 11-17)**
- Ocel (konstrukční) 7.85; Nerez (1.4301) 7.90; Litina 7.20; Hliník (slitiny) 2.70; Mosaz 8.50; Měď 8.96; Bronz 8.80; Titan 4.51; Zinek 7.14.
- PLA 1.24; PETG 1.27; ABS 1.04; ASA 1.07; PA6 / PA12 1.14; POM 1.41; PTFE 2.20; PE-HD 0.95; PP 0.91; PVC 1.40; PC / plexi (PMMA) 1.19.
- FR4 (DPS) 1.85; Pryž / silikon 1.20; Sklo 2.50; Dřevo (smrk) 0.45; Dřevo (buk) 0.72; MDF / HDF 0.75; Promasil / izolační deska 0.24; Minerální vata 0.10.
- Assignments are keyed by prototype name (or key), so they survive re-export (38).
- Custom density is a free value; the library value is used for library entries (127-128).

**5. Process model:** none. The JSON is written in the GUI process.

**6. Dependencies:** no stepview imports. Used by `main.py` (`Assignments`, `MaterialDialog`, `CACHE_DIR`).

---

## 4. `stepview/analysis.py` (454 lines): picked-entity measurement, mass, interference

**1. Purpose and API**
- Name tables: `CURVE_NAMES` (22), `SURF_NAMES` (24).
- `_p`, `_pt`, `_dir` formatting (28-37).
- `class Entity` (41). Constructor `Entity(shape)` (44), dispatching on vertex, edge or face. Attributes include kind, type, point, center, axis, radius, length, area, is_line, arc, axes, half_angle, radii, centroid, plane_point, patch, patch_area, loop_closed, fit_rms, p0, p1.
  - `_edge(e)` (67), `_face(f)` (89).
  - Classmethods `from_item(it)` (127), `point(p, normal, mesh, snapped, patch)` (136), `loop(lp, wire)` (154).
  - Methods `short()` (179), `label_pos()` (194), `rows()` (209), `kind_cz()` (249), `ref_point()` (252).
- `face_normal_at(face_shape, p)` (263). Outward normal, reversed for `TopAbs_REVERSED`.
- `_line_line_distance(p1, d1, p2, d2)` (286). Returns `(distance, parallel?)`.
- `relation(a, b) -> (rows, draw)` (296).
- `summary(ents) -> rows` (335).
- `proto_props(shape, mesh=False) -> (vol, area, cog, n_solids, n_faces)` (353).
- `solids_of(shape)` (376), `compound(shapes)` (386), `shape_box(shape, gap=0)` (397), `candidate_pairs(boxes)` (405), `interference(s1, s2, touch=False, tol=1e-4)` (426).

**2. UI strings (all Czech, exact)**
- Kinds: `vrchol`, `hrana`, `plocha`, `bod`.
- Types: `bod`, `vrchol sítě`, `bod na síti`, `bod na ploše`, `bod na ploše, rovina` (the `", rovina"` suffix comes from line 150), `úsečka`, `kružnice`, `oblouk` (arc with < 359.9°), `elipsa`, `spline`, `křivka`, `rovina`, `válec`, `kužel`, `koule`, `anuloid`, `spline plocha`, `plocha`, `… (síť)`, `obrys`, `kružnice (síť)`, `oblouk (síť)`.
- Rows: `Entita`, `X; Y; Z`, `Normála`, `Rovinná plocha (síť)`, `Délka`, `Poloměr R`, `Průměr Ø`, `Střed`, `Osa`, `Úhel oblouku`, `Proložení kružnice` (`± … mm (ze sítě)`), `Směr`, `ΔX; ΔY; ΔZ`, `Poloosy`, `Začátek`, `Konec`, `Plocha`, `Těžiště plochy`, `Poloviční úhel`, `R / r`.
- Relation rows: `Min. vzdálenost` (bold), `ΔX / ΔY / ΔZ`, `Bod 1`, `Bod 2`, `Vzdálenost středů`, `Středy ΔX / ΔY / ΔZ`, `Úhel`, `Úhel (přímka – rovina)`, `Vzdálenost rovin`, `Kolmá vzdálenost (tloušťka)`, `Vzdálenost os (rovnoběžné)` or `(mimoběžné)`.
- Summary rows: `Vybráno`, `Součet délek hran`, `Součet ploch`, `Součet rovinných ploch (síť)`.
- Hover text example: `f"{kind}: {type}  Ø … R …  L …  A … mm²  (x; y; z)"`.

**3. OCCT:** groups D (topology), E (explorer and orientation), I (measuring), J (booleans), K (vertex and edge construction), H (mass), F (bounding boxes). Full list in Appendix A.

**4. Algorithms and constants (preserve)**
- Arc vs circle: `arc = degrees(abs(LastParameter - FirstParameter))`, type becomes `oblouk` when `arc < 359.9` (82).
- Plane normal: reversed when the face orientation is `TopAbs_REVERSED` (100).
- Surface area for mass uses the display mesh, not an exact integral (`proto_props`, 355-362). Comment says exact integration cost 2/3 of runtime.
- Parallel-plane thickness (321-328): `tol = 1e-4` (degrees) when both are B-rep planes, else `2.0` degrees. Distance is `|dot(pa - pb, a.axis)|`. Label `"Vzdálenost rovin"` for two planes, else `"Kolmá vzdálenost (tloušťka)"`.
- Line-line parallel test: `|v1 × v2| < 1e-9` (291).
- Angle between line and plane normal: `abs(90 - min(ang, 180-ang))` (320). Line-line and normal-normal: `min(ang, 180-ang)` for lines only (316-317).
- Interference (426-454):
  - `BRepAlgoAPI_Common`, `SetRunParallel(True)`, `SetFuzzyValue(tol)` with `tol = 1e-4` (435).
  - Clash when intersection volume `> 1e-3` mm³ (448).
  - Touch when `BRepExtrema` distance `<= 1e-3` (452).
  - Boolean exceptions return `(None, 0, None)`.
- Sweep-and-prune on X, then Y and Z overlap test (405-423).
- Mass arithmetic is in `main.py:1625` (section 15).

**5. Process model:** none. Runs in the GUI thread (called from `collisions.py` and `main.py`).

**6. Dependencies:** imports only OCP. Used by `main.py` (`load_occ`, `face_normal_at`, `Entity`, `proto_props`, `relation`, `summary`), by `collisions.py` (`solids_of`, `shape_box`, `candidate_pairs`, `interference`, `compound`).

---

## 5. `stepview/meshmeasure.py` (493 lines): STL mesh analysis (numpy)

**1. Purpose and API**
- Constants (14-21): `FEATURE_DEG = 30.0`, `FLAT_DEG = 0.5`, `GRID_MIN_TRIS = 40_000`, `MAX_FEATURE_EDGES = 400_000`, `MAX_TRIS = 3_000_000`, `ASCII_FACET_BYTES = 400`, `STL_RECORD` dtype (`n` float32[3], `v` float32[3,3], `a` uint16).
- `_read_ascii(f, chunk=16 MiB, max_tris)` (28).
- `class Loop(pts, closed)` (57): `length`, `circle = (centre, normal, radius, rms)` or None, `coverage` in degrees, `kind()` (104).
- `class Mesh` (110):
  - `from_stl(path, max_tris)` (144, binary or ASCII).
  - `from_corners(corners)` (167): vertex welding.
  - `nearest_triangle(p)` (194), `nearest_vertex(p, r)` (217).
  - `prepare()` (234) and `_grid_build()` (240), `_grid_tris(p, r)` (266): uniform facet grid.
  - `loops()` (290), `loop_near(p)` (308), `loop_at(p, tol)` (316).
  - `_topology()` (331): neighbours, boundary and non-manifold counts, sharp edges.
  - `flat_patch(t)` (358): flood fill of coplanar facets, returns `(facets, area, is_face, centroid)`.
  - `_build_loops()` (401): chains sharp edges into polylines.
  - `info_rows()` (446).
- `_closest_on_triangle(p, a, b, c)` (460, Ericson).
- `mesh_for_file(path, max_tris)` (487). Returns None on OSError, ValueError or MemoryError.

**2. UI strings:** (the mesh rows are built in `analysis.py`/`main.py`)
- `info_rows` labels: `Síť` (`"N trojúhelníků, M vrcholů"`), `Uzavřená síť` (`"ano"` or `"ne (N otevřených hran, M nemanifoldních)"`), `Ostré hrany` (`"N obrysů, M kružnic (otvory / čepy)"`), `Průměry kružnic`.
- Loop kind names: `obrys`, `kružnice` (coverage > 300°), `oblouk`.

**3. OCCT:** none.

**4. Constants and algorithms (preserve)**
- Sharp edge: dihedral angle > 30° (`cos(30°)` test, line 350); open boundary and non-manifold edges are always sharp (349).
- Flat patch: facet normal within `FLAT_DEG = 0.5°` of the seed, plane offset `<= dtol`, where `dtol = max(1e-3, 1e-4 × ptp(nodes))` (369, 381).
- Patch is a real planar face when `>= 60%` of its border sides are sharp (396).
- Circle fit (71-102): planar test `s[2]/s[0] <= 0.02`; least-squares circle; reject if `rms > 0.02 r`; reject if coverage < 60°; `kind = kružnice` when coverage > 300°.
- Minimum 6 points for a fit (73).
- Vertex welding: 21 bits per axis, packed into int64 (176-182).
- Facet grid: cell `h = 2 × median facet extent` (246); ≤ 400 cells per query (273); facets spanning more than 2 cells go to a bounding-box list (251, 263).
- Loop chaining: `len(fe) > MAX_FEATURE_EDGES` gives no loops (404).
- ASCII file-size pre-check: `size > max_tris × 400` rejects (158). Binary: `84 + 50×count == size` (151).
- ASCII parser enforces `nverts > 3 × max_tris` while parsing (48).

**5. Process model:** runs in a daemon Python thread named `"stl-mesh"` (`main.py:1381`). Result is returned to the GUI thread through the `meshReady` signal (`main.py:412, 1379`).

**6. Dependencies:** numpy only. Used by `main.py` (`setup_mesh` line 1362, `loop_near`, `loop_at`, `nearest_triangle`, `nearest_vertex`, `flat_patch`, `info_rows`) and `viewer3d.py` (through duck typing: `mesh.loop_near`, `loop_at`, `nearest_triangle`, `nearest_vertex`, `flat_patch`, `display_cache`, `nodes`, `tris`, `normals`, `patch_face`). viewer3d does not import the module.

---

## 6. `stepview/collisions.py` (150 lines): interference dialog

**1. Purpose and API**
- `class CollisionDialog(QDialog)` (15). `__init__(parent, parts, scope, on_pick, on_show_all, on_close)`. `parts` is `[(node, world TopoDS_Shape)]`.
- `run()` (70) does the work. `_fill_sorted()` (131). `_picked()` (140). `toggle_run()` (64). `closeEvent` (147).

**2. UI strings (exact)**
- Window title `"Kolize dílů"`.
- Header `f"Kontrola průniků: {scope} ({n} dílů)"`. Scope is `"vybrané díly"` or `"všechny viditelné díly"` (main.py:1675-1677).
- Checkbox `"hlásit i dotyky (plochy na sobě, bez průniku)"`.
- Buttons `"Spustit"`, `"Zastavit"`, `"Spustit znovu"`, `"Zobrazit všechny průniky"`, `"Zavřít"`.
- Columns `"Díl A"`, `"Díl B"`, `"Typ"`, `"Průnik [mm³]"`. Types `"kolize"` and `"dotyk"`.
- Status: `"Hledám dvojice těles s překrývajícími se obálkami…"`, `f"Tělesa {k} / {n}   ·   nalezeno kolizí {m}"`, and the final `"Hotovo za … s: … dvojic dílů / … dvojic těles s překrytím obálek, <b>N kolizí</b>, M dotyků. Klikni na řádek = ukázat v modelu."` (`"Zastaveno"` when stopped).
- Warning `"Pro kontrolu kolizí jsou potřeba aspoň 2 viditelné díly."` (main.py:1685).

**3. OCCT:** none directly. Calls `analysis.interference`, `analysis.shape_box` (enlarged by 0.01 mm), `analysis.candidate_pairs`, `analysis.compound`, `analysis.solids_of`.

**4. Algorithms and constants**
- Work is per solid, not per part (79-86), so multi-body parts only test the overlapping bodies.
- Box gap `0.01` mm (85). Pairs in the same part are skipped (86).
- One touch test per part pair (100).
- Results per part pair are sorted by volume, descending (132).
- Progress and status refresh at most every 0.1 s (108).

**5. Process model:** runs in the GUI thread. The dialog calls `QApplication.processEvents()` during the loop (78, 112). Stop is a flag (`_stop`, 95). No threads.

**6. Dependencies:** imports `analysis` (line 8). Created by `main.py:run_collisions` (1668-1719).

---

## 7. `stepview/viewer3d.py` (1149 lines): OCCT V3d view embedded in Qt

**1. Purpose and API**
- Constants and helpers: `ORIENT` (47-55), `DEFAULT_MSAA = 2` (58), `msaa_samples()` (61), `rgb(r,g,b)` (80), `nice_step(raw)` (84), `fmt_val(v)` (95), colour constants (103-107).
- `class PickItem(kind, shape, point, normal, mesh, snapped, loop, patch)` (110). `kind` is `"shape"`, `"point"` or `"loop"`. Methods `wire()` (121), `patch_face()` (126), `same(other)` (138).
- `polyline_wire(pts, closed)` (148), `mesh_patch_face(mesh, facets)` (158), `polylines_compound(loops)` (188), `_shape_mid(s)` (201).
- `standalone_object(n)` (212), `leaf_objects(leaves, proto_count, style)` (220) returns `(objects, protos)`.
- `_capsule(ptr)` (242): wraps an HWND into a PyCapsule for `WNT_Window`.
- `class OccView(QWidget)` (249). Signals: `selectionChanged`, `contextMenuAt(QPoint)`, `measured(p1, p2)`, `angleMeasured(float)`, `entitiesPicked(list)`, `hoverText(str)`, `zoomSelected`.
  - Setup: `__init__` (258), `showEvent` (327), `_setup_view` (345), `set_dark_background` (371), `resizeEvent` (379), `redraw` (388).
  - Scale bar: `set_scale_bar` (400), `_update_scale_bar` (410).
  - Camera: `fit_all` (469), `fit_box` (475), `set_orientation(key)` (482), `set_perspective` (486), `_zoom_at` (497), `orbit(ang_h, ang_v, pivot)` (594), `roll(ang)` (585).
  - Mouse and keys: `mousePressEvent` (519), `mouseMoveEvent` (609), `keyPressEvent` (692), `mouseReleaseEvent` (707), `mouseDoubleClickEvent` (740), `wheelEvent` (745), `leaveEvent` (752).
  - 3D mouse: `space_motion(v, dt)` (550).
  - Section: `set_section(axis, pos, flip, redraw)` (766).
  - Measure: `set_measure_mode(kind, snap_objs)` (792), `add_mesh` (848), `show_features` (858), `_hover_activate` (877), `clear_measure` (886), `pin_measure` (896), `world_per_pixel` (906), `_detected_item(shift)` (912), `_world_detected` (955), `_pick_entity` (966), `clear_picked` (981), `show_picked` (986), `draw_relation(draw, label)` (1015), `_measure_click` (1116), `label_at` (1112).
  - Overlays: `clear_overlay` (1035), `show_point(p, text, color)` (1041), `show_shapes(shapes, color, transparency)` (1058), `_mline` (1074), `_mlabel` (1087), `_marker` (1104), `_patch_obj` (679), `_show_hover` (648).
  - `screenshot(path)` (1148) calls `view.Dump`.

**2. UI and interaction (exact strings and behaviour)**
- Hover text and measure hints are set from `main.py`. Viewer strings: scale bar label `f"{fmt_val(length)} mm"` (446); T label `"  T  " + fmt_num(...) + " kg"` (main.py:1750), or `"  T (objem)"`.
- View cube labels (356-361): `"Přední"` (front, Yneg), `"Zadní"` (Ypos), `"Horní"` (Zpos), `"Spodní"` (Zneg), `"Levý"` (Xneg), `"Pravý"` (Xpos).
- Mouse (exact behaviour):
  - LMB click: select (`AIS_SelectionScheme_Replace`). Ctrl+LMB: XOR (`AIS_SelectionScheme_XOR`, 733). Empty space clears (730-731).
  - RMB press and drag: orbit about a pivot. The pivot is the model point under the cursor at press time, or the view centre (528-530, 536-543).
  - MMB, or Ctrl+RMB: pan (531-532, 629-630).
  - RMB click without drag: context menu (737-738).
  - LMB double-click: zoom to selection (740-743).
  - Wheel: `_zoom_at(x, y, 1.2 ** (delta/120))` (750). Ignored for 0.4 s after 3D mouse motion (748).
  - Drag threshold: 4 px manhattan (622).
  - Arrow keys: orbit 15°. Shift+arrow: 90° (696-703). Ctrl, Alt or Meta disable this.
  - Enter in pick mode: `pin_measure` (693-694).
  - View cube click: `cube.HandleClick` plus animation (720-723).
- Measure tool (`measure_mode == "pick"`, Onshape-like): LMB adds or removes an entity (724-726). Shift+click = exact B-rep face point with normal (`_detected_item`, 947-951).
- Hover preview is sent to the status bar through `hoverText`.

**3. OCCT:** groups A (not used here), G (`BRepMesh`, `BRepBuilderAPI`), I (`BRepAdaptor`, `gp`), K (vertex/edge/polygon construction), L (display, AIS, Graphic3d, V3d, Aspect, Prs3d), M (`Image`/dump via `view.Dump`), N (`Quantity_Color`). Full list in Appendix A, group L.

**4. Constants and algorithms (preserve)**
- MSAA: env `STEPVIEW_MSAA`, else QSettings key `"msaa"` (read only; the code never writes it), else default 2. Allowed values 0, 2, 4, 8 (61-77).
- Colours:
  - Default part colour `(200, 204, 210)` (main.py:284).
  - Edge line `(25, 25, 30)` (viewer3d.py:305).
  - Highlight `(80, 200, 255)` (309). Selection `(255, 140, 0)` (312).
  - `PICK_COLOR = (255, 140, 0)`, `HOVER_COLOR = (0, 170, 255)` (105-106).
  - Axis colours X, Y, Z: `(220, 40, 40)`, `(30, 160, 50)`, `(40, 90, 230)` (107).
  - Picked-snapped point marker `(0, 150, 255)` (992). Centre-of-gravity text `(0, 120, 255)` (1041).
  - Interference overlay `(230, 20, 20)` (main.py:1708, 1716).
  - Measure label `(170, 0, 0)` (1091). Centre line `(120, 120, 130)` (1031).
  - Gradient light: top `(230, 235, 242)`, bottom `(150, 162, 180)`. Dark: `(50, 54, 62)` and `(20, 22, 26)` (372).
  - Scale bar: `(235,235,235)` dark, `(30,32,38)` light (439).
  - ViewCube box `(225, 228, 235)` (356).
- ViewCube geometry: size 55 px, font 11 px, placed at 90 px (354-358). Animation duration 0.35 s (362). Animation timer 15 ms (303).
- Scale bar (410-460): target length 110 px, rounded by `nice_step` to 1/2/5 × 10ⁿ (427). Only in orthographic projection (415). Persistence `TMF_2d` at offset (24, 22) px (440-441). Text height 13 px (451).
- Snap and picking:
  - Overlay pixel tolerance `SetPixelTolerance(3)` normally, `7 × dpr` while measuring (802-803, 314).
  - Mesh vertex snap radius `7 × world_per_pixel` (930, 937, 1122).
  - Loop pick tolerance `7 px` (930).
  - Pivot re-pick after 0.3 s idle (556).
- 3D mouse object-mode mapping: `SM_ROT = 150°/s` (546), `SM_PAN = 1100 px/s` (548), `SM_ZOOM = 2.2` e-folds/s (549).
- Orbit: `k = 0.006 / dpr` rad per physical pixel (627).
- Section plane (766-789): normal is `-1` on the axis by default, `+1` when flipped (776). Capping is on, uses object material, hatch off (781-785).
- Scale-bar rounding and mapping for 2D views are in `view2d.py`.
- Perspective zoom (`_zoom_at`, 511-514): the eye moves toward the cursor point.
- `show_shapes` meshes results with `BRepMesh_IncrementalMesh(s, 0.05, False, 0.3, True)` (1063), topmost z-layer.
- Measure labels: distance `f"{d:.2f} mm"` with `.` replaced by `,` (1133); angle `f"{ang:.2f}°"` (1143).

**5. Process model**
- No threads of its own. Runs in the GUI process only.
- `mesh_patch_face` writes a temporary binary STL with `tempfile.mkstemp(prefix="cadview_", suffix=".stl")` (170), reads it with `RWStl.ReadFile_s`, and deletes it (177-179). This is in the GUI process.
- `_capsule` uses `ctypes.pythonapi.PyCapsule_New` (243-246).
- `OpenGl_GraphicDriver(display, True)` (267). Linux uses `Xw_Window` (45), Windows uses `WNT_Window` (43).

**6. Dependencies:** no stepview imports. Imported by `main.py` (`OccView`, `leaf_objects`, `rgb`, `standalone_object`, `polylines_compound`), by `thumbs.py` (`_capsule`, `leaf_objects`, `rgb`), and it duck-types `meshmeasure.Mesh` and `analysis.Entity` through `PickItem`.

---

## 8. `stepview/fastscene.py` (802 lines): batched 2D scene and native level assignment

**1. Purpose and API**
- Module constants (32-39):
  - `K_LINES, K_POLY, K_PATH = 0, 1, 2`.
  - `CHUNK_ELEMS = 6000`.
  - `BIG_PRIM = 3000`.
  - `MAX_LEVELS = 400000`.
  - `PY_MAX_ENTITIES = 15000`.
  - `PY_HARD_MAX_PRIMS = 30000`.
  - `VISIT_BUDGET_PY = 3e7`, `VISIT_BUDGET_NATIVE = 4e8`.
  - `NOMERGE` from env `STEPVIEW_DXF_NOMERGE`.
- Native module import (41-48): `import rdxf`. Refused when `API != 2` (45) or env `STEPVIEW_NO_RDXF` is set.
- `class FastPathError(RuntimeError)` (51). `have_native()` (55).
- Levels: `_bounds` (60), `_cell_ranges` (67), `grid_cell(bb, pad, budget)` (75), `assign_levels_py` (91), `assign_levels(bb, gid, pad)` (130).
- `morton(x, y)` (139).
- Items: `class ChunkItem(QGraphicsPathItem)` (155): empty `shape()`, numpy bounding rect. `class FillItem` (176).
- `class Batch` (194): `pick(r)` (230), `snap_vertices` (271), `snap(r, tr, px, py, lines)` (291), `highlight(scene, top, color)` (318), `chunks_visible` (207), `prim_elements` (217), `prims_of_top` (221), `entities_of` (225).
- `class BatchBackend(Backend)` created by `backend_class()` (360-626). Methods `draw_line` (451), `draw_solid_lines` (458), `draw_path` (469), `draw_filled_paths` (475), `draw_filled_polygon` (481), `add_prim` (420), `get_group` (412), `build` (487).
- `arc_vertices(cx, cy, r, a0, a1)` (633).
- `frontend_class()` (656) returns `FastFrontend` (673). Methods `draw_entities` (693), `_fast` (709), `_geometry` (744).
- `draw_layout(ctx, layout, cfg, scene)` (793) returns the backend. Sets `scene.batch` and `scene.setSceneRect`.

**2. UI:** none. Layer toggles go through `view2d.LayerMap`.

**3. OCCT:** none. Qt Graphics and ezdxf frontend only.

**4. Algorithms and constants (preserve)**
- Z-order: a primitive goes to the lowest level of its own group above all levels already used by other groups in the cells its inflated bounding box touches (see `assign_levels_py` 91-127; identical to `rdxf` 29-135).
- Grid cell (75-88): `1.5 × median primitive extent`, at least `max(w,h)/512`. Grown by 1.6× up to 16 times until the total cell visits fit the budget.
- Pad: `0.003 × max(fw, fh) + 1e-9` (534).
- `MAX_LEVELS` check (536). Levels above it fall back to the legacy renderer.
- Strokes: merged into chunks of up to 6000 path elements in Morton order (570-571, 605-607). Translucent strokes (colour alpha ≠ FF) and `NOMERGE` keep one item per primitive (602-603).
- Fills and text: one item per primitive (587-598).
- Pen: cosmetic, width `lineweight / 0.3527 × lineweight_scaling` (561). Stroke half-width for picking `hw = lw/0.3527 × scale × 0.5` (547).
- Minimum lineweight `0.24` mm set on the config (387-389).
- Picking margin: `hw × 1.5` for square caps (237). Stroker: square caps, round joins (244-245). Long primitives (>3000 elements) pick only the segments near the cursor (250-256).
- Snap: vertices of paths first, plain-line endpoints as a fallback (`snap`, 291-315). Distances are measured in whole pixels (`qround`, 302-303). Ties go to the topmost primitive (307-313).
- Highlight width `max(2.0, lw/0.3527 × scale)` (335). Polygons (SOLID, wide polylines) get no pen (`0.0`) (335).
- Arcs (633-650): `n = ceil(span / π × 2)` cubic segments (each at most a quarter turn), `t = 4/3 × tan(seg/4)`.
- Circle: reversed vertices, start at the exact start point (774-775).
- Degenerate arc `|span| < 1e-6` or full circle goes to the stock code (781-782).
- Dashed lines (`continuous` false) and `Viewport` go to the stock code (727-728, 702-703).
- Invisible entities draw nothing (725-726).
- Text and dimensions go to ezdxf's own frontend.

**5. Process model:** none. Runs in the GUI thread or in the thumbnail process (`thumbs.render_2d` through `view2d.render_thumbnail`).

**6. Dependencies:** imports `qtbuild` (line 30). Imported lazily by `view2d.render_scene` (lines 458, 502). Imports native `rdxf` if present.

---

## 9. `native/rdxf/src/lib.rs` (305 lines): Rust helper (PyO3 + numpy)

**1. Purpose and API**
- Python module `rdxf` (`#[pymodule] fn rdxf`, line 300).
- `API: u32 = 2` (line 16). `fastscene.py` requires exactly 2 (`fastscene.py:45`).
- `assign_levels(bb, gid, pad, cell) -> int32[P]` (line 30). Inputs: `bb` (P,4) f64, `gid` (P,) i32 ≥ 0 (contiguous), floats. Returns ValueError on violations.
- `read_bulk(path) -> dict` (line 276). Proof of concept, file header says "NOT used by the viewer yet" (line 5). Returns `kind`, `layer`, `color`, `flags`, `true_color`, `geo` (n×8), `pts_start`, `pts_n`, `pts` (k×3), `layer_names`, `other_types`, `n`.

**2. UI:** none.

**3. OCCT:** none.

**4. Algorithm and constants**
- `MAX_CELLS = 4_000_000` (line 19). Grid bigger than this is rejected.
- Same logic as `assign_levels_py`: `best_l`, `best_g`, `second_l` per cell (82-84). Per-group ascending level lists (86). `partition_point(|&c| c <= m)` finds the lowest own level above `m` (107-115).
- Update rule (117-132): if `l > best_l`, set `best_l` and `best_g`, and when the old best group differed, `second_l = old best_l`. Else if the group differs and `l > second_l`, set `second_l = l`.
- Input checks: finite values, `pad >= 0`, `cell > 0`, ids ≥ 0 (41-53).
- DXF parser (160-272): ENTITIES section only, codes 8, 62, 420, 70, 43, 10/20/42 (LWPOLYLINE), 11/21, 40, 50, 51. Colour default 256 (196-199).

**5. Process model:** none. Called from the GUI or thumbnail process as a Python extension.

**6. Dependencies:** consumed only by `fastscene.py`. Must match the Python fallback result exactly (the file header says so).

---

## 10. `stepview/view2d.py` (1142 lines): DXF/DWG loading, drawing, 2D view

**1. Purpose and API**
- Constants: `DRAW_EXT = (".dxf", ".dwg")` (28). `NOT_DRAWABLE` (29-30). `NO_WINDOW` (31). `_ENV_LOCK` (32).
- Converters: `find_oda()` (45), `find_dwg2dxf()` (58), `dwg_to_dxf(src, dst) -> converter name` (69).
- Loading: `read_dxf(path) -> (doc, errors)` (99), `fix_inventor_refs(doc)` (110), `load_drawing(path, dxf_cache=None, report) -> (doc, info)` (140).
- `layer_of(e)` (173), `drawable_count(layout)` (181), `default_layout(doc)` (185), `layer_table(doc)` (195).
- Security: `is_remote_path(name)` (211), `remote_images(doc)` (228).
- Budget: `expanded_entity_count(doc, layout, limit)` (255), `check_layout_budget(doc, layout_name)` (322). Exception `DrawingTooComplex` (251).
- `class LayerMap(groups, base)` (330): `is_shown`, `apply(layer_vis)` (349), `bounding_rect(scene)` (372).
- Rendering: `render_scene(doc, layout_name, layer_vis, light_bg, images, track, fast)` (440). `_render_scene` (475). `_tracking_backend()` (398).
- Info: `UNITS` (518), `_p` (522), `_pt` (526), `doc_info(doc)` (530), `entity_rows(e)` (537).
- View: `class DrawingView(QGraphicsView)` (588). Signals: `cursorMoved`, `entityPicked(top, sub)`, `measured`, `angleMeasured`, `contextMenuAt`, `viewChanged`.
  - `set_scene` (626), `apply_layers` (661), `fit` (683), `zoom_to_items` (690), `clear_selection` (699), `select_top` (711), `_pick_batch` (727), `_pick` (742), `set_measure_mode` (758), `_snap` (832), `_measure_click` (881), `space_motion` (969).
  - Mouse: `mousePressEvent` (917), `mouseMoveEvent` (922), `mouseReleaseEvent` (939), `mouseDoubleClickEvent` (958), `wheelEvent` (962).
- `class Ruler(view, horizontal)` (1000). `_nice` (1085). `class DrawingPanel(view)` (1095). `set_rulers(on)` (1113).
- `render_thumbnail(doc, size, path)` (1118).

**2. UI (exact)**
- Converter errors: `"Chybí převodník DWG (dwg2dxf z LibreDWG, případně ODA File Converter)."` (89). `"Převod DWG selhal"` plus last stderr line (94).
- Progress: `"Převádím DWG na DXF…"` (151), `"Čtu DXF…"` (145, 149, 165).
- Budget messages: `"Výkres se rozvinutím bloků rozpadne na více než {lim} entit – … Výkres se nevykreslí."` (266-268). Cyclic block: `"Cyklický blok: „{name}“ se přímo nebo nepřímo vkládá sám do sebe – výkres nelze vykreslit."` (275-276). Depth: `"Bloky jsou vnořené příliš hluboko (více než {MAX_BLOCK_DEPTH} úrovní) – výkres nelze vykreslit."` (278-279). Amplification: `"Výkres má v souboru jen … entit, ale rozvinutím vnořených bloků by jich vykreslil …"` (316-318).
- Entity row labels: `Entita`, `Hladina`, `Délka`, `Úhel`, `Začátek`, `Konec`, `Poloměr`, `Průměr`, `Střed`, `Obvod`, `Úhel oblouku`, `Délka oblouku`, `Vrcholů`, `Uzavřená` (`ano`/`ne`), `Text`, `Výška`, `Naměřeno`, `Text kóty`, `Blok`, `Vložení`, `Měřítko`, `Natočení`, `Šrafa`, `Handle`.
- Ruler: label text with `,` as decimal separator (1058-1063).
- Measure label: `_p(length, 2)` (899), angle with `°` (905-910).
- Remote image skip, used in `main.py:1235`.

**3. OCCT:** none.

**4. Algorithms and constants (preserve)**
- DWG pipeline: ODA File Converter if found (`find_oda`, 45-55; Windows glob under `ProgramFiles`/`ProgramFiles(x86)`; Linux `shutil.which` and `/opt`, `/usr/bin` globs). Otherwise bundled LibreDWG `dwg2dxf -v0 -y -o dst src`, 600 s timeout (`dwg_to_dxf`, 87-95).
- ODA version: `odafc.convert(src, dst, version="R2018", replace=True)` (83). `QT_*` environment variables are removed for the ODA call, under `_ENV_LOCK` (80-85).
- LibreDWG path on Windows (60): `libredwg/dwg2dxf.exe` from `_resource_dir()`. Windows never falls back to `PATH` (63-64).
- Read: strict `ezdxf.readfile` first, then `recover.readfile` (99-107).
- Limits: `MAX_EXPANDED_ENTITIES = 300_000` (242). `AMPLIFY_MIN = 50_000` (246), `AMPLIFY_RATIO = 100` (247). `MAX_BLOCK_DEPTH = 100` (248). Memo per block name (252-253).
- Layer visibility: LayerMap groups items by the tuple of layer keys they depend on (`set_item_data` in `_tracking_backend`, 414-426). The layer toggle is `all(layer visible)` per group (`is_shown`, 345-347). Blind entities `Insert, Face3d, Viewport` are ignored for layer keys (`blind`, 367).
- `render_scene` (440-472): fast path first. Falls back to the legacy renderer on any exception except `DrawingTooComplex`. Legacy forced by env `STEPVIEW_DXF_LEGACY` (437).
- Fast path refused without native module when entities > 15 000 (459-460).
- Thumbnail: white paper, 3 % margin (1127), target draw `QRectF` centred (1131), border `(150,150,150)` (1135), `images=False` (1121).
- Ruler (1036-1070): step `_nice(75 × upp)`, minor step `step/4` when the mantissa is 2 else `/5`, ticks capped at 3000 (1048).
- Pick radius 5 px (`_pick`, 745). Snap radius 10 px (832). Drag threshold 4 px (931). Wheel zoom `1.2 ** (d/120)` (966). Keyboard pan in `space_motion`: `1100 px/s`, zoom `2.2` (974, 979-980).
- Selection overlay orange `(255, 140, 0)` (596). Measure pen `(230, 30, 30)`, width 2 cosmetic (775). Snap square 10 px `(0, 200, 255)` (873). Ruler cursor `(230, 60, 60)` (1078).
- `UNITS` map from `$INSUNITS` codes (518-519).

**5. Process model**
- DWG conversion happens in `DrawingLoader` (a QThread in `main.py`, 392-408). ODA and LibreDWG run as subprocesses, LibreDWG with `capture_output`.
- DWG temporary directories: `tempfile.mkdtemp(prefix="dwg_", dir=base)` (157). `base` is the cache directory when caching, else the system temp (155-157). Removed in `finally` with `shutil.rmtree` (168).
- With a cache path, the converted DXF is moved into the cache with `os.replace` (162-164).
- Thumbnail path (`render_thumbnail`) also calls `load_drawing` with no cache, so DWG thumbnails use a temp directory.

**6. Dependencies:** imports fastscene lazily. ezdxf lazily (`import ezdxf` inside functions). Used by `main.py` (`DRAW_EXT`, `load_drawing`, `render_scene`, `DrawingView`, `DrawingPanel`, `entity_rows`, `doc_info`, `layer_table`, `layer_of`, `drawable_count`, `default_layout`, `remote_images`, `DrawingTooComplex`, `QGraphicsScene`), by `thumbs.py` (`load_drawing`, `render_thumbnail`), and by `fastscene.py` indirectly (through `LayerMap`-style names).

---

## 11. `stepview/spacemouse.py` (380 lines): 3Dconnexion via Win32 Raw Input

**1. Purpose and API**
- Constants (25-46): `FULL_SCALE = 350.0`, `DEAD_ZONE = 0.04`, `CURVE = 1.6`, `WM_INPUT = 0x00FF`, `RID_INPUT = 0x10000003`, `RIDI_DEVICEINFO = 0x2000000B`, `RIDI_DEVICENAME = 0x20000007`, `RIM_TYPEHID = 2`.
- `PRODUCTS` (34-39): product ids mapped to names. `VK_LAYOUT` (42): product ids with virtual-key buttons. `BUTTONS` (43-44). `AXES` (45-46).
- `available()` (49).
- Win32 structs and `_user32()` (54-96). `devices()` (108) returns `[(vid, pid, name)]` for usage page 1, usage 8.
- `class SpaceMouse(QObject)` (127). Signals `motion(object, float)` and `button(str)`. Methods:
  - `load()` (156), `save()` (173), `attach(hwnd)` (181, `RegisterRawInputDevices` with usage page 1 and usage 8).
  - `handle(message)` (187), `report(device, data)` (209), `levels()` (244), `shaped()` (249), `_tick()` (264), `reset()` (276), `_app_state()` (281).
- `class SpaceMouseDialog(sm, parent)` (287).

**2. UI strings (exact)**
- Dialog title `"3D myš (3Dconnexion)"`.
- Device status `"Připojeno: "` followed by names with `(VVVV:PPPP)`. Empty state: `"Žádná 3D myš nenalezena. Připoj ji – funguje i připojení za běhu programu."`
- `"Ovládat pohled 3D myší"`, `"Rychlost:"`, `"Otáčení"`, `"Posun a zoom"`, `"Jen převažující pohyb (dominantní osa)"`.
- Axis group `"Osy – vychýlení čepičky a obrácení směru"`; per-axis checkbox `"obrátit"`.
- Axis names (`AXES`): `"Posun vlevo / vpravo"`, `"Posun nahoru / dolů"`, `"Zoom (tah k sobě / od sebe)"`, `"Klopení (vpřed / vzad)"`, `"Otáčení (kroucení)"`, `"Naklápění (do stran)"`.
- Button label `"Tlačítko: –"` and `f"Tlačítko: {n}"`.
- Info text: `"Model se pohybuje jako čepička (tlač doprava = model doprava, tah k sobě = přiblížit, kroucení = otáčení). Tlačítka: Fit = zobrazit vše, na SpaceMouse Pro / Enterprise i pohledy (Top, Front, Right, …) a Roll. Levé tlačítko (Menu) obsluhuje ovladač 3DxWare."`
- Speed label: `2 ** ((slider - 50)/25)` formatted as `"0.25×"` … `"4×"`, comma decimal.
- Menu entry `"3D myš (3Dconnexion)…"` (main.py:788).

**3. OCCT:** none.

**4. Algorithms and constants**
- HID reports: rid 1 is translation X, Y, Z (int16). If length ≥ 13 it carries rotations too (6-axis report) (215-217). rid 2 is rotation (218-219). rid 3 is buttons (220-230).
- Axis mapping to screen frame (levels, 247): `(tx, -tz, ty, rx, -rz, ry)` from HID values (HID Z is down, Y towards the user).
- Dead zone and curve (253): `y = sign(x) × ((|x| - 0.04)/(1 - 0.04))^1.6`, zero inside the dead zone. Inversion per axis (254). Pan/rotate masks (255-258). Dominant-axis mode keeps only the largest axis (259-261). Speed multiplier last (262).
- Buttons: `BUTTONS = {1:"fit", 2:"top", 3:"left", 4:"right", 5:"front", 6:"bottom", 7:"back", 8:"roll_cw", 9:"roll_ccw", 10:"iso", 11:"iso"}` (43-44). Bit 0 is Menu (left to 3DxWare). Only emitted for `VK_LAYOUT` products (228).
- Timer 16 ms (145). `dt` clamped to `[0, 0.1]` (266).
- Speed 0.1 to 10 when loaded (169), dialog slider 0 to 100 logarithmic, 0.25× to 4× (308-309).
- Stop after the cap returns to zero (268-270).
- Reset on losing focus (282-283).

**5. Process model:** none. Runs in the GUI thread from `nativeEvent` (main.py:804-811). Registration is for the main window handle (`attach`, main.py:800).

**6. Dependencies:** no stepview imports. Used by `main.py` (`SpaceMouse`, `SpaceMouseDialog`, `_space_motion`, `_space_button`, `nativeEvent`, `showEvent`). It calls `view.space_motion` and `view.roll` and `view.set_orientation`, and `view2d.space_motion`.

---

## 12. `stepview/thumbsettings.py` (176 lines): Explorer thumbnail switch (HKCU)

**1. Purpose and API**
- Constants: `CLSID = "{7A1F3C52-8E0B-4C6D-9F2A-5B3E1D4C6A70}"` (10), `IID_THUMB = "{e357fccd-a995-4576-b01f-234630154e96}"` (11), `FORMATS` (12-13): STEP `.step .stp`, IGES `.iges .igs`, STL `.stl`, DXF `.dxf`, DWG `.dwg`.
- `available()` (16) = win32 only. `_key(ext)` (25).
- `is_on(ext)` (29) checks HKCU and HKLM. `machine_wide(ext)` (42).
- `installed_dir()` (58): frozen copy only, under `%LOCALAPPDATA%\Programs` or `ProgramFiles*`.
- `_ensure_clsid()` (70): registers the COM server for the current user if the installer did not. Returns a Czech error string or None.
- `set_on(ext, on)` (103). `notify_shell()` (115): `SHChangeNotify(0x08000000, ...)` (`SHCNE_ASSOCCHANGED`).
- `class ThumbDialog(parent)` (122). `apply()` (164).

**2. UI strings (exact)**
- Title `"Náhledy v Průzkumníku"`.
- Text `"Zobrazovat v Průzkumníku Windows náhled modelu / výkresu místo ikony u souborů:"`.
- Checkbox labels: `f"{name}  ({exts})"`, and for DWG a suffix `"   – méně bezpečné (převodník LibreDWG)"`.
- Tooltip for DWG: `"Náhled spouští převodník LibreDWG na každý zobrazený soubor. Ten je méně odolný vůči poškozeným nebo podvrženým souborům – zapínej jen, když pracuješ s DWG z důvěryhodných zdrojů. Vykreslování náhledů běží v omezeném procesu (nízká integrita, limit paměti)."`
- Machine-wide tooltip: `"Zapnuto instalací pro všechny uživatele – změnit jde jen přeinstalací"` (checkbox disabled).
- Info: `"Pozn.: ve složce SynologyDrive náhledy nefungují (Synology tam používá vlastní).<br>Už vytvořené náhledy si Windows pamatují – po vypnutí je smaž tlačítkem níže (Vyčištění disku → <b>Miniatury</b>)."`
- Button `"Vymazat uložené náhledy…"` (runs `cleanmgr.exe` by full path under `SystemRoot`, 154-155).
- Error messages from `_ensure_clsid`:
  - `"Náhledy v Průzkumníku jde zapnout jen z nainstalované verze (instalátor CadProhlizec_setup). …"`
  - `"Chybí StepThumb.dll vedle programu – náhledy nejde zapnout."`
- Tray of the main menu: `"Náhledy v Průzkumníku…"` (main.py:786).

**3. OCCT:** none.

**4. Constants and algorithms**
- Registration key: `HKCU\Software\Classes\SystemFileAssociations\<ext>\ShellEx\{IID_THUMB}`, default value = CLSID (line 26, 106-107).
- COM server key: `HKCU\Software\Classes\CLSID\{CLSID}` with default `"CAD prohlížeč – náhledy"` (92). `InprocServer32` values (94-99): default `mscoree.dll`, `ThreadingModel` `Both`, `Class` `StepThumb.StepThumbnailProvider`, `Assembly` `StepThumb, Version=1.3.0.0, Culture=neutral, PublicKeyToken=null`, `RuntimeVersion` `v4.0.30319`, `CodeBase` `file:///<dll path with />`.
- Only the installed copy is registered, not a portable copy (72-87).
- Disable removes the per-extension key (110).
- DWG is only enabled when chosen, and is disabled by default in the installer (iss:45, 112).

**5. Process model:** none. Writes the registry in the GUI process.

**6. Dependencies:** imports `winreg` and `ctypes`. Used by `main.py` (`thumb_settings`, line 790).

---

## 13. `stepview/thumbs.py` (374 lines): renderer for `--thumbnail`

**1. Purpose and API**
- Entry: `run(src, out, size) -> int` (347). Exit 0 means PNG written; 1 means failure. Called from `main.py:172-181` as `thumbs.run(argv[2], argv[3], argv[4])`, then `os._exit(code)`.
- `_qt_app()` (16), `_matte(path_black, path_white, out, size)` (21).
- `render_3d(path, out, size)` (53): OCCT off-screen render with OpenGL.
- `_lin2srgb(v)` (123). `_collect_triangles(doc)` (128). `render_3d_soft(doc, out, size)` (177): software renderer.
- `render_2d(path, out, size)` (264).
- `_headless()` (281). `remote_step_refs(path)` (320). `_has_external_keyword(path)` (304).
- Constants: `MAX_INPUT = 80 MiB` (297), `_EXTERNAL_KEYWORDS` (300), `_PIECE = 8 MiB` (301).

**2. UI:** none. `_headless` installs a silent Qt message handler (292) and sets `SetErrorMode` (287).

**3. OCCT:** `AIS_InteractiveContext`, `AIS_Shaded`, `Aspect_DisplayConnection`, `Aspect_TOL_SOLID`, `Graphic3d_BufferType`, `Image_AlienPixMap`, `OpenGl_GraphicDriver`, `Prs3d_LineAspect`, `TCollection_AsciiString`, `V3d_TypeOfOrientation_Zup_AxoRight`, `V3d_Viewer`, `WNT_Window` (lines 55-63). Software path uses `BRep_Tool.Triangulation_s`, `TopExp_Explorer`, `TopLoc_Location`, `TopoDS.Face`, `XCAFDoc_ShapeTool.GetShape_s`, `Shape.Moved`, `loc.Transformation()`, `Poly_Triangulation` node and triangle access (132-171).

**4. Algorithms and constants (preserve)**
- Size clamp: `size = max(16, min(size, 1024))` (349). Render size `rs = min(max(size,64),1024) * 2` (104), then 2×2 downsample (255-258, render_3d_soft).
- `render_3d`: MSAA 8 samples (89), `swapInterval = 0` (77), backgrounds black and white, then alpha = `1 - (white - black)` mean over RGB, colour = `black / alpha` (`_matte` 21-50). Crop to content with `alpha > 0.02` (38) and centre into `size × 0.94` (42).
- `render_3d_soft`:
  - `S = 2 × size` (184).
  - Isometric axes: `back = (1,-1,1)/√3`, `right = (1,1,0)/√2`, `up = back × right` (185-187).
  - Fit `k = 0.94 × S / extent` (190).
  - Light `back×0.75 + up×0.55 - right×0.35`, normalised (199-200).
  - Two-sided Lambert, shade `0.30 + 0.70 × λ` (202). Specular term `0.08 × λ⁸` (203).
  - Samples per triangle `ceil(3 × projected area)`, clipped to 200 000 per triangle (206), total cap 12 000 000 (207-208).
  - R2 sequence `0.7548776662466927`, `0.5698402909980532` (211-212).
  - Nearest sample per pixel by z (223-226).
  - Hole fill when at least 3 of 4 neighbours are covered (235-244).
  - Face boundaries darkened to 25 % (248-253).
- `render_2d`: DWG gets a private `mkdtemp(prefix="stepthumb_")` and `o.dxf` (272-273), removed with `rmtree` (277-278).
- `run` dispatch (355-370):
  - Files > 80 MiB refused (352).
  - STEP and STEPVIEW only: `remote_step_refs(src)` refuses (356).
  - DXF and DWG go to `render_2d`.
  - On win32, OCCT OpenGL render first (`render_3d`), software fallback if it throws (361-366). Env `STEPVIEW_SOFT_THUMBS` forces software (361).
  - Non-win32 uses `render_3d_soft`.
- `remote_step_refs` (320-344): keyword pre-filter (`DOCUMENT_FILE`, `EXTERNAL_SOURCE`, `EXTERNALLY_DEFINED`, 300), then a network-path/URL/escaped-slash regex (327). Overlap 256 bytes between pieces (340).

**5. Process model**
- Separate process. Windows: launched by `StepThumb.cs` through `Sandbox.Run` (Low IL plus job object). Linux: launched by the `.thumbnailer` with `timeout -k 5 60 prlimit --as=4000000000 --cpu=120` (install.sh:75-85), and the program sets its own `RLIMIT_AS` 4 GiB and `RLIMIT_CPU` 120 s (`main.py:153-175`).
- Temporary files: `tempfile.mkdtemp(prefix="stepthumb_")` for three PNG renders (`render_3d` 105-118), and for DWG (272-278).
- Windows GDI: `user32.CreateWindowExW` hidden `STATIC` window used as OCCT's native window (75), destroyed at the end (119).
- `_headless()` sets `QT_QPA_PLATFORM=offscreen` process-wide (284).

**6. Dependencies:** imports `core` (read and build tree), `viewer3d` (`_capsule`, `leaf_objects`, `rgb`), `view2d` (`load_drawing`, `render_thumbnail`, `default_layout`).

---

## 14. `stepview/main.py` (2361 lines): GUI, workers, cache, CLI

Section 14 has the most UI. Items 1 to 6 follow.

**1. Purpose, CLI, functions and classes**

CLI (exact argv handling):
- `main.py <file>`: the first argument that does not start with `--` is opened (`main()`, lines 2332-2334). Early prestart at import (223-237) uses `sys.argv[1]`.
- `--thumbnail <src> <out.png> <size>`: dispatched at line 172 (before any Qt import) when `len(argv) >= 5`. Exit code is 0 or 1 and the process ends with `os._exit`.
- `--worker <src> <dst>`: line 145, `len(argv) >= 4`. Runs `run_worker` and exits.
- `--worker-serve <job>`: line 149, `len(argv) >= 3`. Pre-started idle worker.
- Env `STEPVIEW_SHOT` (2335), `STEPVIEW_SHOT_DELAY` (2353, default 4000 ms), `STEPVIEW_SELECT` (2341) are testing hooks: they save a screenshot via `screen().grabWindow` and quit.

Module-level:
- `VERSION = "1.9"` (16). `CACHE_MIN = 1 MiB` (17). `WORKER_THRESHOLD = int(env STEPVIEW_WORKER_MIN or CACHE_MIN)` (20). `CACHE_LIMIT = 3 GiB` (21). `CACHE_DIR` (22-25). `WORKER_IDLE_S = 120` (48).
- `run_worker(src, dst)` (29-45). `job_valid(src, dst)` (51-65). `serve_worker(job)` (68-89). `spawn_worker()` (125-136). `submit_job(job, src, dst)` (139-142).
- `ensure_cache_dir()` (100-109). `cache_path(path)` (112-115). `part_path(cache)` (118-119).
- `prewarm_ezdxf()` (186-209), `wait_ezdxf_warm()` (212-218), `limit_thumbnail_resources(...)` (153-169).
- `load_occ()` (266-281): lazy import of OCP and viewer3d.
- `lin2srgb` (304), `qcolor` (309), `fmt_len` (315), `fmt_num` (319), `class Html(str)` (324), `esc` (328), `rich_tip` (332), `store_cache` (339), `prune_cache` (349), `clear_cache` (374).
- `class DrawingLoader(QThread)` (392): `loaded`, `failed`, `progress` signals.
- `class MainWindow(QMainWindow)` (411). Key methods with line numbers:
  - `ensure_3d` (541), `_build_actions` (582), `_build_status` (711), `show_welcome` (730), `open_dialog` (747), `drawing_likely` (759), `fill_recent` (771), `thumb_settings` (790), `showEvent` (795), `nativeEvent` (804), `recent` (836), `add_recent` (842).
  - Workers: `_warm_ok` (848), `prestart_worker` (854), `_drop_warm` (864), `open_file` (873), `set_busy` (956), `_leave_empty_3d` (962), `poll_worker` (979), `_cleanup_tmp` (989), `cancel_load` (997), `on_worker_finished` (1016).
  - 2D: `set_mode` (1050), `open_drawing` (1069), `on_drawing_failed` (1084), `on_drawing_loaded` (1091), `render_drawing` (1119), `refresh_layers` (1138), `build_layer_items` (1160), `on_layout_changed` (1183), `on_light_bg` (1191), `set_layers_visible` (1199), `show_drawing_info` (1213), `on_entity_picked` (1246).
  - 3D: `clear_model` (1271), `show_doc` (1309), `setup_mesh` (1354), `_mesh_ready` (1383), `build_tree_items` (1405), `update_status` (1500), `show_model_info` (1523), `show_props` (1542), `_mass_rows` (1573), `mass_props` (1609), `assign_material` (1642), `run_collisions` (1668), `update_tree_materials` (1721), `show_mass` (1733), `on_props_link` (1765), `on_tree_selection` (1789), `on_view_selection` (1836), `clear_selection` (1860), `on_item_changed` (1876), `hide_selected` (1905), `isolate_selected` (1918), `show_all` (1932), `toggle_transp_selected` (1961), `zoom_selected` (1978), `show_context_menu` (1993), `apply_filter` (2036), `toggle_edges` (2077), `toggle_wire` (2087), `set_measure` (2097), `on_entities` (2146), `update_section` (2229), `screenshot` (2244), `about` (2252), `dragEnterEvent` (2279), `dropEvent` (2291), `closeEvent` (2298).
  - Entry: `main()` (2313).

**2. UI (exact strings)**

*Toolbar "Nástroje" (631-709):*
- `Otevřít` (Ctrl+O). Tooltip `Otevřít STEP / IGES / STL / DXF / DWG`. Dropdown menu with recent files (`recent_btn`, 636-643).
- `Znovu načíst` (F5).
- `Vše` (F), tooltip `Zobrazit celý model / výkres`.
- `Na výběr` (Z), tooltip `Přiblížit na vybrané`.
- View buttons, tooltip `Pohled <name>  [<key> nebo <Shift+n>]`: `Iso` (0, Shift+7), `Přední` (1, Shift+1), `Zadní` (2, Shift+2), `Horní` (3, Shift+5), `Spodní` (4, Shift+6), `Levý` (5, Shift+3), `Pravý` (6, Shift+4).
- `Hrany` (E, checkable, on by default at 629), tooltip `Zobrazit hrany`.
- `Drátově` (W), tooltip `Drátový model`.
- `Perspektiva` (P), tooltip `Perspektivní promítání`.
- Section: label `" Řez: "`, combo `vyp.`, `X`, `Y`, `Z` (tooltip `Řezová rovina kolmá na osu`), slider 0 to 1000 (default 500, width 140), checkbox `otočit`.
- `měřítko` (checkbox, default on, `scale_bar`), tooltip `Měřítko v levém dolním rohu (jen v pravoúhlém promítání)`.
- 2D only: label `" Layout: "`, combo tooltip `Prostor modelu / výkresové listy (layouty)`, `bílé pozadí` (default off, `light_bg`), `pravítka` (default on, `rulers`).
- Separator, then `Měřit` (M, checkable; tooltip `3D: klikej na vrcholy, hrany a plochy – délky, Ø / R, plochy, vzdálenosti s ΔX/ΔY/ΔZ, úhly. 2D: vzdálenost dvou bodů`), `Úhel` (U, checkable), `Hmotnost` (G), `Kolize` (K), `Snímek` (Ctrl+S).
- Right side: `Nápověda` (F1).

*Actions not on the toolbar (reachable by shortcut, context menu or the properties panel):*
- `Materiál…` (no shortcut), `Průhledné` (T), `Kopírovat výsledek měření` (Ctrl+C), `Skrýt` (H), `Jen vybrané` (I), `Zobrazit vše` (A), `Zrušit výběr` (Esc), `Rozbalit vše`, `Sbalit vše`.
- Note: `a_material` and `a_transp` are in the `o3` list (648) but are not added to the toolbar, so their visibility toggles have no visible effect.

*Recent-files dropdown (771-788):*
- `(žádné nedávné soubory)` when empty. Items are basenames, tooltips are full paths. Then `Vymazat mezipaměť načtených modelů`, `Náhledy v Průzkumníku…` (Windows), `3D myš (3Dconnexion)…` (when a SpaceMouse object exists).

*Context menus:*
- 3D (2014-2031): `Na výběr`, `Skrýt`, `Jen vybrané`, `Průhledné`, `Zobrazit vše`, `Materiál…`, `Hmotnost`, `Kolize`, `Kopírovat název` (when selection), `Rozbalit vše`, `Sbalit vše`, `Vše`.
- Measure mode, distance (1995-2000): `Ponechat měření a měřit dál  (Enter)`, `Kopírovat výsledek měření`, `Vyčistit výběr`, `Smazat vše a ukončit měření  (Esc)`.
- 2D (2002-2012): `Skrýt hladinu`, `Jen tato hladina`, `Zobrazit všechny hladiny`, `Na výběr`, `Vše`.

*Status and progress:*
- `Načítám <file>…` (886), `Zrušit` (719), `Spouštím načítání…` (985), `Zobrazuji model…` (1030), `Vykresluji…` (1108), `Žádný model` (1512).
- `<file>: <last log line>  (N s)` (987). `"Otevři model nebo výkres (Ctrl+O), nebo ho sem přetáhni."`. Empty-state text `"3D: STEP, IGES, STL   ·   2D: DXF, DWG"` (473-474).
- 3D status: `<file>  |  N dílů (M unikátních), K sestav  |  rozměr X × Y × Z mm  |  načteno za N s (z mezipaměti)` (1516-1520).
- 2D status: `<file>  |  <version>  |  N entit, M hladin  |  rozměr W × H <units>  |  načteno za N s (DWG z mezipaměti)` (1504-1508).
- Window title `<file> – CAD prohlížeč`. Tree headers `Název` / `Info` (3D) or `Hladina` / `Entit` (2D). Filter placeholders `Hledat ve stromu…` and `Hledat hladinu…`.
- Layer tooltip `Ve výkresu vypnutá / zmrazená hladina`. Tree tooltips `Sestava: …`, `Díl: …`, `Použit N× v modelu`, `Instance: …`. Count text `N ks`, `×N`.
- Measure hints (2116-2123): `Měřit: klikej na vrcholy, hrany a plochy (další klik přidá, opakovaný odebere, prázdno = vyčistit), Shift+klik = bod na ploše, Enter = ponechat, Ctrl+C = kopírovat, Esc = konec`; `Vzdálenost: klikni na 2 body – přichytí se k vrcholům / bodům geometrie, Shift = vodorovně / svisle.  Měření zůstávají, Esc = smazat a konec`; `Úhel: klikni na konec 1. ramene, vrchol a konec 2. ramene.  Esc = smazat a konec`.
- Measure result text: `Vzdálenost: … mm (ΔX …, ΔY …, ΔZ …)` (2220-2224, 2197-2198); `Úhel: …°` (2209); `Výsledek měření zkopírován do schránky` (2144); `Řez X = … mm` (2242); `Uloženo: <path>` / `Snímek se nepodařilo uložit` (2250).
- Help text in the properties panel for the measure tool (2152-2161, 2170-2171) and the footer `"Enter = ponechat · Ctrl+C = kopírovat"` (2206).
- Selection hint `"Těžiště zobrazeno modrou tečkou (T). Esc nebo nový výběr = skrýt"` (1753).

*Properties panel labels (exact): `Název`, `Díl / dokument`, `Sestava`, `Typ` (`sestava`/`díl`), `Obsahuje` (`N dílů, M přímých položek`), `Počet v modelu`, `Rozměr X×Y×Z`, `Min (X, Y, Z)`, `Max (X, Y, Z)`, `Barva`, `Objem`, `Povrch` (`≈ … cm²`), `Materiál`, `Hmotnost`, `Těžiště X; Y; Z`, `Pozor`, `Hmotnost (ocel 7,85)`, `Hmotnost (Al 2,70)`, `Těžiště objemu`, `Těles / ploch`, `Hmotnostní vlastnosti`, `Rozsah`, `Vybráno`, `Složka`, `Velikost`, `Verze`, `Jednotky`, `Layout`, `Entit`, `Hladin / bloků`, `Rozměr výkresu`, `Typy entit`, `Upozornění`, `Obrázky`, `Inventor`, `Převod DWG`, `Opraveno chyb`, `Načteno za`, `Dílů`, `Sestav`, `Načteno za:`, `Měření`.*
- Warning texts: `"<n>× 3D těleso (ACIS) – v 2D náhledu se nezobrazí"`, `"<n>× obrázek ze síťové cesty nenačten (bezpečnost) – zobrazen jen rámeček"`, `"<n>× pohledů převedeno"` (`Inventor`), `"mezipaměť"` (DWG converter label).
- Mass: `"<n> dílů bez materiálu"`, `"bez materiálu"`, `"přiřadit…"`, `"spočítat (N unikátních dílů)"`, `"Materiály…"`. The mass fallback densities are hard-coded (see §4).
- Layer-from-material shortname: `short = name.split(" (")[0].split(" /")[0]`, lowercase with `*` when the density came from the default (`main.py:1729-1730`).

*Dialogs and messages (exact):*
- Open dialog: title `Otevřít model / výkres`, filter `Všechny CAD soubory (*.step *.stp *.iges *.igs *.stl *.dxf *.dwg);;3D modely (*.step *.stp *.iges *.igs *.stl);;Výkresy (*.dxf *.dwg);;Všechny soubory (*.*)`.
- Save dialog title `Uložit snímek`, filter `PNG (*.png)`.
- `Soubor neexistuje:\n{path}` (876). `Nepodařilo se načíst model:\n{ex}` (921, 1041). `Nepodařilo se spustit načítání:\n{ex}` (943). `Nepodařilo se načíst model:\n` + stderr line or `načítací proces skončil chybou (kód {code})` (1026-1027). `Nepodařilo se načíst výkres:\n{msg}` (1089).
- Unexpected error (2321-2328): `Neočekávaná chyba:\n\n` + the last 1800 characters of the traceback.
- Cache clear menu item `Vymazat mezipaměť načtených modelů`.
- About box (2252-2276): `CAD prohlížeč <VERSION>`, description `Prohlížení STEP / IGES / STL modelů a DXF / DWG výkresů.`, mouse and key summary.
- Help key line (welcome 742-744): `F vše · Z na výběr · H skrýt · I jen vybrané · A vše zobrazit · T průhledné · E hrany · W drátově · P perspektiva · M měřit (vrcholy, hrany, plochy) · G hmotnost a těžiště · K kolize · 0–6 nebo Shift+1…7 pohledy · F5 znovu načíst · F1 nápověda`.

*Shortcut map (global, `_act` at 568-580, `WindowShortcut`):* Ctrl+O, F5, F, Z, 0 to 6, Shift+1 to Shift+7, E, W, P, M, U, G, K, Ctrl+S, H, I, A, T, Esc, F1, Ctrl+C (copy measure). Tooltips append `[shortcut]`.

*Mouse and keys by view (3D in §7; 2D below):*
- 2D: LMB click selects the entity (`entityPicked`). Empty space clears. LMB drag pans through scroll bars (threshold 4 px). Wheel zooms ×1.2 per notch under the cursor. Double-click zooms to the selection. RMB opens the context menu. Measure mode: LMB clicks place points, Shift constrains to horizontal or vertical. Snap within 10 px.
- Tree: double-click calls `zoom_selected`. Checkbox per node toggles visibility (`on_item_changed`).
- Drag and drop: `dragEnterEvent` accepts URLs with `ALL_EXT`, pre-starts a worker for CAD files or prewarms ezdxf for drawings (2279-2289). `dropEvent` opens the first accepted file (2291-2296).
- Props panel links: `mat:<idx>` opens the material dialog, `mat:all` opens it for the default, `calc:<idx>` computes mass for an assembly (1765-1783).

**3. OCCT used by `main.py`:** `AIS_ConnectedInteractive`, `AIS_Shaded`, `AIS_WireFrame`, `AIS_Shape`, `Bnd_Box`, `BRepBndLib`, `XCAFDoc_ShapeTool`, `gp_Pnt`, all via `load_occ()` (266-281) and direct imports at 1388 and 1629. Everything else is in `core`, `analysis` and `viewer3d`.

**4. Constants and algorithms (preserve)**
- Loading thresholds: CAD files ≥ 1 MiB get a cache copy (`CACHE_MIN`). Files ≥ `WORKER_THRESHOLD` (same value) load in a worker process (912). Small files load in process unless an idle worker is waiting (912). Cache check first (891-906).
- Worker: `WORKER_IDLE_S = 120` (48). A warm worker is valid only with `time.time() - start < 105` (852). Progress poll every 150 ms (950).
- Cache key: `sha1("<normcase abspath>|<size>|<mtime_ns>|1.9") + ".xbf"` (112-115). Partial file `<name>.tmp.xbf` (119). Log file `<dst>.log`. Job file `job_<pid>_<n>.tmp`, written as `.w` then `os.replace` (139-142).
- `job_valid` (51-65): absolute source path; destination must end `.xbf`, be absolute and inside `CACHE_DIR` (realpath, commonpath), and not equal the directory.
- Pruning (349-371): files older than 6 h that contain `.tmp.` or end with `.log`, `.tmp`, `.w` are removed. Old `dwg_*` directories are removed. Then `.xbf` and `.dxf` (not `.tmp.`) are kept in mtime order up to `CACHE_LIMIT = 3 GiB`. Runs 5 s after start (2330), after every load (1045, 1093), and in `store_cache`.
- Cache hit refreshes mtime (`os.utime`, 898), which is the LRU ordering.
- Model recent list: max 12 entries (`r[:11]` + the new one, 844). `lastdir` is stored (845).
- Tree expand rule (1459-1466): when there is one root, expand top-level and the first two levels when counts are ≤ 30 and ≤ 15.
- `DrawingLoader` and cancel (997-1010). Thread cannot be stopped: `closeEvent` calls `os._exit(0)` if the loader is still running after 1.5 s (2307-2310).
- Assembly limits: enforced in `show_doc` (1311) through `count_instances` before anything is expanded.
- `show_doc` at line 1309 sets `_showing = True` to block nested loads.
- Mass (1609-1633): `vol` in mm³ from `analysis.proto_props`, `kg = vol × density(g/cm³) × 1e-6` (1625). Centre of gravity weighted by mass; volume centre when no density.
- Fallback densities in the mass rows: steel `7.85` (`vol × 7.85e-6` kg, 1587), aluminium `2.70` (1588).
- Mass rows use `cm³` = `vol/1000` and `cm²` = `area/100` (1574-1575).
- Section plane (2229-2242): slider 0 to 1000 mapped linearly over the model bounding box along X, Y or Z. Flip checkbox.
- `setup_mesh` runs only for a single STL leaf (1359).
- Measure snapping uses 7 px tolerance (see §7).
- Shortcuts for orientation: `set_orientation` is called only when `view` exists (`_space_button`, 826-830).
- `show_doc` builds the tree and sets the layer check boxes; layer toggles call `refresh_layers` after 150 ms debounce (482-483, 1881).
- Layer toggling in 2D uses `LayerMap` (no re-draw) when available (1881, 1143).
- Thumbnail settings are reached from the recent-files menu (786).
- `_space_motion` ignores 3D mouse input while a modal widget is active (814).

**5. Process model (main.py specifics; section 5 of the spec covers the rest)**
- Threads:
  - `DrawingLoader` (QThread) for DXF and DWG loading (392, 1075-1082). Waits for the ezdxf prewarm thread first (404).
  - `ezdxf-prewarm`, a daemon thread that only imports modules (`prewarm_ezdxf`, 186-209). Started at launch when the argument is a drawing (`EARLY_DRAWING`, 234-235, 256-257), from `open_dialog` (750), from drag enter (2288).
  - `stl-mesh`, a daemon thread for `meshmeasure.mesh_for_file` (1367-1381). Result via `meshReady` signal, so the GUI thread alone touches Qt and OCCT.
- Subprocesses (`subprocess.Popen`, `main.py:133-135`): stdin/stdout/stderr `DEVNULL`, `CREATE_NO_WINDOW` on Windows, `sys.executable` (frozen) or `sys.executable abs(main.py)`. Arguments `--worker-serve <job file>`.
- Worker protocol: JSON `[src, dst]` in the job file (`submit_job`). The worker deletes the job file (`_remove(job)`, 80), validates it (`job_valid`), runs `core.read_cad` with progress lines appended to `<dst>.log`, `core.save_xbf(doc, dst)`, then writes `DONE` (or `ERROR <msg>`) (29-45). The worker exits after one job. The parent checks `lines[-1] == "DONE"` and the exit code (1020).
- Cancel: `self._proc.kill()` then `wait(3)`, then cleanup of `.tmp` files (1002-1010). Warm worker kill in `_drop_warm` (864-871).
- XBF hand-over: the worker writes `<cache>.tmp.xbf` (`part_path`, 118-119; `save_xbf` writes it with `SaveAs` using BinXCAF format). The parent renames it to the final cache name with `os.replace` (1033) and loads it with `core.load_xbf` (1034). The XBF includes triangulations (`SetWithTriangles(True)`, core.py:45).
- In-process save of the cache for small models (≥ 1 MiB but loaded in process): `QTimer.singleShot(0, store_cache)` (926), which runs in the GUI thread.
- Environment variables read by the GUI process:
  - `OPENBLAS_NUM_THREADS` set to `"1"` if unset (13).
  - `STEPVIEW_WORKER_MIN` (20), `LOCALAPPDATA` (23), `XDG_CACHE_HOME` (25), `QT_QPA_PLATFORM` (2314-2315: set to `xcb` on Linux when absent).
  - `STEPVIEW_SHOT`, `STEPVIEW_SHOT_DELAY`, `STEPVIEW_SELECT` (testing, 2335-2353).
  - `ProgramFiles`, `ProgramFiles(x86)`, `ProgramW6432`, `LOCALAPPDATA` for install-location checks (thumbsettings.py:53-54, view2d.py:48).
  - `SystemRoot` for `cleanmgr.exe` (thumbsettings.py:154).
- Stale temp files at startup: `QTimer.singleShot(5000, prune_cache)` (2330).
- Single-instance: none. No `QLocalServer`, `QSharedMemory` or `QLocalSocket` in the code (grep verified). Every file open starts a new process.
- Sandboxing: none in the interactive GUI. Thumbnail process only (sections 13 and 15).
- Crash handling: `sys.excepthook` shows a message box (2321-2328).
- Settings file location: `QSettings("CadProhlizec", "StepViewer")` (418). On Windows it is `HKCU\Software\CadProhlizec\StepViewer` (the installer deletes this key on uninstall, `iss:152-153`). On Linux the file location is Qt's default; the code does not set one.

**6. Dependencies (main.py)**
- Imports `materials` and `view2d` at top level (252-254). Imports `spacemouse` on Windows (421), `thumbsettings` (791), `spacemouse` settings dialog (833), `collisions` (1671), `meshmeasure` and `viewer3d` (1362-1363), `analysis` and `viewer3d` through `load_occ` (271-273), `core` through `load_occ` and `main.py`-level worker code (`core` imported in workers at 37 and 71).
- Module dependency map is in §15.

**Settings keys (QSettings, complete list):**
- `geometry` (QByteArray, 531, 2303), `split` (state, 536, 2304).
- `recent` (list, max 12, 763, 837, 844).
- `lastdir` (751, 845, 2245).
- `scale_bar` (bool, 675, 2212). `light_bg` (bool, 687, 1192). `rulers` (bool, 692, 2217).
- `msaa` (read only, `viewer3d.py:70`).
- `spacemouse/enabled`, `spacemouse/rotate`, `spacemouse/pan`, `spacemouse/dominant` (bool), `spacemouse/speed` (float, 0.1 to 10), `spacemouse/invert0` to `invert5` (spacemouse.py:156-178).
- Non-QSettings persistence: materials JSON, cache files, Windows registry entries for thumbnails, installer registry entries (section 14 `[Registry]`).

---

## 15. `shellext/StepThumb.cs` and `shellext/Sandbox.cs`: how the app is invoked

**1. Purpose and API**
- `namespace StepThumb`, COM class `StepThumbnailProvider` (`StepThumb.cs:38-39`), CLSID `{7A1F3C52-8E0B-4C6D-9F2A-5B3E1D4C6A70}` (line 38). Implements `IInitializeWithFile` (path), `IInitializeWithStream` (copy), and `IThumbnailProvider` (`GetThumbnail`, line 76). IID of IThumbnailProvider `{e357fccd-a995-4576-b01f-234630154e96}` (32).
- `Sandbox.Run(exe, args, workDir, timeoutMs, log)` (`Sandbox.cs:44`) returns the exit code or -1 on timeout.

**2. UI:** none. Diagnostics only: `%TEMP%\StepViewerThumbs.log`, written only if the file already exists (`StepThumb.cs:65-74`).

**3. OCCT:** none in the DLL.

**4. Constants and behaviour (preserve)**
- Max input: `MaxBytes = 80 MiB` (49), checked from `Stat` and again while copying (98, 154).
- Timeout: 60 000 ms (50, 113).
- Concurrency gate: named semaphore `Local\StepViewerThumbnails`, count 2 (54). Wait for it with the same timeout (109).
- Size: `Math.Max(32u, Math.Min(cx, 1024u))` passed as `%s` (113).
- Command line: `"<dir>\StepViewer.exe" --thumbnail "<src>" "<png>" <size>` (107-113). The exe is looked for next to the DLL (107-108).
- Extension check for stream input: `.step .stp .iges .igs .stl .dxf .dwg`, otherwise `.step` (100-101).
- Output: PNG, read once into memory, max 4 MiB (`MaxPngBytes`, 165). Max dimension `min(2048, 2 × size)` (166, 228). IHDR must be first, every chunk must be inside the file, IEND must be last (196-215). Size must equal IHDR (235). Converted to top-down 32-bit ARGB DIB (236-245). `pdwAlpha = 2` (`WTSAT_ARGB`, line 79).
- Input copy for streams: `LocalLow\StepViewerThumbs\<guid><ext>` (`WorkDir`, `Sandbox.cs:18-36`).

*Sandbox (Sandbox.cs):*
- Creates a job object with `KILL_ON_JOB_CLOSE`, `PROCESS_MEMORY` (2 GiB, line 16), `ActiveProcessLimit = 4` (renderer and DWG converter, 110), `DIE_ON_UNHANDLED_EXCEPTION`, `ACTIVE_PROCESS` (108-109).
- UI restrictions `0xFF` total: no clipboard, global atoms, system parameters, display settings, desktops, exit windows, USER handles outside the job (126-127).
- Renderer runs with a Low integrity token (`S-1-16-4096`, line 172), created with `CreateProcessAsUser` suspended (161, 186), assigned to the job, then resumed (57-63). Fails closed: a process that is not confined is terminated (83-84).
- Environment rebuilt: `TEMP`, `TMP`, `XDG_CACHE_HOME` set to the work dir, `QT_QPA_PLATFORM=offscreen` (209-212).
- Timeout: `TerminateJobObject(job, 1)` and return -1 (72-73).

**5. Process model:** the DLL is loaded into Explorer's thumbnail host. For each file it spawns a sandboxed `StepViewer.exe --thumbnail` process (Low IL, job object, 2 GiB, 4 processes, 60 s). Temporary input (for streams) and PNG output live in `LocalLow\StepViewerThumbs`. Both are deleted after use (`TryDelete`, 133-134, 193).

**6. Dependencies:** calls only `StepViewer.exe --thumbnail` (the CLI contract in section 14). Registered by the installer (`iss:90-112`) and by `thumbsettings.py` (section 12). `Sandbox` is used only by `StepThumb.cs`.

---

## 16. `linux/install.sh` (105 lines): app-facing contract on Linux

**1. CLI:** `./install.sh` (per user, `~/.local`), `sudo ./install.sh --system` (`/usr/local`), `--default` (sets defaults via `xdg-mime`, line 88-91), `--dwg-thumbs` (adds `image/vnd.dwg` to thumbnail types, 38), `--uninstall`, `-h|--help`.

**2. UI:** Czech console messages (lines 56-104), for example `"Hotovo. Spuštění: cad-viewer (nebo z nabídky aplikací „CAD prohlížeč“)."`.

**3. OCCT:** none.

**4. App-facing paths and types**
- Application directory `APPDIR` = `$HOME/.local/lib/cad-viewer` or `/usr/local/lib/cad-viewer` (27, 30).
- Launcher symlink `BIN/cad-viewer` (66). Desktop file `$SHARE/applications/cad-viewer.desktop` with `Exec=$APPDIR/cad-viewer %f` (67). MIME file `cad-viewer-mime.xml` (68). Icon `cad-viewer.png` at `hicolor/256x256/apps` (69).
- Types opened (line 34): `model/step model/iges model/stl application/sla image/vnd.dxf image/vnd.dwg`.
- Thumbnail types (line 37): same without DWG (DWG only with `--dwg-thumbs`).
- Thumbnailer file `$THUMBDIR/cad-viewer.thumbnailer` (80-85): `Exec=${WRAP}$APPDIR/cad-viewer --thumbnail %i %o %s`, `MimeType=` the thumbnail types.
- Wrappers: `timeout -k 5 60` and `prlimit --as=4000000000 --cpu=120` when found (75-79). Explanation of the 4 GB limit at 73-74.
- Uninstall removes `${XDG_CACHE_HOME:-$HOME/.cache}/cad-viewer` (50).

**5. Process model:** the thumbnailer runs the app with `--thumbnail` for every file in a folder, wrapped in `timeout` and `prlimit`. The app adds its own `RLIMIT_AS` 4 GiB and `RLIMIT_CPU` 120 s (`main.py:153-175`).

**6. Dependencies:** uses the CLI contract `cad-viewer [file]` and `cad-viewer --thumbnail <in> <out> <size>`.

---

## 17. `installer/StepViewer.iss` (163 lines): app-facing contract on Windows

**1. CLI:** `StepViewer.exe "%1"` for file associations (lines 68, 71, 73). `StepViewer.exe` is run after install with `postinstall` (115).

**2. UI strings (Czech, exact):**
- Tasks: `"Vytvořit zástupce na ploše"`, `"Náhledy modelů a výkresů v Průzkumníku Windows (ikony souborů)"`, `"i náhledy DWG (převodník LibreDWG – méně odolný vůči poškozeným / podvrženým souborům)"` (unchecked by default), `"Otevírat 3D modely (.step .stp .iges .igs .stl) dvojklikem"`, `"Otevírat výkresy (.dxf .dwg) dvojklikem"` (unchecked by default). Group labels `"Další zástupci:"`, `"Průzkumník:"`, `"Asociace souborů:"`.
- Run: `"Spustit CAD prohlížeč"`.
- Icon and name: `{#AppName}` = `CAD prohlížeč`. Publisher `CadProhlizec`.
- Uninstall: `UninstallDisplayName` = `CAD prohlížeč`.
- Registered description strings: `"3D model (STEP / IGES / STL)"`, `"Výkres (DXF / DWG)"`, `"CAD prohlížeč – náhledy"`.

**3. OCCT:** none.

**4. App-facing contract (exact)**
- `AppId={6E3B2F0C-6C55-4B7E-9A4D-2F1D8C5A7B10}` (line 15). Same as 1.0, so it is an upgrade (14).
- `DefaultDirName={autopf}\StepViewer` (20). `PrivilegesRequired=lowest` with dialog override (24-25). Install to `Program Files` only when the user chooses all users.
- `OutputBaseFilename=CadProhlizec_setup_{#AppVersion}` (29), output in `..\dist`.
- Files: `..\dist\StepViewer\*` (55), `StepThumb.dll` replaced at reboot if locked (57). Licences into `{app}\licenses` (58).
- Shortcuts: `{autoprograms}\CAD prohlížeč` and optionally `{autodesktop}` (61-62).
- ProgIds: `StepViewer.Model` (STEP/IGES/STL) and `StepViewer.Drawing` (DXF/DWG). Command `"{app}\StepViewer.exe" "%1"` (68, 71). `Applications\StepViewer.exe` with `FriendlyAppName` (72-73). All under `HKA`.
- Associations on `.step .stp .iges .igs .stl` (task `assoc`), `.dxf .dwg` (task `assoc2d`) (83-89), plus `OpenWithProgids` always (75-81).
- Thumbnail registration: `HKA\Software\Classes\SystemFileAssociations\<ext>\ShellEx\{IID}` = CLSID for each extension (98-104). `thumbs\dwg` task for `.dwg` (104). Removed when the task is unticked (106-112).
- `InprocServer32` values (92-97): `mscoree.dll`, `ThreadingModel=Both`, `Class=StepThumb.StepThumbnailProvider`, `Assembly=StepThumb, Version=1.3.0.0, Culture=neutral, PublicKeyToken=null`, `RuntimeVersion=v4.0.30319`, `CodeBase=file:///{app}/StepThumb.dll` (`ThumbCodeBase`, 127-134).
- `InstallDelete` removes the old 1.0 shortcuts `STEP prohlížeč.lnk` (51-52).
- Uninstall (117-120): deletes `{localappdata}\StepViewer\cache` and `{localappdata}Low\StepViewerThumbs`. Code section (138-163) removes `HKCU\Software\CadProhlizec\StepViewer`, `HKCU\Software\CadProhlizec` if empty, the CLSID key, and the per-user thumbnail association for each extension.
- Compression `lzma2/max`, `SolidCompression=yes`, `CloseApplications=yes`, `ChangesAssociations=yes`.

**5. Process model:** none. Installer only.

**6. Dependencies:** requires the app to be installed at `{autopf}\StepViewer` (see `thumbsettings.installed_dir`, which checks the same locations). Files from `..\dist\StepViewer\*`, `licenses\*`, `..\stepview\stepview.ico`. Registry locations match `thumbsettings.py` (section 12).

---

## 18. Cross-module dependency map

```
main.py ──┬─> materials.py              (no stepview deps)
          ├─> view2d.py ──> fastscene.py ──> qtbuild.py
          │                   └─(native) rdxf (native/rdxf/src/lib.rs)
          ├─> spacemouse.py           (no stepview deps)
          ├─> thumbsettings.py        (no stepview deps)
          ├─> (lazy, 3D) core.py, analysis.py, viewer3d.py
          │      analysis.py ──> (OCP only)
          │      core.py ──> (OCP only)
          │      viewer3d.py ──> (OCP only; PickItem duck-types meshmeasure.Mesh, analysis.Entity)
          ├─> collisions.py ──> analysis.py
          └─> meshmeasure.py          (numpy only)

thumbs.py (--thumbnail)  ──> core.py, viewer3d.py (_capsule, leaf_objects, rgb), view2d.py
StepThumb.cs (Explorer DLL) ──> launches StepViewer.exe --thumbnail (via Sandbox.cs)
```

- Module import relationships in one list:
  - `main.py` imports: `materials`, `view2d` (top level); `spacemouse` (Windows); `thumbsettings`, `collisions`, `meshmeasure`, `viewer3d`, `analysis`, `core` (lazy).
  - `view2d.py` imports: `fastscene` (lazy, inside `render_scene`); ezdxf (lazy).
  - `fastscene.py` imports: `qtbuild` (top level); `rdxf` (optional, top level in a try).
  - `thumbs.py` imports: `core`, `viewer3d`, `view2d` (all lazy inside functions).
  - `collisions.py` imports: `analysis`.
  - `analysis.py`, `core.py`, `viewer3d.py`, `meshmeasure.py`, `materials.py`, `spacemouse.py`, `thumbsettings.py`, `qtbuild.py` import no other stepview module.

---

## Appendix A. OCCT inventory by purpose (for a minimal C++ bridge)

These names are the OCP (Python) names, which map 1:1 to OCCT C++ classes and methods.

**A. Application and XCAF document (core.py 5-30, 40-55)**
- `XCAFApp_Application.GetApplication_s()`, `app.MessageDriver()`, `app.DefineFormat(name, desc, "xbf", retr, store)`, `app.InitDocument(doc)`, `app.SaveAs(doc, path)` (status string checked for `"OK"`).
- `TDocStd_Document(format)`, `doc.Main()`.
- `BinXCAFDrivers_DocumentStorageDriver().SetWithTriangles(msgdriver, True)`, `BinXCAFDrivers_DocumentRetrievalDriver().Read(path, doc, app)`, `.GetStatus()`.

**B. Import (core.py 81-111)**
- `STEPCAFControl_Reader`: `SetColorMode`, `SetNameMode`, `SetLayerMode`, `SetPropsMode`, `SetGDTMode`, `SetMatMode`, `SetViewMode`, `ReadFile`, `SetShapeFixParameters(DataMap)`, `Transfer(doc)`.
- `IGESCAFControl_Reader`: `SetColorMode`, `SetNameMode`, `SetLayerMode`, `ReadFile`, `Transfer`.
- `IFSelect_RetDone`. `DataMap_TCollection_AsciiString_TCollection_AsciiString`.
- `TCollection_AsciiString`, `TCollection_ExtendedString`.

**C. STL import (core.py 118-129; viewer3d.py 158-185)**
- `RWStl.ReadFile_s(path, angle)`, `RWStl.ReadFile_s(path)`, `tri.NbTriangles()`, `tri.NbNodes()`, `tri.Node(i)`, `tri.Triangle(i).Get()`, `tri.ComputeNormals()`.
- `BRep_Builder().MakeFace(face, tri)`. `TopoDS_Face()`.

**D. XCAF tree and attributes (core.py 176-415; main.py; thumbs.py)**
- `XCAFDoc_DocumentTool.ShapeTool_s(main)`, `ColorTool_s(main)`, `GetLengthUnit_s(doc, 0.0)` (unused).
- `XCAFDoc_ShapeTool`: `GetShapes(seq)`, `GetFreeShapes(seq)`, `IsAssembly_s(lab)`, `IsReference_s(lab)`, `GetReferredShape_s(lab, proto)`, `GetLocation_s(lab)`, `GetComponents_s(proto, comps, False)`, `GetShape_s(lab)`, `AddShape(face, False)`.
- `XCAFDoc_ColorTool.GetColor_s(lab, XCAFDoc_ColorSurf | XCAFDoc_ColorGen, Quantity_Color)`. `XCAFDoc_ColorSurf`, `XCAFDoc_ColorGen`. `Quantity_Color` `Red()`, `Green()`, `Blue()`.
- `TDataStd_Name`: `Set_s(lab, name, True)`, `GetID_s()`, `lab.FindAttribute(id, attr)`, `attr.Get().ToExtString()`.
- `TDF_Label`, `TDF_Tool.Entry_s(lab, s)`, `Sequence_TDF_Label`.
- `XCAFPrs_AISObject(label)` and `SetLocalTransformation`. `XCAFPrs_AISObject(proto)` for shared prototypes.

**E. Topology utilities (core.py 132-147; analysis.py; viewer3d.py; thumbs.py)**
- `TopExp.MapShapes_s(shape, TopAbs_FACE, map)`, `IndexedMap_TopoDS_Shape_TopTools_ShapeMapHasher` (`_ShapeMap`), `.Extent()`.
- `TopExp_Explorer(shape, type)` with `More()`, `Next()`, `Current()`.
- `TopoDS.Face()`, `TopoDS.Edge()`, `TopoDS.Vertex()`, `TopoDS_Compound`, `TopoDS_Face`.
- `BRep_Builder`: `MakeCompound`, `Add`, `MakeFace`.
- `TopLoc_Location`: `Multiplied`, `IsIdentity`, `Transformation`. Shape `Moved(loc)`, `Transformed`, `ShapeType()`, `IsNull()`, `Orientation()`, `IsSame()`, `HasTransformation()`, `Transformation()`, `HasShape()`.
- Enums: `TopAbs_FACE`, `TopAbs_EDGE`, `TopAbs_VERTEX`, `TopAbs_SOLID`, `TopAbs_REVERSED`.

**F. Bounding boxes (core.py 58, 187-191; analysis.py 397-402; main.py)**
- `Bnd_Box`: `CornerMin()`, `CornerMax()`, `IsVoid()`, `Enlarge(gap)`.
- `BRepBndLib.Add_s(shape, box, False)` (core), `BRepBndLib.AddOptimal_s(shape, box, True, False)` (analysis, main).

**G. Meshing (core.py 138-222; viewer3d.py 1061-1063)**
- `BRepMesh_IncrementalMesh(shape, defl, relative, angle, parallel)` and `BRepMesh_IncrementalMesh(shape, IMeshTools_Parameters)`.
- `IMeshTools_Parameters`: `Deflection`, `Angle`, `Relative`, `InParallel`, `MeshAlgo`. `IMeshTools_MeshAlgoType_Delabella`.
- `BRepTools.Triangulation_s(shape, tol, bool)`. `BRep_Tool.Triangulation_s(face, loc)` (presence test). `BRep_Tool.Surface_s(face)` (mesh-only test). `BRep_Tool.Pnt_s(vertex)`.
- `Poly_Triangulation` (`NbNodes`, `NbTriangles`, `Node`, `Triangle`).

**H. Mass properties (analysis.py 353-372; 445-447)**
- `BRepGProp.VolumeProperties_s(shape, gprops, skip, use_triangulation, mesh)` and `(shape, gprops)`. `BRepGProp.SurfaceProperties_s(shape, gprops, skip, use_triangulation)` and `(face, gprops)`.
- `GProp_GProps`: `Mass()`, `CentreOfMass()`.

**I. Measuring and geometry evaluation (analysis.py 67-124, 266-282, 296-332; viewer3d.py 201-209)**
- `BRepAdaptor_Curve(edge)`: `GetType()`, `Value(u)`, `FirstParameter()`, `LastParameter()`, `Line()`, `Circle()`, `Ellipse()`.
- `BRepAdaptor_Surface(face)`: `GetType()`, `Plane()`, `Cylinder()`, `Cone()`, `Sphere()`, `Torus()`.
- `GeomAbs_*` enums: `Line`, `Circle`, `Ellipse`, `BSplineCurve`, `Plane`, `Cylinder`, `Cone`, `Sphere`, `Torus`, `BSplineSurface`. (`GeomAbs_C0` imported but unused.)
- `GCPnts_AbscissaPoint.Length_s(curve)`.
- `gp_Pnt`, `gp_Vec` (`Crossed`, `Dot`, `Angle`, `Magnitude`, `Distance`), `gp_Dir` (`Reversed`), `gp_Ax1`, `gp_Pln`, `gp_Trsf` (`SetRotation`, `SetTranslation`, `Multiplied`).
- `BRepExtrema_DistShapeShape(a, b)`: `IsDone()`, `NbSolution()`, `Value()`, `PointOnShape1(i)`, `PointOnShape2(i)`.
- `GeomAPI_ProjectPointOnSurf(p, surf)`: `NbPoints()`, `LowerDistanceParameters()`. `GeomLProp_SLProps(surf, u, v, 1, 1e-6)`: `IsNormalDefined()`, `Normal()`.

**J. Booleans and interference (analysis.py 426-454)**
- `BRepAlgoAPI_Common()`: `SetArguments`, `SetTools`, `SetRunParallel(True)`, `SetFuzzyValue(1e-4)`, `Build()`, `IsDone()`, `Shape()`.
- `List_TopoDS_Shape` (aliased `TopTools_ListOfShape`).

**K. Overlay construction (analysis.py 140-144; viewer3d.py 143-155, 438, 1041-1048, 1104-1107)**
- `BRepBuilderAPI_MakeVertex(p).Vertex()`, `BRepBuilderAPI_MakeEdge(a, b).Edge()`, `BRepBuilderAPI_MakePolygon` (`Add`, `Close`, `Wire`).

**L. Display, view and selection (viewer3d.py 9-45, 103-107, 220-239, 249-1148; thumbs.py 55-63, 75-117)**
- Driver and viewer: `OpenGl_GraphicDriver(display, bool)` with `ChangeOptions().swapInterval` and `InitContext()`. `Aspect_DisplayConnection()`. `V3d_Viewer(driver)`: `SetDefaultLights()`, `SetLightOn()`, `CreateView()`.
- View: `V3d_View`: `SetProj(V3d_TypeOfOrientation_Zup_*)`, `FitAll(box or margin, bool)`, `ZFitAll()`, `SetWindow(win)`, `MustBeResized()`, `Redraw()`, `Invalidate()`, `SetBgGradientColors(top, bottom, Aspect_GradientFillMethod_Vertical, False)`, `SetBackgroundColor(c)`, `ChangeRenderingParams()` (`NbMsaaSamples`, `IsAntialiasingEnabled`), `Camera()`, `AddClipPlane`, `RemoveClipPlane`, `Pan(dx, dy, 1.0, True)`, `Convert(px)`, `ToPixMap(pm, w, h, Graphic3d_BT_RGB)`, `Dump(path)`.
- Window: `WNT_Window(capsule)` (Windows) and `Xw_Window(display, winid)` (Linux): `SetVirtual(True)`, `DoResize()`, `Map()`, `IsMapped()`.
- Camera: `Graphic3d_Camera`: `Projection_e` (`Projection_Perspective`, `Projection_Orthographic`), `SetProjectionType`, `ProjectionType`, `SetScale`, `Scale`, `SetCenter`, `Center`, `SetEye`, `Eye`, `Direction`, `Up`, `Transform(trsf)`, `Project(p)`, `UnProject(p)`.
- Clip planes: `Graphic3d_ClipPlane(pln)`: `SetCapping(True)`, `SetUseObjectMaterial(True)`, `SetCappingHatchOff()`, `SetEquation(pln)`. `Graphic3d_SequenceOfHClipPlane().SetOverrideGlobal(True)`.
- Interactive context: `AIS_InteractiveContext(viewer)`: `Display(obj, mode, selmode, update)`, `Erase`, `Remove`, `Activate(obj, mode)`, `Deactivate(obj, mode)`, `IsDisplayed`, `SetDisplayMode`, `SetTransparency`, `UnsetTransparency`, `RecomputePrsOnly`, `MoveTo(x, y, view, bool)`, `HasDetected()`, `DetectedOwner()`, `DetectedInteractive()`, `ClearDetected(bool)`, `SelectDetected(scheme)`, `AddOrRemoveSelected(obj, bool)`, `ClearSelected(bool)`, `InitSelected()`, `MoreSelected()`, `SelectedInteractive()`, `NextSelected()`, `MainSelector()` (`NbPicked`, `PickedPoint(1)`), `UpdateCurrentViewer()`, `SetPixelTolerance(n)`, `HighlightStyle()`, `SelectionStyle()` (`SetColor`, `SetDisplayMode`), `DefaultDrawer()` (`SetAutoTriangulation`), `SetPixelTolerance`.
- Presentation objects: `AIS_Shape(shape)`: `SetColor`, `SetWidth`, `SetTransparency`, `SetZLayer`, `Attributes()`, `SetDynamicHilightAttributes`, `UnsetHilightAttributes`, `SetTransformPersistence`, `Transparency()`, `DisplayMode()`, `SelectionMode_s(type)`. `AIS_ConnectedInteractive()`: `Connect(prototype, trsf)`. `AIS_TextLabel()`: `SetText`, `SetPosition`, `SetHJustification`, `SetVJustification`, `SetColor`, `SetHeight`, `SetZLayer`. `AIS_ViewCube()`: `SetSize`, `SetFontHeight`, `SetBoxColor`, `SetBoxSideLabel`, `SetDuration`, `SetFitSelected`, `SetClipPlanes`, `SetTransformPersistence`, `HandleClick`, `UpdateAnimation(True)`. `AIS_ViewCubeOwner` (type test). `AIS_WireFrame`, `AIS_Shaded`, `AIS_SelectionScheme_Replace`, `AIS_SelectionScheme_XOR`.
- Presentation attributes: `Prs3d_Drawer()`: `SetLink`, `SetColor`, `SetDisplayMode`, `SetTransparency`. `Prs3d_LineAspect(color, Aspect_TOL_SOLID | Aspect_TOL_DASH, width)`. `Prs3d_PointAspect(Aspect_TOM_BALL, color, size)`. Attributes `SetWireAspect`, `SetFaceBoundaryDraw`, `SetFaceBoundaryAspect`, `SetPointAspect`.
- Enums: `Graphic3d_TMF_2d`, `Graphic3d_TMF_TriedronPers`, `Graphic3d_TransformPers`, `Graphic3d_HTA_CENTER`, `Graphic3d_VTA_BOTTOM`, `Graphic3d_ZLayerId_Top`, `Graphic3d_ZLayerId_TopOSD`, `Graphic3d_ZLayerId_Topmost`, `Graphic3d_BufferType_Graphic3d_BT_RGB`, `Aspect_TOTP_LEFT_LOWER`, `Aspect_TOTP_RIGHT_UPPER`, `Aspect_GradientFillMethod_Vertical`, `Aspect_DisplayConnection`. Unused import: `Graphic3d_NameOfMaterial_Plastified` (viewer3d.py:29).
- `Quantity_Color(r, g, b, Quantity_TOC_sRGB)`.
- `OCP.collections.Vec2_int` (aliased `Graphic3d_Vec2i`) for `TransformPers` offsets.

**M. Image export (thumbs.py 105-113; viewer3d.py 1148)**
- `Image_AlienPixMap()` with `Save(TCollection_AsciiString(path))`. `view.ToPixMap(pm, w, h, bufType)`. `view.Dump(path)`.

**N. Platform handles (thumbs.py 75-119; viewer3d.py 42-45, 330-334)**
- Windows: `WNT_Window` from an HWND through a PyCapsule, `CreateWindowExW`, `DestroyWindow`.
- Linux: `Xw_Window(display, winid)`.

---

## Appendix B. Observations (facts, not design)

These are things noticed while reading. They are not proposals.

1. Dead code: `core.py:240` `length_unit` is never called. `rdxf` `read_bulk` (`lib.rs:276`) is not used by the viewer, per its own header (line 5). `viewer3d.py:25` imports `GeomAbs_C0` and `viewer3d.py:29` imports `Graphic3d_NameOfMaterial_Plastified`, and neither is used.
2. `viewer3d.py:70` reads QSettings key `msaa`, but nothing in the code writes it. Only an env var or a manual settings edit changes it.
3. `main.py:648` puts `a_transp` and `a_material` in the `o3` list, but neither is added to the toolbar (they are not in the `tb.addAction` calls at 650-709). Reachable only by shortcut `T`, the context menu, or the properties-panel link. `Materiál…` has no shortcut.
4. `main.py:1587-1588` hard-codes steel `7.85` and aluminium `2.70` for the "no material" fallback. This is separate from the `materials.py` library.
5. `thumbs.py:356`: the remote-path pre-check (`remote_step_refs`) runs only for `.step` and `.stp`. IGES (`.iges`, `.igs`) is not pre-checked for external references, while `StepThumb.cs` accepts IGES (line 100). Keep this in mind when porting the file-type list.
6. `view2d.py:80-85` changes the process environment (`os.environ`) for the ODA call under `_ENV_LOCK`. Other threads reading the environment at the same time are not protected by this lock.
7. `main.py:2310`: `closeEvent` calls `os._exit(0)` when a loader thread refuses to stop within 1.5 s. Settings are saved first (2303-2305).
8. `thumbs.py:284` sets `QT_QPA_PLATFORM=offscreen` for the whole process, not just for Qt. `StepThumb.cs:212` also sets it for the child.
9. `thumbsettings.py:154`: `cleanmgr.exe` is launched by full path under `SystemRoot`, with a comment explaining it avoids exe planting.
10. `clear_cache` (`main.py:374-384`) removes files in the cache directory. The `materials/` subdirectory survives, because `_remove` silently ignores the `OSError` from removing a directory. The Inno uninstaller deletes the whole `cache` directory, materials included (`iss:118`).
11. `StepThumb.cs:54` uses a named semaphore `Local\StepViewerThumbnails` with count 2 for all thumbnail renders in the session. `StepThumb.cs:51-53` notes the accepted denial-of-service risk from a same-session process creating the name first.
12. `analysis.py:324` uses a 2° tolerance for non-plane pairs and 1e-4° for plane pairs. The docstring says mesh normals are noisy.
13. `fastscene.py` and `lib.rs` both implement `assign_levels`. The Python version is the reference. The two were compared line by line for the update rule and the `partition_point` lookup, and they match.
14. The project has no single-instance mechanism (see §14, section 5). Each Explorer double-click starts a new process, and each file load starts a new worker.
