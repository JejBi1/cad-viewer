"""Fast scene builder of the 2D (DXF / DWG) viewer (1.9): a drawing becomes a few hundred big QGraphicsPathItems instead
of one QGraphicsItem per segment / entity.

How it works
  * BatchBackend (an ezdxf drawing backend) records every drawn primitive - a line, a polyline, a path, a filled
    polygon, a filled path (text glyphs) - in DRAWING ORDER together with its group (layer key + colour + line weight),
    the DXF entity it belongs to and its geometry as QPainterPath elements in numpy arrays.
  * z-order stays exact. Primitives of different groups whose (slightly inflated) bounding boxes can overlap keep their
    drawing order, everything else is free to be merged: assign_levels() gives every primitive a level, one level
    holds primitives of one group only, a level is drawn at z = level. (Rust implementation in native/rdxf, numpy /
    Python fallback below.)
  * Strokes of a level are merged into chunks of ~6000 path elements in Morton (space filling curve) order, so Qt can
    still skip what is outside the view; one chunk = one QGraphicsPathItem with a cosmetic pen. Fills (text, SOLID)
    stay one item per primitive, translucent strokes too (overlapping translucent shapes blend one by one).
  * Picking, snapping and selection work on the numpy arrays (Batch), not on QGraphicsItems; they reproduce what
    QGraphicsView.items() returned for the one-item-per-entity scene (see Batch.pick).
  * FastFrontend draws plain LINE / CIRCLE / ARC / LWPOLYLINE directly (properties resolved once per distinct set of
    attributes, geometry from numpy) and leaves everything else - text, dimensions, hatches, blocks, splines ... - to
    ezdxf's own frontend.
Any exception in here makes view2d.render_scene draw the legacy way (one item per primitive).
"""
import math
import os

import numpy as np
from PySide6.QtCore import QRectF, Qt
from PySide6.QtGui import QBrush, QColor, QPainterPath, QPainterPathStroker, QPen
from PySide6.QtWidgets import QGraphicsPathItem

from qtbuild import M_LINE, M_MOVE, elements_from_path, elements_from_paths, qpath_from_elements

K_LINES, K_POLY, K_PATH = 0, 1, 2          # geometry kinds of a primitive: segment pairs / one polyline / QPainterPath
CHUNK_ELEMS = 6000                         # QPainterPath elements per merged stroke item (~2000 segments)
BIG_PRIM = 3000                            # elements: above this, picking strokes only the segments near the cursor
MAX_LEVELS = 400000                        # the selection overlay (z 5e5) and the measure items (z 1e6) stay on top
PY_MAX_ENTITIES = 15000                    # without rdxf: more entities than this are drawn the legacy way ...
PY_HARD_MAX_PRIMS = 30000                  # ... and a drawing that turns out to need more primitives is given up
VISIT_BUDGET_PY, VISIT_BUDGET_NATIVE = 3e7, 4e8      # grid cell visits of assign_levels (coarser cells above this)
NOMERGE = bool(os.environ.get("STEPVIEW_DXF_NOMERGE"))    # test switch: one item per stroke primitive (exact blending)

try:                                       # native level assignment (native/rdxf, Rust); optional
    if os.environ.get("STEPVIEW_NO_RDXF"):
        raise ImportError("disabled by STEPVIEW_NO_RDXF")
    import rdxf as _rdxf
    if getattr(_rdxf, "API", 0) != 2:
        raise ImportError("rdxf API mismatch")
except Exception:                          # noqa: BLE001 - the Python fallback below does the same, only slower
    _rdxf = None


class FastPathError(RuntimeError):
    """The fast scene builder gives up (view2d draws the legacy way)."""


def have_native():
    return _rdxf is not None


# ------------------------------------------------------------------------------------------------ levels
def _bounds(bb, pad):
    x0, y0 = bb[:, 0].min() - pad, bb[:, 1].min() - pad
    w = max(bb[:, 2].max() + pad - x0, 1e-9)
    h = max(bb[:, 3].max() + pad - y0, 1e-9)
    return x0, y0, w, h


def _cell_ranges(bb, pad, cell, x0, y0, nx, ny):
    ix0 = np.clip(((bb[:, 0] - pad - x0) / cell).astype(np.int64), 0, nx - 1)
    iy0 = np.clip(((bb[:, 1] - pad - y0) / cell).astype(np.int64), 0, ny - 1)
    ix1 = np.clip(((bb[:, 2] + pad - x0) / cell).astype(np.int64), 0, nx - 1)
    iy1 = np.clip(((bb[:, 3] + pad - y0) / cell).astype(np.int64), 0, ny - 1)
    return ix0, iy0, ix1, iy1


