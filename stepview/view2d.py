"""2D drawings (DXF / DWG) – loading via ezdxf, rendering into a QGraphicsView."""
import collections
import glob
import logging
import math
import os
import re
import subprocess
import sys
import time
import threading

from PySide6.QtCore import QPointF, QRect, QRectF, Qt, Signal
from PySide6.QtGui import QBrush, QColor, QFont, QPainter, QPainterPath, QPen
from PySide6.QtWidgets import (QGraphicsEllipseItem, QGraphicsLineItem, QGraphicsRectItem,
                               QGraphicsScene, QGraphicsSimpleTextItem, QGraphicsView, QGridLayout,
                               QWidget)

# ezdxf is imported lazily: it costs ~1.5 s and pulls in numpy, so 3D-only sessions never load it


def _item_keys():
    """QGraphicsItem data keys under which the ezdxf Qt backend stores the DXF entity / parent stack."""
    from ezdxf.addons.drawing.pyqt import CorrespondingDXFEntity, CorrespondingDXFParentStack
    return CorrespondingDXFEntity, CorrespondingDXFParentStack


DRAW_EXT = (".dxf", ".dwg")
NOT_DRAWABLE = {"3DSOLID", "BODY", "REGION", "SURFACE", "EXTRUDEDSURFACE", "LOFTEDSURFACE",
                "REVOLVEDSURFACE", "SWEPTSURFACE", "PLANESURFACE", "VIEWPORT", "ACAD_PROXY_ENTITY"}
NO_WINDOW = 0x08000000 if sys.platform == "win32" else 0
_ENV_LOCK = threading.Lock()      # os.environ is changed only for the ODA call (see dwg_to_dxf)


def _resource_dir():
    if getattr(sys, "frozen", False):
        return sys._MEIPASS
    return os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "third_party")


# ---------------------------------------------------------------- DWG conversion
WIN = sys.platform == "win32"


def find_oda():
    hits = []
    if WIN:
        for base in (os.environ.get("ProgramFiles"), os.environ.get("ProgramFiles(x86)")):
            if base:
                hits += glob.glob(os.path.join(base, "ODA", "ODAFileConverter*", "ODAFileConverter.exe"))
    else:
        import shutil
        hits += [p for p in (shutil.which("ODAFileConverter"),) if p]
        hits += glob.glob("/opt/ODAFileConverter*/ODAFileConverter") + glob.glob("/usr/bin/ODAFileConverter*")
    return sorted(hits)[-1] if hits else None


def find_dwg2dxf():
    sub = "libredwg" if (WIN or getattr(sys, "frozen", False)) else "libredwg-linux"
    exe = os.path.join(_resource_dir(), sub, "dwg2dxf.exe" if WIN else "dwg2dxf")
    if os.path.exists(exe):
        return exe
    if WIN:                 # shutil.which also looks into the current folder on Windows (exe planting)
        return None
    import shutil
    return shutil.which("dwg2dxf")


def dwg_to_dxf(src, dst):
    """Convert DWG -> ASCII DXF. ODA File Converter if installed, else bundled LibreDWG."""
    oda = find_oda()
    if oda:
        import ezdxf
        from ezdxf.addons import odafc
        ezdxf.options.set("odafc-addon", "win_exec_path", oda)
        # ODA File Converter is a Qt application too: it must not inherit our Qt settings (QT_QPA_PLATFORM=offscreen,
        # QT_PLUGIN_PATH of the bundled PySide6 ...) - with them it shows "no Qt platform plugin could be initialized"
        # and converts nothing. ezdxf starts it with the process environment, so the QT_* variables are taken out for
        # the conversion (Qt in this process read them at start-up already).
        with _ENV_LOCK:
            saved = {k: os.environ.pop(k) for k in list(os.environ) if k.upper().startswith("QT_")}
            try:
                odafc.convert(src, dst, version="R2018", replace=True)
            finally:
                os.environ.update(saved)
        return "ODA File Converter"
    exe = find_dwg2dxf()
    if not exe:
        raise RuntimeError("Chybí převodník DWG (dwg2dxf z LibreDWG, případně ODA File Converter).")
    r = subprocess.run([exe, "-v0", "-y", "-o", dst, src], capture_output=True, timeout=600,
                       creationflags=NO_WINDOW)
    if r.returncode != 0 or not os.path.exists(dst):
        msg = (r.stderr or b"").decode("utf-8", "replace").strip().splitlines()
        raise RuntimeError("Převod DWG selhal" + (": " + msg[-1] if msg else "."))
    return "LibreDWG"


# ---------------------------------------------------------------- loading
def read_dxf(path):
    """Fast strict reader first, tolerant recover mode as fallback."""
    import ezdxf
    from ezdxf import recover
    try:
        return ezdxf.readfile(path), 0
    except Exception:  # noqa: BLE001
        doc, auditor = recover.readfile(path)
        return doc, len(auditor.errors)


def fix_inventor_refs(doc):
    """Autodesk Inventor stores drawing views as ACIDBLOCKREFERENCE (unknown to ezdxf) –
    they are plain block references, so add an equivalent INSERT next to each."""
    n = 0
    for layout in doc.layouts:
        for e in list(layout):
            if e.dxftype() != "ACIDBLOCKREFERENCE" or not hasattr(e, "xtags"):
                continue
            name, ins, sx, sy, sz, rot = None, (0, 0, 0), 1.0, 1.0, 1.0, 0.0
            for sub in e.xtags.subclasses:
                for code, value in sub:
                    if code == 2:
                        name = value
                    elif code == 10:
                        ins = value
                    elif code == 41:
                        sx = value
                    elif code == 42:
                        sy = value
                    elif code == 43:
                        sz = value
                    elif code == 50:
                        rot = value
            if name and name in doc.blocks:
                layout.add_blockref(name, ins, dxfattribs={
                    "xscale": sx, "yscale": sy, "zscale": sz, "rotation": rot, "layer": "0"})
                n += 1
    return n


