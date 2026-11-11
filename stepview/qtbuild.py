"""numpy -> Qt geometry without per-point Python calls.

QPainterPath has a documented QDataStream wire format, so a whole path (any number of subpaths, lines and cubic
Beziers) is built from one numpy buffer: ~0.2 us per element instead of 1-2 us per moveTo / lineTo call. The same
conversion is done for ezdxf paths (command codes + vertices) in one vectorised pass, so a drawing of N entities costs
a few numpy calls instead of N Python loops.
"""
import struct

import numpy as np
from PySide6.QtCore import QByteArray, QDataStream
from PySide6.QtGui import QPainterPath

M_MOVE, M_LINE, M_CURVE, M_CDATA = 0, 1, 2, 3          # QPainterPath::ElementType
_REC = np.dtype([("t", ">i4"), ("x", ">f8"), ("y", ">f8")])      # QDataStream is big endian
_I4 = struct.Struct(">i")
_TAIL = struct.Struct(">ii")


def qpath_from_elements(types, xy, fill_rule=0):
    """types: (n,) ints 0..3 (a CurveTo must be followed by two CurveToData), xy: (n, 2) float64
    -> QPainterPath (fill_rule 0 = OddEvenFill, what ezdxf's to_qpainter_path produces). ValueError if Qt refuses."""
    n = len(types)
    path = QPainterPath()
    if n == 0:
        return path
    rec = np.empty(n, dtype=_REC)
    rec["t"] = types
    rec["x"] = xy[:, 0]
    rec["y"] = xy[:, 1]
    moves = np.flatnonzero(rec["t"] == M_MOVE)
    last_move = int(moves[-1]) if len(moves) else 0
    ba = QByteArray(_I4.pack(n) + rec.tobytes() + _TAIL.pack(last_move, fill_rule))
    ds = QDataStream(ba)
    ds.__rshift__(path)
    if path.elementCount() != n:
        raise ValueError("QPainterPath refused the element stream (%d of %d elements)" % (path.elementCount(), n))
    return path


# ezdxf command codes (ezdxf.path.Command)
C_LINE, C_CURVE3, C_CURVE4, C_MOVE = 1, 2, 3, 4
_CNT_IN = np.zeros(8, np.int64)      # input vertices consumed per command
_CNT_IN[[C_LINE, C_MOVE]] = 1
_CNT_IN[C_CURVE3] = 2
_CNT_IN[C_CURVE4] = 3
_CNT_OUT = np.zeros(8, np.int64)     # Qt elements produced per command
_CNT_OUT[[C_LINE, C_MOVE]] = 1
_CNT_OUT[[C_CURVE3, C_CURVE4]] = 3
_MOVE_CMD = np.array([C_MOVE], np.int64)


def _fill_elements(cmds, v, s, o, n_out):
    """Shared core: command codes `cmds` read vertices `v` from index `s` (per command) and write Qt elements from
    index `o`. A quadratic curve becomes the equivalent cubic exactly like QPainterPath.quadTo does."""
    ty = np.empty(n_out, np.uint8)
    xy = np.empty((n_out, 2), np.float64)
    m = (cmds == C_MOVE) | (cmds == C_LINE)
    ty[o[m]] = np.where(cmds[m] == C_MOVE, M_MOVE, M_LINE)
    xy[o[m]] = v[s[m]]
    m = cmds == C_CURVE4
    if m.any():
        oo, ss = o[m], s[m]
        for j in range(3):
            ty[oo + j] = M_CURVE if j == 0 else M_CDATA
            xy[oo + j] = v[ss + j]
    m = cmds == C_CURVE3
    if m.any():
        oo, ss = o[m], s[m]
        p0, q, e = v[ss - 1], v[ss], v[ss + 1]
        ty[oo], ty[oo + 1], ty[oo + 2] = M_CURVE, M_CDATA, M_CDATA
        xy[oo] = p0 + (q - p0) * (2.0 / 3.0)
        xy[oo + 1] = e + (q - e) * (2.0 / 3.0)
        xy[oo + 2] = e
    return ty, xy


def elements_from_path(vertices, commands):
    """ezdxf NumpyPath2d (vertices (N, 2), command codes (k,)) -> (types, xy) QPainterPath elements."""
    k = len(commands)
    v = np.asarray(vertices, np.float64)
    if k == 0:
        return np.zeros(1, np.uint8), v[:1]
    cmds = np.asarray(commands)
    c0 = cmds[0]
    if (cmds == c0).all():                              # the common cases: a polyline / a chain of arcs
        if c0 == C_LINE:
            ty = np.ones(k + 1, np.uint8)
            ty[0] = M_MOVE
            return ty, v
        if c0 == C_CURVE4:
            ty = np.full(3 * k + 1, M_CDATA, np.uint8)
            ty[0] = M_MOVE
            ty[1::3] = M_CURVE
            return ty, v
    # a leading MOVE command consumes the start vertex like every other command
    allc = np.concatenate((_MOVE_CMD, cmds))
    cin, cout = _CNT_IN[allc], _CNT_OUT[allc]
    s = np.cumsum(cin) - cin
    o = np.cumsum(cout) - cout
    return _fill_elements(allc, v, s, o, int(cout.sum()))


def elements_from_paths(paths):
    """List of ezdxf NumpyPath2d -> one (types, xy) with one subpath per path (what ezdxf.to_qpainter_path builds).
    All paths are converted in one vectorised pass."""
    if len(paths) == 1:
        return elements_from_path(paths[0]._vertices, paths[0]._commands)
    parts, verts = [], []
    for p in paths:
        parts.append(_MOVE_CMD)
        parts.append(p._commands)
        verts.append(p._vertices)
    cmds = np.concatenate(parts)
    v = np.concatenate(verts).astype(np.float64, copy=False)
    cin, cout = _CNT_IN[cmds], _CNT_OUT[cmds]
    s = np.cumsum(cin) - cin
    o = np.cumsum(cout) - cout
    return _fill_elements(cmds, v, s, o, int(cout.sum()))