def grid_cell(bb, pad, budget):
    """Grid cell size for assign_levels: 1.5 x the median primitive extent (at most 512 cells along the drawing), made
    coarser until the total number of cells all bounding boxes touch is within `budget` (a few huge primitives
    must not cost P x grid). Coarser cells only mean more false overlaps, i.e. less merging - never a wrong order."""
    x0, y0, w, h = _bounds(bb, pad)
    ext = np.maximum(bb[:, 2] - bb[:, 0], bb[:, 3] - bb[:, 1])
    cell = max(float(np.median(ext)) * 1.5, max(w, h) / 512.0)
    for _ in range(16):
        nx, ny = int(w / cell) + 1, int(h / cell) + 1
        ix0, iy0, ix1, iy1 = _cell_ranges(bb, pad, cell, x0, y0, nx, ny)
        if float(((ix1 - ix0 + 1) * (iy1 - iy0 + 1)).sum()) <= budget:
            break
        cell *= 1.6
    return cell


def assign_levels_py(bb, gid, pad, cell):
    """Python / numpy reference of rdxf.assign_levels (identical result). bb (P, 4) x0 y0 x1 y1, gid (P,) >= 0.
    Primitive i goes to the lowest level of its own group above every level already used by a different group in
    the grid cells its inflated bounding box touches. Primitives of one group look the same, so they commute."""
    P = len(gid)
    level = np.empty(P, np.int32)
    if P == 0:
        return level
    x0, y0, w, h = _bounds(bb, pad)
    nx, ny = int(w / cell) + 1, int(h / cell) + 1
    ix0, iy0, ix1, iy1 = (a.tolist() for a in _cell_ranges(bb, pad, cell, x0, y0, nx, ny))
    best_l = np.full((ny, nx), -1, np.int32)           # highest level in the cell
    best_g = np.full((ny, nx), -1, np.int32)           # ... and its group
    second_l = np.full((ny, nx), -1, np.int32)         # highest level of any group other than best_g
    glevels = {}                                       # group -> its levels, ascending
    nlev = 0
    for i, g in enumerate(gid.tolist()):
        sl = (slice(iy0[i], iy1[i] + 1), slice(ix0[i], ix1[i] + 1))
        bg, bl, sc = best_g[sl], best_l[sl], second_l[sl]
        m = int(np.where(bg != g, bl, sc).max())
        lv = glevels.setdefault(g, [])
        L = -1
        for cand in lv:                                # lowest own level above m
            if cand > m:
                L = cand
                break
        if L < 0:
            L = nlev
            nlev += 1
            lv.append(L)
        level[i] = L
        up = L > bl                                    # L becomes the best level of the cell
        same = bg == g
        second_l[sl] = np.where(up & ~same, bl, np.where(~same, np.maximum(sc, L), sc))
        best_l[sl] = np.where(up, L, bl)
        best_g[sl] = np.where(up, g, bg)
    return level


def assign_levels(bb, gid, pad):
    bb = np.ascontiguousarray(bb, np.float64)
    gid = np.ascontiguousarray(gid, np.int32)
    cell = grid_cell(bb, pad, VISIT_BUDGET_NATIVE if _rdxf is not None else VISIT_BUDGET_PY)
    if _rdxf is not None:
        return _rdxf.assign_levels(bb, gid, float(pad), float(cell))
    return assign_levels_py(bb, gid, pad, cell)


def morton(x, y):
    """Interleave the bits of two uint arrays in [0, 65536) (Z-order curve index)."""
    def spread(v):
        v = v.astype(np.uint64) & 0xFFFF
        v = (v | (v << 8)) & 0x00FF00FF
        v = (v | (v << 4)) & 0x0F0F0F0F
        v = (v | (v << 2)) & 0x33333333
        v = (v | (v << 1)) & 0x55555555
        return v
    return spread(x) | (spread(y) << np.uint64(1))


# ------------------------------------------------------------------------------------------------ items
_EMPTY_PATH = None


class ChunkItem(QGraphicsPathItem):
    """Merged strokes. QGraphicsPathItem derives its bounding rect from the stroked outline of the WHOLE path, which
    costs a stroker run over thousands of elements and would be the build-time bottleneck: the rectangle comes from
    numpy instead. The shape is empty, so Qt hit testing skips the chunk (Batch.pick works on the arrays - the stroked
    shape of a 6000-element path for every query would be far too slow)."""
    _batch_chunk = True

    def __init__(self, path, rect):
        super().__init__(path)
        self._rect = rect

    def boundingRect(self):
        return self._rect

    def shape(self):
        global _EMPTY_PATH
        if _EMPTY_PATH is None:
            _EMPTY_PATH = QPainterPath()
        return _EMPTY_PATH


class FillItem(QGraphicsPathItem):
    """A filled primitive (text glyphs, SOLID). The bounding rect comes from numpy as in ChunkItem; shape() and hit
    testing stay stock (computed by Qt when a click needs them)."""
    _batch_chunk = False

    def __init__(self, path, rect):
        super().__init__(path)
        self._rect = rect

    def boundingRect(self):
        return self._rect


def _qcolor(c):
    return QColor(c) if len(c) == 7 else QColor(f"#{c[7:9]}{c[1:7]}")      # '#RRGGBB' / '#RRGGBBAA' -> '#AARRGGBB'