def load_drawing(path, dxf_cache=None, report=lambda m: None):
    """Returns (doc, info dict). For DWG, dxf_cache = path of a cached conversion (None: convert, read, discard)."""
    info = {"errors": 0, "converter": None, "inventor": 0}
    if not path.lower().endswith(".dwg"):
        report("Čtu DXF…")
        doc, info["errors"] = read_dxf(path)
    elif dxf_cache and os.path.exists(dxf_cache):
        info["converter"] = "mezipaměť"
        report("Čtu DXF…")
        doc, info["errors"] = read_dxf(dxf_cache)
    else:
        report("Převádím DWG na DXF…")
        import shutil
        import tempfile
        # the converter writes into a fresh private directory (mkdtemp = unique, 0700), never to a predictable or
        # pre-created file name; the finished DXF is moved to its place in one step
        base = os.path.dirname(os.path.abspath(dxf_cache)) if dxf_cache else None
        d = tempfile.mkdtemp(prefix="dwg_", dir=base)
        try:
            tmp = os.path.join(d, "o.dxf")
            info["converter"] = dwg_to_dxf(path, tmp)
            src = tmp
            if dxf_cache:
                os.replace(tmp, dxf_cache)
                src = dxf_cache
            report("Čtu DXF…")
            doc, info["errors"] = read_dxf(src)
        finally:
            shutil.rmtree(d, ignore_errors=True)
    info["inventor"] = fix_inventor_refs(doc)
    return doc, info


def layer_of(e):
    """Layer name of any entity (unknown entity types raise on dxf.get)."""
    try:
        return e.dxf.layer
    except Exception:  # noqa: BLE001
        return "0"


def drawable_count(layout):
    return sum(1 for e in layout if e.dxftype() not in NOT_DRAWABLE)


def default_layout(doc):
    msp = doc.modelspace()
    if drawable_count(msp) > 0:
        return "Model"
    for name in doc.layouts.names_in_taborder():
        if name != "Model" and drawable_count(doc.layouts.get(name)) > 0:
            return name
    return "Model"


def layer_table(doc):
    """[(name, QColor, visible-in-file)] for all layers."""
    from ezdxf.colors import aci2rgb
    out = []
    for lay in doc.layers:
        try:
            rgb = lay.rgb or aci2rgb(abs(lay.color) or 7)
        except Exception:  # noqa: BLE001
            rgb = (200, 200, 200)
        out.append((lay.dxf.name, QColor(*rgb), lay.is_on() and not lay.is_frozen()))
    return out


_URL_SCHEME = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]+:")      # 2+ letters: "C:" is a drive, "file:" / "http:" a URL


def is_remote_path(name):
    """True for an image path that is not a plain local one: UNC (\\\\server\\share, any mix of / and \\ -
    ezdxf and Windows treat both as separators), device / NT paths (\\\\?\\, \\\\.\\, \\??\\) and URLs.
    Opening such a path makes Windows connect to the server and send the user's NTLM credentials."""
    raw = "".join(ch for ch in str(name) if ch >= " ").strip()     # control characters / blanks around the path
    n = raw.replace("/", "\\")
    if not n:
        return False
    if n.startswith(("\\\\", "\\??\\")) or "://" in raw or _URL_SCHEME.match(n):
        return True
    from pathlib import PureWindowsPath
    try:
        return PureWindowsPath(n).drive.startswith("\\\\")
    except ValueError:
        return True


def remote_images(doc):
    """IMAGEDEF file names that are not plain local paths (see is_remote_path)."""
    bad = []
    for e in doc.objects.query("IMAGEDEF"):
        name = e.dxf.get("filename", "") or ""
        if is_remote_path(name):
            bad.append(name)
    return bad


# ---------------------------------------------------------------- resource limits
# Block references nest: 10 INSERTs per level x 5 levels = 100 000 drawn entities from a 25 KB file, and a block
# that (indirectly) inserts itself recurses until Python gives up. The number of entities a layout expands to is
# known from the block graph before anything is drawn.
MAX_EXPANDED_ENTITIES = 300_000
# The absolute limit alone still lets a 25 KB file through that expands to 211 000 entities (minutes in the GUI
# thread: ~0.6 ms per entity). Real drawings re-use blocks, but not by a factor of hundreds: many more drawn
# entities than entity records in the file means nested re-use, i.e. a bomb.
AMPLIFY_MIN = 50_000
AMPLIFY_RATIO = 100
MAX_BLOCK_DEPTH = 100         # ezdxf draws chains of ~160 nested blocks, then dies with RecursionError (limit 1000)


class DrawingTooComplex(RuntimeError):
    """The layout would expand to too many entities (or contains a cyclic block reference)."""


def expanded_entity_count(doc, layout, limit=MAX_EXPANDED_ENTITIES):
    """Entities ezdxf would draw for `layout`: every entity once, INSERTs (MINSERT: rows x columns) plus the
    content of their blocks, paper space viewports plus the model space they show. Memoised per block name, so
    the cost is the size of the file, not of the expansion. Raises DrawingTooComplex above `limit` or when the
    blocks reference each other in a cycle."""
    memo = {}                      # block name (lower) -> expanded entity count of its content
    active = []                    # block names on the current path
    stored = [0]                   # entity records actually present in the file (layout + blocks used, each once)

    def over():
        lim = f"{limit:,}".replace(",", " ")
        return DrawingTooComplex(f"Výkres se rozvinutím bloků rozpadne na více než {lim} entit – "
                                 "to je příliš (pravděpodobně vnořené bloky, které se mnohonásobně opakují). "
                                 "Výkres se nevykreslí.")

    def block_count(name):
        key = str(name).lower()
        if key in memo:
            return memo[key]
        if key in active:
            raise DrawingTooComplex(f"Cyklický blok: „{name}“ se přímo nebo nepřímo vkládá sám do sebe – "
                                    "výkres nelze vykreslit.")
        if len(active) >= MAX_BLOCK_DEPTH:
            raise DrawingTooComplex(f"Bloky jsou vnořené příliš hluboko (více než {MAX_BLOCK_DEPTH} úrovní) – "
                                    "výkres nelze vykreslit.")
        if name not in doc.blocks:
            memo[key] = 0
            return 0
        active.append(key)
        try:
            n = entities_count(doc.blocks.get(name))
        finally:
            active.pop()
        memo[key] = n
        return n

    def entities_count(ents):
        total = len(ents)
        stored[0] += total
        for e in ents:
            t = e.dxftype()
            if t == "INSERT":
                try:
                    times = e.dxf.row_count * e.dxf.column_count if e.mcount > 1 else 1   # = what multi_insert yields
                    total += len(e.attribs)
                    total += max(1, times) * block_count(e.dxf.name)
                except (AttributeError, TypeError, ValueError):
                    pass
            if total > limit:
                raise over()
        return total

    total = entities_count(layout)
    if layout.name != "Model":
        views = sum(1 for e in layout if e.dxftype() == "VIEWPORT" and e.dxf.get("status", 1) > 0)
        if views:
            total += views * entities_count(doc.modelspace())
    if total > limit:
        raise over()
    if total > AMPLIFY_MIN and total > AMPLIFY_RATIO * stored[0]:
        raise DrawingTooComplex(
            f"Výkres má v souboru jen {stored[0]} entit, ale rozvinutím vnořených bloků by jich vykreslil "
            f"{total} ({total // max(1, stored[0])}× víc) – to vypadá jako záměrně vytvořená „bomba“ a "
            "vykreslování by trvalo minuty. Výkres se nevykreslí.")
    return total


def check_layout_budget(doc, layout_name):
    """Raise DrawingTooComplex if drawing the layout would exceed the entity budget (cached per document)."""
    cache = doc.__dict__.setdefault("_sv_budget", {})
    if layout_name not in cache:
        cache[layout_name] = expanded_entity_count(doc, doc.layouts.get(layout_name))
    return cache[layout_name]


class LayerMap:
    """Which layers decide whether each item of a drawn scene is visible, so that a layer toggle only shows / hides
    items instead of drawing the whole layout again (0.4 - 8 s). See render_scene(track=True).

    ezdxf hides an entity when the layer it resolves to is off / frozen (an entity on layer "0" inside a block takes
    the layer of its INSERT), and a composite entity (DIMENSION ...) hides everything it draws. INSERT, 3DFACE and
    VIEWPORT ignore their own layer. The scene is drawn once with every layer on; each item is filed under the tuple
    of layers it depends on (`groups`), and a toggle applies  all(layer visible for layer in key)  per group."""

    def __init__(self, groups, base):
        self.groups = groups                       # tuple of layer keys (lower case) -> [QGraphicsItem]
        self.base = base                           # layer key -> visible according to the file
        self.state = {k: True for k in groups}     # shown right now? (the scene starts with everything shown)
        self._rects = {}

    def is_shown(self, key, layer_vis):
        base = self.base                           # a layer missing from the layer table is always shown
        return all(layer_vis.get(k, base[k]) if k in base else True for k in key)

    def apply(self, layer_vis):
        """Show / hide the items for {layer key: bool} overrides (layers not mentioned: as in the file)."""
        for key, items in self.groups.items():
            shown = self.is_shown(key, layer_vis)
            if shown != self.state[key]:
                self.state[key] = shown
                for it in items:
                    it.setVisible(shown)

    def _group_rect(self, key):
        r = self._rects.get(key)
        if r is None:
            x0 = y0 = math.inf
            x1 = y1 = -math.inf
            for it in self.groups[key]:
                x, y, w, h = it.sceneBoundingRect().getRect()
                if w == 0.0 and h == 0.0:           # QRectF::united ignores null rectangles
                    continue
                x0, x1 = min(x0, x, x + w), max(x1, x, x + w)
                y0, y1 = min(y0, y, y + h), max(y1, y, y + h)
            r = self._rects[key] = (x0, y0, x1, y1)
        return r

    def bounding_rect(self, scene):
        """Bounding rectangle of the shown items (QGraphicsScene.itemsBoundingRect also counts hidden ones)."""
        full = scene.itemsBoundingRect()            # C++, all items
        hidden = [k for k, shown in self.state.items() if not shown]
        if not hidden:
            return full
        # Usual case: the hidden layers lie inside the drawing, so the extreme items on all four sides are shown and
        # `full` is the answer. Only the (few) hidden items have to be measured then.
        hx0 = hy0 = math.inf
        hx1 = hy1 = -math.inf
        for key in hidden:
            a, b, c, d = self._group_rect(key)
            hx0, hy0, hx1, hy1 = min(hx0, a), min(hy0, b), max(hx1, c), max(hy1, d)
        tol = 1e-9 * (abs(full.width()) + abs(full.height()) + abs(full.x()) + abs(full.y()) + 1.0)
        if (hx0 > full.x() + tol and hy0 > full.y() + tol and hx1 < full.x() + full.width() - tol
                and hy1 < full.y() + full.height() - tol):
            return full
        x0 = y0 = math.inf
        x1 = y1 = -math.inf
        for key, shown in self.state.items():
            if shown:
                a, b, c, d = self._group_rect(key)
                x0, y0, x1, y1 = min(x0, a), min(y0, b), max(x1, c), max(y1, d)
        return QRectF(x0, y0, x1 - x0, y1 - y0) if x0 <= x1 and y0 <= y1 else QRectF()


_tracking_backend_cls = None


def _tracking_backend():
    """PyQtBackend subclass that files every item under the layers its visibility depends on (see LayerMap)."""
    global _tracking_backend_cls
    if _tracking_backend_cls is None:
        from ezdxf.addons.drawing.pyqt import PyQtBackend
        from ezdxf.entities import Face3d, Insert, Viewport
        blind = (Insert, Face3d, Viewport)         # ezdxf ignores the layer of these when deciding visibility

        class TrackingBackend(PyQtBackend):
            def __init__(self, scene):
                super().__init__(scene)
                self.groups = {}

            def set_item_data(self, item, entity_handle):
                super().set_item_data(item, entity_handle)
                key = []
                for e, p in self.entity_stack:      # the entities being drawn: composite parents ... the current one
                    if not isinstance(e, blind):
                        k = p.layer.lower()
                        if k not in key:
                            key.append(k)
                key = tuple(key)
                g = self.groups.get(key)
                if g is None:
                    g = self.groups[key] = []
                g.append(item)

        _tracking_backend_cls = TrackingBackend
    return _tracking_backend_cls