# ------------------------------------------------------------------------------------------------ result of a build
class Batch:
    """The drawn primitives as arrays (p_* per primitive, e_* per path element) plus the scene items; picking,
    snapping and selection. Primitive index = drawing order = stacking order of the one-item-per-entity scene."""

    def __init__(self):
        self.groups = {}            # layer key tuple -> [item]   (for view2d.LayerMap)
        self.chunks = []            # scene items; chunk id = index; p_chunk maps a primitive to its item
        self.ents, self.tops = [], []       # current entity / top-level entity of each primitive (indexed by p_ent / p_top)
        self.top_index = {}         # id(top level entity) -> index in tops
        self.gdefs = []             # group id -> (kind 'S' stroke / 'F' filled path / 'P' polygon / 'I' item, layer key, colour, lineweight)
        self.lw_scale = 1.0

    # ---- helpers
    def chunks_visible(self, prims):
        """bool mask: is the scene item of each primitive visible (layer toggles hide / show items)?"""
        vis, out = {}, np.empty(len(prims), bool)
        for j, c in enumerate(self.p_chunk[prims].tolist()):
            v = vis.get(c)
            if v is None:
                v = vis[c] = self.chunks[c].isVisible()
            out[j] = v
        return out

    def prim_elements(self, p):
        s = int(self.p_estart[p])
        return self.e_ty[s:s + int(self.p_ecount[p])], self.e_xy[s:s + int(self.p_ecount[p])]

    def prims_of_top(self, top):
        ti = self.top_index.get(id(top))
        return np.flatnonzero(self.p_top == ti) if ti is not None else np.zeros(0, np.int64)

    def entities_of(self, p):
        """(top level entity, entity) of primitive p."""
        return self.tops[self.p_top[p]], self.ents[self.p_ent[p]]

    # ---- picking
    def pick(self, r):
        """Index of the topmost visible STROKE primitive hit by the scene rectangle r (QRectF), else -1.
        Reproduces QGraphicsView.items() of the one-item-per-entity scene: the shape() of a path item is the path
        stroked with the pen width taken as SCENE units (square caps, even for a cosmetic pen) plus the (even-odd)
        interior of the path; the topmost hit is the one drawn last."""
        x0, y0, x1, y1 = r.left(), r.top(), r.right(), r.bottom()
        pb, hw = self.p_bb, self.p_hw
        mg = hw * 1.5                                    # stroke reach incl. the corners of square caps
        m = (pb[:, 0] - mg <= x1) & (pb[:, 2] + mg >= x0) & (pb[:, 1] - mg <= y1) & (pb[:, 3] + mg >= y0) & self.p_stroke
        cand = np.flatnonzero(m)
        if len(cand) == 0:
            return -1
        cand = cand[self.chunks_visible(cand)][::-1]     # latest drawn first
        stroker = QPainterPathStroker()
        stroker.setCapStyle(Qt.SquareCap)
        stroker.setJoinStyle(Qt.RoundJoin)
        for p in cand.tolist():
            ty, xy = self.prim_elements(p)
            w = float(hw[p]) * 2.0
            stroker.setWidth(w if w > 0.0 else 1e-8)
            if len(ty) > BIG_PRIM:                       # long polyline / hatch: only the segments near the rect
                lo = np.minimum(xy[:-1], xy[1:]) - mg[p]
                hi = np.maximum(xy[:-1], xy[1:]) + mg[p]
                k = np.flatnonzero((ty[1:] == M_LINE) & (lo[:, 0] <= x1) & (hi[:, 0] >= x0)
                                   & (lo[:, 1] <= y1) & (hi[:, 1] >= y0))
                if len(k) == 0:
                    continue
                pts = np.empty((2 * len(k), 2))
                pts[0::2], pts[1::2] = xy[k], xy[k + 1]
                t2 = np.ones(2 * len(k), np.uint8)
                t2[0::2] = M_MOVE
                shape = stroker.createStroke(qpath_from_elements(t2, pts))
            else:
                path = qpath_from_elements(ty, xy)
                shape = stroker.createStroke(path)
                shape.addPath(path)
            if shape.intersects(r):
                return p
        return -1

    # ---- snapping
    def snap_vertices(self, r):
        """On-curve vertices (move / line elements) inside the scene rect r of the visible primitives that can snap,
        with an 'is a plain LINE' flag per vertex. Returns (element index, xy, flag)."""
        x0, y0, x1, y1 = r.left(), r.top(), r.right(), r.bottom()
        pb = self.p_bb
        cand = np.flatnonzero((pb[:, 0] <= x1) & (pb[:, 2] >= x0) & (pb[:, 1] <= y1) & (pb[:, 3] >= y0) & ~self.p_nosnap)
        empty = (np.zeros(0, np.int64), self.e_xy[:0], np.zeros(0, bool))
        if len(cand) == 0:
            return empty
        cand = cand[self.chunks_visible(cand)]
        if len(cand) == 0:
            return empty
        cnt = self.p_ecount[cand]
        tot = int(cnt.sum())
        idx = np.repeat(self.p_estart[cand] - (np.cumsum(cnt) - cnt), cnt) + np.arange(tot)
        xy = self.e_xy[idx]
        ok = (xy[:, 0] >= x0) & (xy[:, 0] <= x1) & (xy[:, 1] >= y0) & (xy[:, 1] <= y1) & (self.e_ty[idx] <= M_LINE)
        idx = idx[ok]
        return idx, self.e_xy[idx], self.p_isline[self.e_prim[idx]]

    def snap(self, r, tr, px, py, lines):
        """(distance^2 in viewport pixels, x, y) of the vertex nearest to the viewport point (px, py) among the
        candidates inside the scene rect r, or None. tr = viewport transform. lines=False: vertices of paths /
        polylines, True: end points of plain LINE primitives. Distances are measured between whole pixels (QPoint)
        and ties go to the topmost primitive, like the one-item-per-entity loop it replaces."""
        idx, pts, isline = self.snap_vertices(r)
        m = isline if lines else ~isline
        if not m.any():
            return None
        idx, pts = idx[m], pts[m]

        def qround(v):                                  # QPointF.toPoint()
            return np.where(v >= 0, np.floor(v + 0.5), np.ceil(v - 0.5))
        vx = qround(tr.m11() * pts[:, 0] + tr.m21() * pts[:, 1] + tr.dx()) - px
        vy = qround(tr.m12() * pts[:, 0] + tr.m22() * pts[:, 1] + tr.dy()) - py
        d = vx * vx + vy * vy
        tie = np.flatnonzero(d == d.min())
        pr = self.e_prim[idx[tie]]
        if lines:       # the segments of one primitive were separate items: the later one is met first, start before end
            rel = idx[tie] - self.p_estart[pr]
            order = np.lexsort((rel % 2, -(rel // 2), -pr))
        else:
            order = np.lexsort((idx[tie], -pr))
        k = int(tie[order[0]])
        return float(d[k]), float(pts[k, 0]), float(pts[k, 1])

    # ---- selection
    def highlight(self, scene, top, color):
        """Orange overlay for every visible primitive of the top level entity `top`: [(item, None)] for view2d's
        selection list (overlay items are removed again by DrawingView.clear_selection). The original recoloured the
        pen of each entity item, width max(2, its width)."""
        prims = self.prims_of_top(top)
        if len(prims):
            prims = prims[self.chunks_visible(prims)]
        by_width, extra = {}, []
        for p in prims.tolist():
            kind, _lk, _col, lw = self.gdefs[self.p_gid[p]]
            if kind == "I":
                it = self.chunks[self.p_chunk[p]]
                if hasattr(it, "pen"):
                    extra.append(it)
                continue
            # polygons (SOLID, wide polylines) have no pen: the 1.8 selection recoloured a pen that is not drawn, so they
            # showed no highlight but were part of the selection (zoom to selection). Same here: an invisible item.
            by_width.setdefault(0.0 if kind == "P" else max(2.0, lw / 0.3527 * self.lw_scale), []).append(p)
        out = []
        for w, ps in by_width.items():
            ty = np.concatenate([self.prim_elements(p)[0] for p in ps])
            xy = np.concatenate([self.prim_elements(p)[1] for p in ps])
            it = QGraphicsPathItem(qpath_from_elements(ty, xy))
            if w == 0.0:
                it.setPen(QPen(Qt.NoPen))
            else:
                pen = QPen(color)
                pen.setCosmetic(True)
                pen.setWidthF(w)
                pen.setJoinStyle(Qt.RoundJoin)
                it.setPen(pen)
            it.setZValue(5e5)
            it._overlay = True
            scene.addItem(it)
            out.append((it, None))
        return out + [(it, None) for it in extra]


# ------------------------------------------------------------------------------------------------ backend
_backend_cls = None


def backend_class():
    global _backend_cls
    if _backend_cls is not None:
        return _backend_cls
    from ezdxf.addons.drawing.backend import Backend
    from ezdxf.addons.drawing.pyqt import PyQtBackend
    from ezdxf.entities import Face3d, Insert, Viewport
    blind = (Insert, Face3d, Viewport)             # ezdxf ignores the layer of these when deciding visibility

    class BatchBackend(Backend):
        """Collects the primitives of a drawing; build() turns them into scene items."""

        def __init__(self, scene):
            super().__init__()
            self.scene = scene
            self._pq = PyQtBackend(scene)          # reuses ezdxf's item factories for points / images (rare)
            self._pq.entity_stack = self.entity_stack
            self._pq._add_item = self._misc_item
            self.groups = {}                       # set by view2d: layer key -> [item]
            self.gkey, self.gdef = {}, []          # group key -> id ; id -> group key
            self.p_gid, self.p_kind, self.p_xy, self.p_ty = [], [], [], []
            self.p_ent, self.p_top, self.p_isline = [], [], []
            self.misc = {}                         # primitive index -> ready-made QGraphicsItem (points, images)
            self.ents, self.tops = [], []
            self._ent_i, self._top_i = {}, {}

        # ---- ezdxf plumbing
        def configure(self, config):
            if config.min_lineweight is None:
                config = config.with_changes(min_lineweight=0.24)
            super().configure(config)
            self._pq.configure(config)

        def set_background(self, color):
            self._pq.set_background(color)

        def clear(self):
            pass

        def finalize(self):
            pass

        def _layerkey(self):
            """The layers this primitive's visibility depends on (see view2d.LayerMap)."""
            key = []
            for _e, p in self.entity_stack:
                if not isinstance(_e, blind):
                    k = p.layer.lower()
                    if k not in key:
                        key.append(k)
            return tuple(key)

        def get_group(self, kind, color, lineweight):
            k = (kind, self._layerkey(), color, lineweight if kind != "P" else 0.0)
            g = self.gkey.get(k)
            if g is None:
                g = self.gkey[k] = len(self.gdef)
                self.gdef.append(k)
            return g

        def add_prim(self, gid, geo_kind, xy, ty=None, isline=False):
            st = self.entity_stack
            e, top = st[-1][0], st[0][0]
            ei = self._ent_i.get(id(e))
            if ei is None:
                ei = self._ent_i[id(e)] = len(self.ents)
                self.ents.append(e)
            ti = self._top_i.get(id(top))
            if ti is None:
                ti = self._top_i[id(top)] = len(self.tops)
                self.tops.append(top)
            self.p_gid.append(gid)
            self.p_kind.append(geo_kind)
            self.p_xy.append(xy)
            self.p_ty.append(ty)
            self.p_ent.append(ei)
            self.p_top.append(ti)
            self.p_isline.append(isline)

        def _misc_item(self, item, _handle):       # an item made by PyQtBackend: a point or an image
            gid = self.get_group("I", "", len(self.misc))          # its own group (own level)
            self.add_prim(gid, K_LINES, np.zeros((0, 2)))
            self.misc[len(self.p_gid) - 1] = item

        # ---- drawing primitives
        def draw_point(self, pos, properties):
            self._pq.draw_point(pos, properties)

        def draw_image(self, image_data, properties):
            self._pq.draw_image(image_data, properties)

        def draw_line(self, start, end, properties):
            if start.isclose(end):                 # Qt draws a long line for a zero length line
                self.draw_point(start, properties)
                return
            g = self.get_group("S", properties.color, properties.lineweight)
            self.add_prim(g, K_LINES, np.array(((start.x, start.y), (end.x, end.y))), isline=True)

        def draw_solid_lines(self, lines, properties):
            keep = []
            for s, e in lines:
                if s.isclose(e):
                    self.draw_point(s, properties)
                else:
                    keep.append((s.x, s.y, e.x, e.y))
            if keep:
                g = self.get_group("S", properties.color, properties.lineweight)
                self.add_prim(g, K_LINES, np.array(keep, np.float64).reshape(-1, 2), isline=True)

        def draw_path(self, path, properties):
            if len(path) == 0:
                return
            ty, xy = elements_from_path(path._vertices, path._commands)
            self.add_prim(self.get_group("S", properties.color, properties.lineweight), K_PATH, xy, ty)

        def draw_filled_paths(self, paths, properties):
            paths = list(paths)
            if paths:
                ty, xy = elements_from_paths(paths)
                self.add_prim(self.get_group("F", properties.color, properties.lineweight), K_PATH, xy, ty)

        def draw_filled_polygon(self, points, properties):
            v = np.asarray(points._vertices, np.float64)
            if len(v):
                self.add_prim(self.get_group("P", properties.color, 0.0), K_POLY, v)

        # ---- the scene
        def build(self):
            """Assign levels, merge strokes into chunks, add every item to the scene. Returns the Batch."""
            B = Batch()
            P = len(self.p_gid)
            B.ents, B.tops = self.ents, self.tops
            B.top_index = {id(t): i for i, t in enumerate(self.tops)}
            B.gdefs, B.lw_scale = self.gdef, self.config.lineweight_scaling
            if P == 0:                                # nothing drawn: an empty, well-formed batch
                B.p_gid, B.p_estart, B.p_ecount = np.zeros(0, np.int32), np.zeros(0, np.int64), np.zeros(0, np.int64)
                B.p_top = B.p_ent = B.p_chunk = np.zeros(0, np.int32)
                B.p_isline = B.p_stroke = B.p_nosnap = np.zeros(0, bool)
                B.p_bb, B.p_hw = np.zeros((0, 4)), np.zeros(0)
                B.e_ty, B.e_xy, B.e_prim = np.zeros(0, np.uint8), np.zeros((0, 2)), np.zeros(0, np.int32)
                return B
            if P > PY_HARD_MAX_PRIMS and _rdxf is None:
                raise FastPathError("%d primitives need the native level assignment (rdxf)" % P)
            gid = np.array(self.p_gid, np.int32)
            ecount = np.fromiter((len(a) for a in self.p_xy), np.int64, P)
            estart = np.cumsum(ecount) - ecount
            N = int(ecount.sum())
            e_xy = np.concatenate(self.p_xy).reshape(-1, 2) if N else np.zeros((0, 2))
            if not np.isfinite(e_xy).all():
                raise FastPathError("non-finite coordinates")
            e_prim = np.repeat(np.arange(P, dtype=np.int32), ecount)
            # element types: lines / polylines are implied by the kind, paths bring their own
            kind = np.array(self.p_kind, np.int8)
            rel = np.arange(N) - estart[e_prim]
            ke = kind[e_prim]
            e_ty = np.ones(N, np.uint8)
            e_ty[((ke == K_LINES) & (rel % 2 == 0)) | ((ke == K_POLY) & (rel == 0))] = M_MOVE
            for p in np.flatnonzero(kind == K_PATH).tolist():
                e_ty[estart[p]:estart[p] + ecount[p]] = self.p_ty[p]
            # bounding boxes (items without elements: their scene rectangle)
            bb = np.zeros((P, 4))
            ne = np.flatnonzero(ecount > 0)
            if len(ne):
                st = estart[ne]
                bb[ne, 0] = np.minimum.reduceat(e_xy[:, 0], st)
                bb[ne, 1] = np.minimum.reduceat(e_xy[:, 1], st)
                bb[ne, 2] = np.maximum.reduceat(e_xy[:, 0], st)
                bb[ne, 3] = np.maximum.reduceat(e_xy[:, 1], st)
            for p, it in self.misc.items():
                r = it.sceneBoundingRect()
                bb[p] = (r.left(), r.top(), r.right(), r.bottom())
            if not np.isfinite(bb).all():
                raise FastPathError("non-finite bounding box")
            fw, fh = bb[:, 2].max() - bb[:, 0].min(), bb[:, 3].max() - bb[:, 1].min()
            pad = 0.003 * max(fw, fh) + 1e-9
            level = assign_levels(bb, gid, pad)
            if int(level.max()) >= MAX_LEVELS:
                raise FastPathError("%d levels" % (int(level.max()) + 1))
            # per-primitive / per-element arrays for picking, snapping, selection
            lw_scale = self.config.lineweight_scaling
            gkind = np.array([g[0] for g in self.gdef])
            B.p_gid, B.p_estart, B.p_ecount = gid, estart, ecount
            B.p_top, B.p_ent = np.array(self.p_top, np.int32), np.array(self.p_ent, np.int32)
            B.p_isline = np.array(self.p_isline, bool)
            B.p_bb = bb
            B.p_stroke = (gkind == "S")[gid]
            B.p_nosnap = np.isin(gkind, ("P", "I"))[gid]
            B.p_hw = np.array([(g[3] / 0.3527 * lw_scale * 0.5) if g[0] in ("S", "F") else 0.0 for g in self.gdef])[gid]
            B.p_chunk = np.full(P, -1, np.int32)
            B.e_ty, B.e_xy, B.e_prim = e_ty, e_xy, e_prim

            qcache = {}

            def color(c):
                q = qcache.get(c)
                if q is None:
                    q = qcache[c] = _qcolor(c)
                return q

            def pen_for(c, lw):
                pen = QPen(color(c), lw / 0.3527 * lw_scale)       # as ezdxf's PyQtBackend._get_pen
                pen.setCosmetic(True)
                pen.setJoinStyle(Qt.RoundJoin)
                return pen
            no_pen, no_brush = QPen(Qt.NoPen), QBrush(Qt.NoBrush)

            scene = self.scene
            order = np.argsort(level, kind="stable")                 # by level, drawing order within a level
            cx, cy = (bb[:, 0] + bb[:, 2]) * 0.5, (bb[:, 1] + bb[:, 3]) * 0.5
            ox, oy = bb[:, 0].min(), bb[:, 1].min()
            mort = morton(np.clip((cx - ox) * (65535.0 / max(fw, 1e-12)), 0, 65535).astype(np.uint32),
                          np.clip((cy - oy) * (65535.0 / max(fh, 1e-12)), 0, 65535).astype(np.uint32))
            for grp in np.split(order, np.flatnonzero(np.diff(level[order])) + 1):
                kind_s, lk, col, lw = self.gdef[int(gid[grp[0]])]
                z = float(level[grp[0]])
                lst = B.groups.setdefault(lk, [])

                def put(it, prims):
                    it.setZValue(z)
                    scene.addItem(it)
                    lst.append(it)
                    B.chunks.append(it)
                    B.p_chunk[prims] = len(B.chunks) - 1
                if kind_s == "I":
                    for p in grp.tolist():
                        self.misc[p].setData(0, p)
                        put(self.misc[p], p)
                elif kind_s in ("F", "P"):                           # fills: one item per primitive
                    pen = pen_for(col, lw) if kind_s == "F" else no_pen
                    brush = QBrush(color(col), Qt.SolidPattern)
                    pw = pen.widthF() * 0.5 if kind_s == "F" else 0.0
                    for p in grp.tolist():
                        s, n = int(estart[p]), int(ecount[p])
                        rect = QRectF(bb[p, 0] - pw, bb[p, 1] - pw, bb[p, 2] - bb[p, 0] + 2 * pw, bb[p, 3] - bb[p, 1] + 2 * pw)
                        it = FillItem(qpath_from_elements(e_ty[s:s + n], e_xy[s:s + n]), rect)
                        it.setPen(pen)
                        it.setBrush(brush)
                        it.setData(0, p)
                        put(it, p)
                else:                                                # strokes
                    pen = pen_for(col, lw)
                    pw = pen.widthF() * 0.5       # bounding rect margin: half the pen width, like the stroked shape
                    if NOMERGE or (len(col) == 9 and col[7:9].lower() != "ff"):
                        parts = [grp[i:i + 1] for i in range(len(grp))]  # translucent: overlaps blend per item
                    else:
                        srt = grp[np.argsort(mort[grp], kind="stable")]
                        cum = np.cumsum(ecount[srt])
                        parts = np.split(srt, np.flatnonzero(np.diff((cum - ecount[srt]) // CHUNK_ELEMS)) + 1)
                    for idx in parts:
                        if len(idx) == 1:                                # one primitive: a view into the arrays
                            q = int(idx[0])
                            s0, n0 = int(estart[q]), int(ecount[q])
                            ty_c, xy_c = e_ty[s0:s0 + n0], e_xy[s0:s0 + n0]
                            x0, y0, x1, y1 = bb[q]
                        else:                                            # gather the elements of the primitives
                            cnt = ecount[idx]
                            offs = np.repeat(estart[idx] - (np.cumsum(cnt) - cnt), cnt) + np.arange(int(cnt.sum()))
                            ty_c, xy_c = e_ty[offs], e_xy[offs]
                            x0, y0, x1, y1 = bb[idx, 0].min(), bb[idx, 1].min(), bb[idx, 2].max(), bb[idx, 3].max()
                        it = ChunkItem(qpath_from_elements(ty_c, xy_c), QRectF(x0 - pw, y0 - pw, x1 - x0 + 2 * pw, y1 - y0 + 2 * pw))
                        it.setPen(pen)
                        it.setBrush(no_brush)
                        put(it, idx)
            return B

    _backend_cls = BatchBackend
    return _backend_cls


# ------------------------------------------------------------------------------------------------ fast entity path
_TAU = 2.0 * math.pi


def arc_vertices(cx, cy, r, a0, a1):
    """Cubic Bezier control polygon (3n + 1, 2) of the counter-clockwise arc a0 -> a1 (radians, span > 0): what
    ezdxf's cubic_bezier_arc_parameters gives (one cubic per started quarter, equal split)."""
    span = a1 - a0
    n = max(math.ceil(span / math.pi * 2.0), 1)
    seg = span / n
    t = (4.0 / 3.0) * math.tan(seg / 4.0)
    ang = a0
    ex, ey = math.cos(ang), math.sin(ang)
    out = [(cx + r * ex, cy + r * ey)]
    for _ in range(n):
        sx, sy = ex, ey
        ang += seg
        ex, ey = math.cos(ang), math.sin(ang)
        out.append((cx + r * (sx - sy * t), cy + r * (sy + sx * t)))
        out.append((cx + r * (ex + ey * t), cy + r * (ey - ex * t)))
        out.append((cx + r * ex, cy + r * ey))
    return np.array(out, np.float64)


_frontend_cls = None


def frontend_class():
    """ezdxf Frontend that draws plain LINE / CIRCLE / ARC / LWPOLYLINE straight into the BatchBackend and hands every
    other entity (and everything inside a clipped block / viewport, dashed lines, polylines with width or bulge ...)
    to the stock code. Geometry equals what ezdxf produces (arcs to 1e-14); properties are resolved once per distinct
    set of attributes."""
    global _frontend_cls
    if _frontend_cls is not None:
        return _frontend_cls
    from ezdxf.addons.drawing import Frontend
    from ezdxf.addons.drawing.config import ProxyGraphicPolicy
    from ezdxf.addons.drawing.frontend import _draw_entities, _draw_viewports
    from ezdxf.entities import Arc, Circle, Line, LWPolyline, Viewport
    from ezdxf.math import Vec3
    isclose = math.isclose
    FAST = (Line, Circle, Arc, LWPolyline)
    Z = Vec3(0, 0, 1)

    class FastFrontend(Frontend):
        def __init__(self, ctx, out, *args, **kwargs):
            super().__init__(ctx, out, *args, **kwargs)
            self._out = out
            self._fp = {}                          # attribute key -> (properties, visible, continuous, backend properties)
            self.fast_count = self.slow_count = 0
            try:                                   # no user property overrides (only ezdxf's own empty stub)
                self._fast_on = (self.config.proxy_graphic_policy != ProxyGraphicPolicy.PREFER
                                 and len(self._property_override_functions) == 1
                                 and hasattr(self.pipeline.clipping_portal, "is_active"))
            except Exception:                      # noqa: BLE001 - another ezdxf version: stock drawing
                self._fast_on = False

        def draw_viewport(self, vp):
            self._fp.clear()                       # a viewport may freeze layers on its own
            try:
                super().draw_viewport(vp)
            finally:
                self._fp.clear()

        def draw_entities(self, entities, *, filter_func=None):
            if filter_func is not None or not self._fast_on:
                return super().draw_entities(entities, filter_func=filter_func)
            viewports = []
            ctx = self.ctx
            for entity in entities:
                if type(entity) in FAST and self._fast(entity):
                    self.fast_count += 1
                    continue
                if isinstance(entity, Viewport):
                    viewports.append(entity)
                    continue
                self.slow_count += 1
                _draw_entities(self, ctx, (entity,))
            _draw_viewports(self, viewports)

        def _fast(self, e):
            """True when `e` has been drawn here."""
            if self.pipeline.clipping_portal.is_active:
                return False
            d = e.dxf.__dict__
            bp = self.ctx.current_block_reference_properties
            bkey = None if bp is None else (bp.layer, bp.color, bp.pen, bp.linetype_name, bp.lineweight,
                                            bp.linetype_scale, bp.is_visible)
            key = (type(e), d.get("layer"), d.get("color"), d.get("true_color"), d.get("linetype"),
                   d.get("lineweight"), d.get("ltscale"), d.get("transparency"), d.get("invisible"), bkey)
            ent = self._fp.get(key)
            if ent is None:
                props = self.ctx.resolve_all(e)
                ent = self._fp[key] = (props, props.is_visible, self.config.line_policy.name == "SOLID"
                                       or len(props.linetype_pattern) < 2, self.pipeline.get_backend_properties(props))
            props, visible, continuous, bprops = ent
            if not visible:
                return True                          # stock behaviour: an invisible entity draws nothing
            if not continuous:
                return False                         # dashed: the linetype stage of the stock pipeline
            try:
                geo = self._geometry(e, d)
            except Exception:                        # noqa: BLE001 - anything unusual: stock drawing
                return False
            if geo is None:
                return False
            be = self._out
            st = be.entity_stack
            st.append((e, props))                    # what pipeline.enter_entity() does
            try:
                be.add_prim(be.get_group("S", bprops.color, bprops.lineweight), geo[0], geo[1], geo[2], geo[3])
            finally:
                st.pop()
            return True

        @staticmethod
        def _geometry(e, d):
            """(kind, xy, types or None, is plain line) or None for what the stock code has to draw."""
            t = type(e)
            if t is Line:
                s, en = d.get("start"), d.get("end")
                if s is None or en is None:
                    return None
                if isclose(s.x, en.x, rel_tol=1e-9, abs_tol=1e-12) and isclose(s.y, en.y, rel_tol=1e-9, abs_tol=1e-12):
                    return None                      # a point: the stock code decides
                return K_LINES, np.array(((s.x, s.y), (en.x, en.y))), None, True
            ext = d.get("extrusion")
            if ext is not None and ext != Z:
                return None
            if t is LWPolyline:
                if d.get("const_width"):
                    return None
                pts = np.frombuffer(e.lwpoints.values, np.float64).reshape(-1, 5)
                if len(pts) < 2 or pts[:, 2:4].any() or (np.abs(pts[:, 4]) >= 1e-6).any():
                    return None                      # widths / bulges
                xy = pts[:, :2]
                if e.closed and not (isclose(xy[0, 0], xy[-1, 0], rel_tol=1e-12, abs_tol=0)
                                     and isclose(xy[0, 1], xy[-1, 1], rel_tol=1e-12, abs_tol=0)):
                    xy = np.concatenate([xy, xy[:1]])
                return K_POLY, xy, None, False
            c = d.get("center")
            r = abs(d.get("radius", 1.0))
            if c is None or r <= 1e-12:
                return None
            if t is Circle:
                v = arc_vertices(c.x, c.y, r, 0.0, _TAU)[::-1].copy()   # ezdxf reverses the path of a full circle ...
                v[0] = (c.x + r, c.y)                                    # ... and starts it at the exact start point
            else:
                a0 = math.radians(d.get("start_angle", 0.0)) % _TAU
                a1 = math.radians(d.get("end_angle", 360.0)) % _TAU
                if a1 <= a0:
                    a1 += _TAU
                if abs(a1 - a0) < 1e-6 or abs(a1 - a0 - _TAU) < 1e-6:
                    return None                      # degenerate arc / full circle: the stock code decides
                v = arc_vertices(c.x, c.y, r, a0, a1)
            ty = np.full(len(v), 3, np.uint8)
            ty[0] = 0
            ty[1::3] = 2
            return K_PATH, v, ty, False

    _frontend_cls = FastFrontend
    return _frontend_cls


def draw_layout(ctx, layout, cfg, scene):
    """Draws `layout` into `scene` the batching way and sets scene.batch. Returns the backend: its .groups is
    {layer key tuple: [item]} for view2d.LayerMap."""
    backend = backend_class()(scene)
    frontend_class()(ctx, backend, config=cfg).draw_layout(layout, finalize=True)
    batch = backend.build()
    scene.batch = batch
    scene.setSceneRect(scene.itemsBoundingRect())          # what PyQtBackend.finalize() does
    backend.groups = batch.groups
    return backend