_log = logging.getLogger("stepview.view2d")
fast_fallback_reason = None        # why the last drawing was drawn the legacy way (None: batching scene builder used)


def _legacy_forced():
    return os.environ.get("STEPVIEW_DXF_LEGACY", "").strip() not in ("", "0")


def render_scene(doc, layout_name, layer_vis=None, light_bg=False, images=True, track=False, fast=None):
    """layer_vis: {layer name lower: bool} overrides of layer visibility.
    images=False never loads raster images referenced by the drawing (thumbnails); with images=True they are
    loaded only when every reference is a local path.
    track=True (the viewer): the whole layout is drawn with every layer on and `scene.layer_map` (a LayerMap) is set,
    which shows / hides items for a layer change without drawing again; the scene already has `layer_vis` applied.
    No map (and a scene drawn the plain way) when the layers are not the same in every viewport.
    fast: None = the batching scene builder of fastscene.py (1.9: a few hundred big path items instead of one item per
    primitive, 2 - 3x faster to draw, ~10x fewer items) - unless STEPVIEW_DXF_LEGACY=1 is set, the drawing is too big
    for its Python fallback (no native rdxf module) or it fails: any exception makes this function draw the legacy
    way (one QGraphicsItem per primitive, `fast_fallback_reason` says why). False: legacy. True: batching, no fallback."""
    global fast_fallback_reason
    layer_vis = layer_vis or {}
    entities = check_layout_budget(doc, layout_name)   # DrawingTooComplex before anything is expanded / drawn
    if fast is False or (fast is None and _legacy_forced()):
        fast_fallback_reason = "legacy renderer requested"
    else:
        try:
            import fastscene
            if fast is None and not fastscene.have_native() and entities > fastscene.PY_MAX_ENTITIES:
                fast_fallback_reason = "%d entities and no native rdxf module" % entities
            else:
                result = _render_scene(doc, layout_name, layer_vis, light_bg, images, track, True)
                fast_fallback_reason = None
                return result
        except DrawingTooComplex:
            raise
        except Exception as ex:  # noqa: BLE001 - the legacy renderer is the safety net for anything in the fast path
            if fast:
                raise
            fast_fallback_reason = "%s: %s" % (type(ex).__name__, ex)
            _log.warning("fast 2D scene builder failed (%s), drawing the legacy way", fast_fallback_reason)
    return _render_scene(doc, layout_name, layer_vis, light_bg, images, track, False)


def _render_scene(doc, layout_name, layer_vis, light_bg, images, track, fast):
    from ezdxf.addons.drawing import Frontend, RenderContext
    from ezdxf.addons.drawing.config import BackgroundPolicy, Configuration, ImagePolicy
    from ezdxf.addons.drawing.pyqt import PyQtBackend
    ctx = RenderContext(doc)
    bases = []                                     # layer visibility in the file, once per rendering context

    if track:
        def override(layers):
            bases.append({lp.layer.lower(): lp.is_visible for lp in layers})
            for lp in layers:
                lp.is_visible = True
    else:
        def override(layers):
            for lp in layers:
                v = layer_vis.get(lp.layer.lower())
                if v is not None:
                    lp.is_visible = v

    ctx.set_layer_properties_override(override)
    layout = doc.layouts.get(layout_name)
    ctx.set_current_layout(layout)
    safe = images and not remote_images(doc)
    cfg = Configuration(background_policy=BackgroundPolicy.WHITE if light_bg else BackgroundPolicy.DEFAULT,
                        image_policy=ImagePolicy.DISPLAY if safe else ImagePolicy.RECT)
    scene = QGraphicsScene()
    if fast:
        import fastscene
        backend = fastscene.draw_layout(ctx, layout, cfg, scene)
    else:
        backend = _tracking_backend()(scene) if track else PyQtBackend(scene)
        Frontend(ctx, backend, config=cfg).draw_layout(layout, finalize=True)
    if track:
        if not bases or any(b != bases[0] for b in bases[1:]):
            # a viewport freezes / overrides layers on its own: visibility is not a function of the layer alone
            return _render_scene(doc, layout_name, layer_vis, light_bg, images, False, fast)
        lm = LayerMap(backend.groups, bases[0])
        lm.apply(layer_vis)
        scene.layer_map = lm
    return scene, ctx


# ---------------------------------------------------------------- info
UNITS = {0: "bez jednotek", 1: "palce", 2: "stopy", 4: "mm", 5: "cm", 6: "m", 7: "km", 8: "mikropalce",
         9: "mil", 10: "yardy", 13: "µm", 14: "dm"}


def _p(v, nd=2):
    return f"{v:,.{nd}f}".replace(",", " ").replace(".", ",")


def _pt(p):
    return f"{_p(p[0])}, {_p(p[1])}"


def doc_info(doc):
    import ezdxf
    rel = ezdxf.const.acad_release.get(doc.dxfversion, doc.dxfversion)
    units = UNITS.get(doc.header.get("$INSUNITS", 0), str(doc.header.get("$INSUNITS")))
    return rel, units


def entity_rows(e):
    t = e.dxftype()
    rows = [("Entita", t), ("Hladina", layer_of(e))]
    try:
        if t == "LINE":
            s, en = e.dxf.start, e.dxf.end
            rows += [("Délka", _p(s.distance(en))),
                     ("Úhel", _p(math.degrees(math.atan2(en.y - s.y, en.x - s.x)) % 360, 2) + "°"),
                     ("Začátek", _pt(s)), ("Konec", _pt(en))]
        elif t == "CIRCLE":
            r = e.dxf.radius
            rows += [("Poloměr", _p(r)), ("Průměr", _p(2 * r)), ("Střed", _pt(e.dxf.center)),
                     ("Obvod", _p(2 * math.pi * r))]
        elif t == "ARC":
            r = e.dxf.radius
            a = (e.dxf.end_angle - e.dxf.start_angle) % 360
            rows += [("Poloměr", _p(r)), ("Průměr", _p(2 * r)), ("Střed", _pt(e.dxf.center)),
                     ("Úhel oblouku", _p(a) + "°"), ("Délka oblouku", _p(math.radians(a) * r))]
        elif t in ("LWPOLYLINE", "POLYLINE", "SPLINE", "ELLIPSE"):
            from ezdxf import path as epath
            pts = list(epath.make_path(e).flattening(0.01))
            ln = sum(pts[i].distance(pts[i + 1]) for i in range(len(pts) - 1))
            rows.append(("Délka", _p(ln)))
            if t == "LWPOLYLINE":
                rows += [("Vrcholů", str(len(e))), ("Uzavřená", "ano" if e.closed else "ne")]
        elif t in ("TEXT", "ATTRIB", "ATTDEF"):
            rows += [("Text", e.dxf.text), ("Výška", _p(e.dxf.height))]
        elif t == "MTEXT":
            rows += [("Text", e.plain_text()[:300]), ("Výška", _p(e.dxf.char_height))]
        elif t == "DIMENSION":
            m = e.get_measurement()
            val = m if isinstance(m, (int, float)) else getattr(m, "magnitude", m)
            rows.append(("Naměřeno", _p(val, 3) if isinstance(val, (int, float)) else str(val)))
            if e.dxf.get("text"):
                rows.append(("Text kóty", e.dxf.text))
        elif t == "INSERT":
            rows += [("Blok", e.dxf.name), ("Vložení", _pt(e.dxf.insert)),
                     ("Měřítko", f"{_p(e.dxf.xscale, 3)} / {_p(e.dxf.yscale, 3)}"),
                     ("Natočení", _p(e.dxf.rotation) + "°")]
        elif t == "HATCH":
            rows.append(("Šrafa", e.dxf.pattern_name))
    except Exception:  # noqa: BLE001 – info is best effort
        pass
    try:
        rows.append(("Handle", e.dxf.handle))
    except Exception:  # noqa: BLE001
        pass
    return rows


# ---------------------------------------------------------------- view
class DrawingView(QGraphicsView):
    cursorMoved = Signal(float, float)
    entityPicked = Signal(object, object)      # top-level entity (or None), picked sub-entity
    measured = Signal(object, object)          # (x, y), (x, y)
    angleMeasured = Signal(float)
    contextMenuAt = Signal(object)
    viewChanged = Signal()

    SEL = QColor(255, 140, 0)

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setScene(QGraphicsScene())
        self.setRenderHints(QPainter.Antialiasing | QPainter.TextAntialiasing | QPainter.SmoothPixmapTransform)
        self.setHorizontalScrollBarPolicy(Qt.ScrollBarAlwaysOff)
        self.setVerticalScrollBarPolicy(Qt.ScrollBarAlwaysOff)
        self.setTransformationAnchor(QGraphicsView.AnchorUnderMouse)
        self.setResizeAnchor(QGraphicsView.AnchorViewCenter)
        self.setViewportUpdateMode(QGraphicsView.SmartViewportUpdate)
        self.setFrameShape(QGraphicsView.NoFrame)
        self.setMouseTracking(True)
        self.scale(1, -1)                      # DXF: +Y up
        self._sm_last = 0.0                    # 3D mouse: last motion time, sub-pixel pan remainder
        self._sm_frac = (0.0, 0.0)
        self._press = None
        self._last = None
        self._drag = False
        self._sel_items = []
        self._index = {}
        self.batch = None                      # fastscene.Batch of the scene (batching scene builder), else None
        self.layer_map = None
        self.measure_mode = False
        self._mpts = []
        self._mitems = []
        self._snap_item = None
        self.content_rect = QRectF()

    # ---------------------------------------------------------- scene
    def set_scene(self, scene, keep_view=False):
        t = self.transform()
        c = self.mapToScene(self.viewport().rect().center())
        self.clear_selection()
        self.clear_measure()
        self.setScene(scene)
        self.layer_map = getattr(scene, "layer_map", None)       # LayerMap: layer toggles without drawing again
        self._fit_scene_rect()
        self._index = collections.defaultdict(list)
        self.batch = getattr(scene, "batch", None)
        if self.batch is None:                 # one item per primitive: index the items by top level entity
            items = scene.items()
            k_ent, k_stack = _item_keys() if items else (None, None)
            for it in items:
                e = it.data(k_ent)
                if e is None:
                    continue
                stack = it.data(k_stack) or ()
                top = stack[0] if stack else e
                self._index[id(top)].append(it)
        if keep_view:
            self.setTransform(t)
            self.centerOn(c)
        else:
            self.fit()

    def _fit_scene_rect(self):
        """content_rect = bounds of what is shown; the scene rect leaves room to pan far outside."""
        scene = self.scene()
        lm = self.layer_map
        self.content_rect = lm.bounding_rect(scene) if lm is not None else scene.itemsBoundingRect()
        r = self.content_rect
        big = max(r.width(), r.height(), 1.0) * 20
        scene.setSceneRect(r.adjusted(-big, -big, big, big))

    def apply_layers(self, layer_vis):
        """Layer visibility changed: show / hide the drawn items (same picture as drawing the layout again, only
        much faster). False when the scene has no layer map - the caller has to draw again."""
        lm = self.layer_map
        if lm is None:
            return False
        t = self.transform()
        c = self.mapToScene(self.viewport().rect().center())
        self.clear_selection()                      # as set_scene: a new picture has no selection / measurement
        self.clear_measure()
        self.setUpdatesEnabled(False)
        try:
            lm.apply(layer_vis)
            self._fit_scene_rect()
            self.setTransform(t)
            self.centerOn(c)
        finally:
            self.setUpdatesEnabled(True)
        self.viewport().update()
        self.viewChanged.emit()
        return True

    def fit(self):
        r = self.content_rect
        if r.isEmpty():
            return
        m = max(r.width(), r.height()) * 0.03
        self.fitInView(r.adjusted(-m, -m, m, m), Qt.KeepAspectRatio)

    def zoom_to_items(self, items):
        r = QRectF()
        for it in items:
            r = r.united(it.sceneBoundingRect())
        if not r.isEmpty():
            m = max(r.width(), r.height()) * 0.2 + 1e-6
            self.fitInView(r.adjusted(-m, -m, m, m), Qt.KeepAspectRatio)

    # ---------------------------------------------------------- selection
    def clear_selection(self):
        for it, pen in self._sel_items:
            try:
                if pen is None:                # batching scene: an overlay item (removed) or an item that was not changed
                    if getattr(it, "_overlay", False):
                        it.scene().removeItem(it)
                else:
                    it.setPen(pen)
            except (RuntimeError, AttributeError):
                pass
        self._sel_items = []

    def select_top(self, top):
        self.clear_selection()
        if self.batch is not None:
            self._sel_items = self.batch.highlight(self.scene(), top, self.SEL)
            return [it for it, _ in self._sel_items]
        for it in self._index.get(id(top), []):
            if hasattr(it, "pen") and it.isVisible():       # parts on hidden layers are not selected / zoomed to
                pen = it.pen()
                self._sel_items.append((it, QPen(pen)))
                p = QPen(pen)
                p.setColor(self.SEL)
                p.setCosmetic(True)
                p.setWidthF(max(2.0, pen.widthF()))
                it.setPen(p)
        return [it for it, _ in self._sel_items]

    def _pick_batch(self, rect):
        """Topmost entity under the scene rectangle in a batching scene: the stroke primitives are tested on the
        arrays (Batch.pick), fills / points / images are Qt items; the one drawn last wins, as before."""
        B = self.batch
        try:
            best = B.pick(rect)
            for it in self.scene().items(rect, Qt.IntersectsItemShape, Qt.DescendingOrder):
                q = it.data(0)
                if q is not None and not getattr(it, "_batch_chunk", False) and q > best and it.isVisible():
                    best = q
            return B.entities_of(best) if best >= 0 else (None, None)
        except Exception as ex:  # noqa: BLE001 - a failing pick must not break the mouse handler
            _log.warning("pick failed: %s", ex)
            return None, None

    def _pick(self, pos):
        r = 5
        if self.batch is not None:
            return self._pick_batch(self.mapToScene(QRect(int(pos.x() - r), int(pos.y() - r), 2 * r, 2 * r)).boundingRect())
        items = self.items(pos.x() - r, pos.y() - r, 2 * r, 2 * r)
        if not items:
            return None, None
        k_ent, k_stack = _item_keys()
        for it in items:
            e = it.data(k_ent)
            if e is not None and it.isVisible():
                stack = it.data(k_stack) or ()
                return (stack[0] if stack else e), e
        return None, None

    # ---------------------------------------------------------- measure
    def set_measure_mode(self, kind):
        """kind: None / "dist" / "angle"."""
        self.measure_mode = kind
        self.clear_measure()
        self.viewport().setCursor(Qt.CrossCursor if kind else Qt.ArrowCursor)

    def clear_measure(self):
        sc = self.scene()
        extra = [x for x in (self._snap_item, getattr(self, "_rubber", None)) if x is not None]
        for it in self._mitems + extra:
            try:
                sc.removeItem(it)
            except RuntimeError:
                pass
        self._mitems, self._mpts, self._snap_item, self._rubber = [], [], None, None

    def _measure_pen(self):
        pen = QPen(QColor(230, 30, 30), 2)
        pen.setCosmetic(True)
        return pen

    def _add_line(self, a, b, dashed=False):
        ln = QGraphicsLineItem(a.x(), a.y(), b.x(), b.y())
        pen = self._measure_pen()
        if dashed:
            pen.setStyle(Qt.DashLine)
        ln.setPen(pen)
        ln.setZValue(1e6)
        self.scene().addItem(ln)
        return ln

    def _add_text(self, p, text):
        txt = QGraphicsSimpleTextItem(text)
        f = QFont()
        f.setPointSize(11)
        f.setBold(True)
        txt.setFont(f)
        txt.setBrush(QBrush(QColor(230, 30, 30)))
        txt.setFlag(QGraphicsSimpleTextItem.ItemIgnoresTransformations)
        txt.setPos(p)
        txt.setZValue(1e6)
        self.scene().addItem(txt)
        self._mitems.append(txt)

    def _measure_point(self, pos, shift):
        sp = self._snap(pos) or self.mapToScene(pos)
        if shift and self._mpts:                 # Shift = horizontal / vertical from the last point
            a = self._mpts[-1]
            if abs(sp.x() - a.x()) >= abs(sp.y() - a.y()):
                sp = QPointF(sp.x(), a.y())
            else:
                sp = QPointF(a.x(), sp.y())
        return sp

    def _update_rubber(self, pos, shift):
        if not self._mpts:
            return
        a = self._mpts[-1]
        b = self._measure_point(pos, shift)
        if getattr(self, "_rubber", None) is None:
            self._rubber = self._add_line(a, b, dashed=True)
        else:
            self._rubber.setLine(a.x(), a.y(), b.x(), b.y())

    def _snap_batch(self, pos, lines):
        """(distance^2, point) of the nearest vertex of the batched geometry within 10 px, or None."""
        try:
            rect = self.mapToScene(QRect(int(pos.x() - 10), int(pos.y() - 10), 20, 20)).boundingRect()
            r = self.batch.snap(rect, self.viewportTransform(), pos.x(), pos.y(), lines)
        except Exception as ex:  # noqa: BLE001
            _log.warning("snap failed: %s", ex)
            return None
        return None if r is None else (r[0], QPointF(r[1], r[2]))

    def _snap(self, pos):
        """Nearest path vertex of drawn geometry within 10 px (scene coords), else cursor point."""
        best, bd = None, 10.0 ** 2
        if self.batch is not None:             # the strokes / fills are in the arrays, nothing else snaps
            for lines in (False, True):        # path vertices first, end points of plain lines only as a fallback
                r = self._snap_batch(pos, lines)
                if r is not None and r[0] < bd:
                    best, bd = r[1], r[0]
                if best is not None:
                    return best
            return None
        for it in self.items(pos.x() - 10, pos.y() - 10, 20, 20):
            if not hasattr(it, "path"):
                continue
            path = it.path()
            for i in range(path.elementCount()):
                el = path.elementAt(i)
                if el.type in (QPainterPath.ElementType.CurveToElement,
                               QPainterPath.ElementType.CurveToDataElement):   # bezier control points
                    continue
                sp = it.mapToScene(QPointF(el.x, el.y))
                vp = self.mapFromScene(sp)
                d = (vp.x() - pos.x()) ** 2 + (vp.y() - pos.y()) ** 2
                if d < bd:
                    best, bd = sp, d
        if best is None:
            for it in self.items(pos.x() - 10, pos.y() - 10, 20, 20):
                if isinstance(it, QGraphicsLineItem) and it not in self._mitems:
                    ln = it.line()
                    for q in (ln.p1(), ln.p2()):
                        sp = it.mapToScene(q)
                        vp = self.mapFromScene(sp)
                        d = (vp.x() - pos.x()) ** 2 + (vp.y() - pos.y()) ** 2
                        if d < bd:
                            best, bd = sp, d
        return best

    def _show_snap(self, sp):
        sc = self.scene()
        if self._snap_item is None:
            self._snap_item = QGraphicsRectItem(-5, -5, 10, 10)
            self._snap_item.setPen(QPen(QColor(0, 200, 255), 2))
            self._snap_item.setFlag(QGraphicsRectItem.ItemIgnoresTransformations)
            self._snap_item.setZValue(1e6)
            sc.addItem(self._snap_item)
        self._snap_item.setVisible(sp is not None)
        if sp is not None:
            self._snap_item.setPos(sp)

    def _measure_click(self, pos, shift=False):
        sp = self._measure_point(pos, shift)
        self._mpts.append(sp)
        dot = QGraphicsEllipseItem(-4, -4, 8, 8)
        dot.setBrush(QBrush(QColor(230, 30, 30)))
        dot.setPen(QPen(Qt.NoPen))
        dot.setFlag(QGraphicsEllipseItem.ItemIgnoresTransformations)
        dot.setPos(sp)
        dot.setZValue(1e6)
        self.scene().addItem(dot)
        self._mitems.append(dot)
        if getattr(self, "_rubber", None) is not None:
            self.scene().removeItem(self._rubber)
            self._rubber = None
        pts = self._mpts
        if self.measure_mode == "dist" and len(pts) == 2:
            a, b = pts
            self._mitems.append(self._add_line(a, b))
            self._add_text((a + b) / 2, _p(math.hypot(b.x() - a.x(), b.y() - a.y()), 2))
            self._mpts = []
            self.measured.emit((a.x(), a.y()), (b.x(), b.y()))
        elif self.measure_mode == "angle" and len(pts) == 3:
            a, v, c = pts                        # arm end, vertex, arm end
            self._mitems.append(self._add_line(v, c))
            a1 = math.atan2(a.y() - v.y(), a.x() - v.x())
            a2 = math.atan2(c.y() - v.y(), c.x() - v.x())
            ang = abs(math.degrees(a2 - a1)) % 360
            if ang > 180:
                ang = 360 - ang
            self._add_text(v, _p(ang, 2) + "°")
            self._mpts = []
            self.angleMeasured.emit(ang)
        elif self.measure_mode == "angle" and len(pts) == 2:
            self._mitems.append(self._add_line(pts[0], pts[1]))

    # ---------------------------------------------------------- mouse
    def mousePressEvent(self, e):
        self._press = e.position().toPoint()
        self._last = self._press
        self._drag = False

    def mouseMoveEvent(self, e):
        pos = e.position().toPoint()
        sp = self.mapToScene(pos)
        self.cursorMoved.emit(sp.x(), sp.y())
        if self._press is None:
            if self.measure_mode:
                self._show_snap(self._snap(pos))
                self._update_rubber(pos, bool(e.modifiers() & Qt.ShiftModifier))
            return
        if not self._drag and (pos - self._press).manhattanLength() < 4:
            return
        self._drag = True
        d = pos - self._last
        self._last = pos
        self.horizontalScrollBar().setValue(self.horizontalScrollBar().value() - d.x())
        self.verticalScrollBar().setValue(self.verticalScrollBar().value() - d.y())

    def mouseReleaseEvent(self, e):
        pos = e.position().toPoint()
        was_drag, self._press = self._drag, None
        self._drag = False
        if was_drag:
            return
        if e.button() == Qt.LeftButton:
            if self.measure_mode:
                self._measure_click(pos, bool(e.modifiers() & Qt.ShiftModifier))
                return
            top, sub = self._pick(pos)
            if top is None:
                self.clear_selection()
            else:
                self.select_top(top)
            self.entityPicked.emit(top, sub)
        elif e.button() == Qt.RightButton:
            self.contextMenuAt.emit(self.mapToGlobal(pos))

    def mouseDoubleClickEvent(self, e):
        if e.button() == Qt.LeftButton and self._sel_items:
            self.zoom_to_items([it for it, _ in self._sel_items])

    def wheelEvent(self, e):
        d = e.angleDelta().y()
        if d and time.monotonic() - self._sm_last > 0.4:      # 3DxWare may emulate the wheel while moving
            f = 1.2 ** (d / 120.0)
            self.scale(f, f)
            self.viewChanged.emit()

    def space_motion(self, v, dt):
        """3D mouse: the drawing follows the cap (push = pan, pull towards you = zoom in); rotations unused."""
        tx, ty, tz = v[0], v[1], v[2]
        self._sm_last = time.monotonic()
        if tz:
            f = math.exp(tz * 2.2 * dt)
            self.setTransformationAnchor(QGraphicsView.AnchorViewCenter)
            self.scale(f, f)
            self.setTransformationAnchor(QGraphicsView.AnchorUnderMouse)
        if tx or ty:                               # scroll bars are whole pixels - keep the remainder
            fx = self._sm_frac[0] - tx * 1100.0 * dt
            fy = self._sm_frac[1] + ty * 1100.0 * dt
            ix, iy = int(fx), int(fy)
            self._sm_frac = (fx - ix, fy - iy)
            self.horizontalScrollBar().setValue(self.horizontalScrollBar().value() + ix)
            self.verticalScrollBar().setValue(self.verticalScrollBar().value() + iy)
        self.viewChanged.emit()

    def scrollContentsBy(self, dx, dy):
        super().scrollContentsBy(dx, dy)
        self.viewChanged.emit()

    def resizeEvent(self, e):
        super().resizeEvent(e)
        self.viewChanged.emit()

    def fitInView(self, *args):
        super().fitInView(*args)
        self.viewChanged.emit()


class Ruler(QWidget):
    """CAD-style ruler along the top / left edge of a DrawingView, in drawing units."""
    THICK = 24

    def __init__(self, view, horizontal, parent=None):
        super().__init__(parent)
        self.view, self.horizontal = view, horizontal
        self.cursor_val = None
        if horizontal:
            self.setFixedHeight(self.THICK)
        else:
            self.setFixedWidth(self.THICK)
        view.viewChanged.connect(self.update)
        view.cursorMoved.connect(self._on_cursor)

    def _on_cursor(self, x, y):
        self.cursor_val = x if self.horizontal else y
        self.update()

    def paintEvent(self, e):
        p = QPainter(self)
        pal = self.palette()
        bg, fg = pal.color(pal.ColorRole.Window), pal.color(pal.ColorRole.WindowText)
        p.fillRect(self.rect(), bg)
        vp = self.view.viewport()
        if self.horizontal:
            n = vp.width()
            a = self.view.mapToScene(0, 0).x()
            b = self.view.mapToScene(n, 0).x()
        else:
            n = vp.height()
            a = self.view.mapToScene(0, 0).y()
            b = self.view.mapToScene(0, n).y()
        if n <= 0 or abs(b - a) < 1e-12:
            return
        upp = abs(b - a) / n
        step = _nice(75 * upp)
        mant = round(step / 10 ** math.floor(math.log10(step)))
        minor = step / (4 if mant == 2 else 5)
        dec = max(0, -int(math.floor(math.log10(step))))
        lo, hi = min(a, b), max(a, b)
        p.setPen(QPen(fg, 1))
        f = p.font()
        f.setPointSizeF(7.5)
        p.setFont(f)
        T = self.THICK
        v = math.floor(lo / minor) * minor
        cnt = 0
        while v <= hi and cnt < 3000:
            cnt += 1
            pix = (v - a) / (b - a) * n
            major = abs(v / step - round(v / step)) < 1e-6
            tl = T * (0.55 if major else 0.22)
            if self.horizontal:
                p.drawLine(QPointF(pix, T - tl), QPointF(pix, T))
            else:
                p.drawLine(QPointF(T - tl, pix), QPointF(T, pix))
            if major:
                txt = f"{v:.{dec}f}"
                if float(txt) == 0:
                    txt = txt.lstrip("-")
                txt = txt.replace(".", ",")
                if self.horizontal:
                    p.drawText(QPointF(pix + 3, 10), txt)
                else:
                    p.save()
                    p.translate(10, pix - 3)
                    p.rotate(-90)
                    p.drawText(QPointF(0, 0), txt)
                    p.restore()
            v += minor
        p.setPen(QPen(fg.darker(150) if fg.lightness() > 128 else fg.lighter(250), 1))
        if self.horizontal:
            p.drawLine(0, T - 1, self.width(), T - 1)
        else:
            p.drawLine(T - 1, 0, T - 1, self.height())
        if self.cursor_val is not None:
            pix = (self.cursor_val - a) / (b - a) * n
            p.setPen(QPen(QColor(230, 60, 60), 1))
            if self.horizontal:
                p.drawLine(QPointF(pix, 0), QPointF(pix, T))
            else:
                p.drawLine(QPointF(0, pix), QPointF(T, pix))


def _nice(raw):
    if raw <= 0:
        return 1.0
    e = 10.0 ** math.floor(math.log10(raw))
    for m in (1.0, 2.0, 5.0, 10.0):
        if m * e >= raw - 1e-12:
            return m * e
    return 10.0 * e


class DrawingPanel(QWidget):
    """DrawingView with rulers on the top and left."""

    def __init__(self, view, parent=None):
        super().__init__(parent)
        self.view = view
        g = QGridLayout(self)
        g.setContentsMargins(0, 0, 0, 0)
        g.setSpacing(0)
        self.corner = QWidget()
        self.corner.setFixedSize(Ruler.THICK, Ruler.THICK)
        self.top = Ruler(view, True)
        self.left = Ruler(view, False)
        g.addWidget(self.corner, 0, 0)
        g.addWidget(self.top, 0, 1)
        g.addWidget(self.left, 1, 0)
        g.addWidget(view, 1, 1)

    def set_rulers(self, on):
        for w in (self.corner, self.top, self.left):
            w.setVisible(on)


def render_thumbnail(doc, size, path):
    """Render the default layout of a drawing to a PNG (white paper)."""
    from PySide6.QtGui import QImage
    scene, _ = render_scene(doc, default_layout(doc), light_bg=True, images=False)
    r = scene.itemsBoundingRect()
    if r.isEmpty():
        return False
    img = QImage(size, size, QImage.Format_ARGB32)
    img.fill(QColor(0, 0, 0, 0))
    m = max(r.width(), r.height()) * 0.03
    r = r.adjusted(-m, -m, m, m)
    s = min(size / r.width(), size / r.height())
    w, h = r.width() * s, r.height() * s
    target = QRectF((size - w) / 2, (size - h) / 2, w, h)
    p = QPainter(img)
    p.setRenderHints(QPainter.Antialiasing | QPainter.TextAntialiasing | QPainter.SmoothPixmapTransform)
    p.fillRect(target, QColor("white"))
    p.setPen(QPen(QColor(150, 150, 150), 1))
    p.drawRect(target.adjusted(0, 0, -1, -1))
    # flip Y: map scene rect upside down into the target
    p.translate(0, target.top() + target.bottom())
    p.scale(1, -1)
    scene.render(p, target, r, Qt.IgnoreAspectRatio)
    p.end()
    return img.save(path)
