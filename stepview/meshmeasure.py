"""Measuring on triangle meshes (STL): vertex snapping, facet normals, sharp-edge loops and hole (circle) fitting.

The B-rep tools (vertices / edges / faces) don't exist in an STL - it is one triangulation. Here the mesh is
analysed with numpy: sharp edges (dihedral angle > FEATURE_DEG) and open boundaries are chained into polylines,
closed / long polylines get a least-squares circle, so the rim of a drilled hole measures as Ø.
"""
import math
import os
import re
import struct

import numpy as np

FEATURE_DEG = 30.0          # dihedral angle that makes an edge "sharp"
FLAT_DEG = 0.5              # facets of one planar face deviate less than this
GRID_MIN_TRIS = 40_000      # from this size picking searches a facet grid instead of all facets
MAX_FEATURE_EDGES = 400_000  # scanned / organic meshes: don't try to chain millions of edges
MAX_TRIS = 3_000_000        # bigger meshes are only displayed (measuring data would need > 0.5 GB)
ASCII_FACET_BYTES = 400     # generous size of one ASCII facet (typically ~250 bytes) - file size limit
STL_RECORD = np.dtype([("n", "<f4", 3), ("v", "<f4", (3, 3)), ("a", "<u2")])
_VERTEX = re.compile(rb"vertex\s+(\S+)\s+(\S+)\s+(\S+)")


def _p(v, d=3):
    return f"{v:,.{d}f}".replace(",", " ").replace(".", ",")


def _read_ascii(f, chunk=1 << 24, max_tris=None):
    """Vertex coordinates (k,3) of an ASCII STL, parsed in 16 MB pieces (one regex over the whole file held
    ~10x its size in Python objects). With max_tris the facet count is enforced while parsing (the file size
    only bounds it loosely: a file of bare "vertex" lines holds 3x more facets per byte than a normal one) and
    None is returned as soon as it is exceeded."""
    parts, tail, nverts = [], b"", 0
    while True:
        data = f.read(chunk)
        buf = tail + data
        if data:
            cut = max(buf.rfind(b"\n"), buf.rfind(b"\r")) + 1
            if cut == 0:
                if len(buf) > 4 * chunk:                 # no line breaks at all: not an STL
                    return np.zeros((0, 3))
                tail = buf
                continue
            buf, tail = buf[:cut], buf[cut:]
        nums = _VERTEX.findall(buf)
        if nums:
            nverts += len(nums)
            if max_tris is not None and nverts > 3 * max_tris:
                return None
            parts.append(np.array(nums, dtype=np.float64))
        if not data:
            break
    v = np.concatenate(parts) if parts else np.zeros((0, 3))
    return v[:len(v) // 3 * 3]


class Loop:
    """A chain of sharp mesh edges; circle = (centre (3,), normal (3,), radius, rms) when it fits one."""
    __slots__ = ("pts", "closed", "length", "circle", "coverage", "wire")

    def __init__(self, pts, closed):
        self.pts = pts
        self.closed = closed
        seg = np.diff(np.vstack([pts, pts[:1]]) if closed else pts, axis=0)
        self.length = float(np.linalg.norm(seg, axis=1).sum())
        self.circle = None
        self.coverage = 0.0
        self.wire = None
        self._fit()

    def _fit(self):
        P = self.pts
        if len(P) < 6:
            return
        c0 = P.mean(axis=0)
        Q = P - c0
        try:
            _, s, vt = np.linalg.svd(Q, full_matrices=False)
        except np.linalg.LinAlgError:
            return
        if s[0] <= 0 or s[2] / s[0] > 0.02:            # not planar
            return
        u, v, n = vt[0], vt[1], vt[2]
        x, y = Q @ u, Q @ v
        A = np.column_stack([x, y, np.ones_like(x)])
        sol, *_ = np.linalg.lstsq(A, x * x + y * y, rcond=None)
        xc, yc = sol[0] / 2, sol[1] / 2
        r2 = sol[2] + xc * xc + yc * yc
        if r2 <= 0:
            return
        r = math.sqrt(r2)
        d = np.hypot(x - xc, y - yc)
        rms = float(np.sqrt(np.mean((d - r) ** 2)))
        if rms > 0.02 * r:
            return
        ang = np.sort(np.arctan2(y - yc, x - xc))
        gaps = np.diff(np.concatenate([ang, ang[:1] + 2 * math.pi]))
        coverage = 360.0 - math.degrees(gaps.max())
        if coverage < 60:                                # a tiny arc says nothing about a radius
            return
        self.coverage = coverage
        self.circle = (c0 + xc * u + yc * v, n, r, rms)

    def kind(self):
        if self.circle is None:
            return "obrys"
        return "kružnice" if self.coverage > 300 else "oblouk"


class Mesh:
    """Triangle mesh in world coordinates: nodes (N,3) float64 (measured values), tris (M,3) int32; per facet
    unit normals and centroids in float32 (only used for searching / grouping) - ~100 MB per million facets."""

    def __init__(self, nodes, tris):
        self.nodes = np.asarray(nodes, dtype=np.float64)
        self.tris = np.asarray(tris, dtype=np.int32)
        v0, v1, v2 = (self.nodes[self.tris[:, i]] for i in range(3))
        n = np.cross(v1 - v0, v2 - v0)
        ln = np.linalg.norm(n, axis=1)
        self.area = float(0.5 * ln.sum())
        n /= np.maximum(ln, 1e-300)[:, None]
        self.normals = n.astype(np.float32)
        v0 += v1
        v0 += v2
        v0 /= 3.0
        self.centroids = v0.astype(np.float32)
        del v0, v1, v2, n, ln
        self.boundary_edges = 0
        self.nonmanifold_edges = 0
        self._loops = None
        self._loop_pts = None
        self._loop_ids = None
        self._seg = None                  # loop segments (a (S,3), b (S,3), loop index (S,))
        self._sharp = None                # sharp / open edges (E,2) node pairs until the loops are chained
        self._nbr = None                  # (M,3) int32 facet neighbours across manifold edges, -1 = none
        self._region = None               # facet -> flat patch id (-1 = not computed yet)
        self._regions = []                # patch id -> (facets, area, is_face, centroid)
        self.display_cache = {}           # viewer: id(facets) -> highlight shape of that patch
        self._grid = None                 # facet grid (see prepare), False = mesh small enough to brute-force
        self._dtol = None                 # plane offset tolerance of flat patches

    # ---------------------------------------------------------- loading
    @staticmethod
    def from_stl(path, max_tris=MAX_TRIS):
        """Binary or ASCII STL -> Mesh; None when it has more than max_tris facets (known before reading it:
        binary = header count, ASCII = file size) or no valid facet."""
        size = os.path.getsize(path)
        with open(path, "rb") as f:
            head = f.read(84)
            count = struct.unpack_from("<I", head, 80)[0] if len(head) == 84 else -1
            if 84 + 50 * count == size:                  # binary: read just the vertex records
                if count > max_tris:
                    return None
                rec = np.fromfile(f, dtype=STL_RECORD, count=count)
                corners = np.ascontiguousarray(rec["v"]).reshape(-1, 3)      # float32
                del rec
            else:                                        # ASCII
                if size > max_tris * ASCII_FACET_BYTES:
                    return None
                f.seek(0)
                corners = _read_ascii(f, max_tris=max_tris)
                if corners is None:                      # more facets than max_tris, found while parsing
                    return None
        return Mesh.from_corners(corners)

    @staticmethod
    def from_corners(corners):
        """(3M,3) facet corners (STL repeats every vertex per facet) -> Mesh with shared nodes."""
        c = corners.reshape(-1, 9)
        ok = np.isfinite(c).all(axis=1)                  # NaN / inf coordinates: drop the facet
        if not ok.all():
            c = c[ok]
        corners = c.reshape(-1, 3)
        if not len(corners):
            return None
        # 21 bits per axis (2 m model -> 1 µm grid) packed into one int64: a 1-D sort instead of a row-wise unique
        lo = corners.min(axis=0).astype(np.float64)
        span = float((corners.max(axis=0).astype(np.float64) - lo).max())
        scale = (2 ** 21 - 2) / max(span, 1e-9)
        key = np.zeros(len(corners), dtype=np.int64)
        for ax, shift in ((0, 42), (1, 21), (2, 0)):
            key |= np.rint((corners[:, ax].astype(np.float64) - lo[ax]) * scale).astype(np.int64) << shift
        _, first, inv = np.unique(key, return_index=True, return_inverse=True)
        del key
        tris = inv.reshape(-1, 3).astype(np.int32)
        good = (tris[:, 0] != tris[:, 1]) & (tris[:, 1] != tris[:, 2]) & (tris[:, 0] != tris[:, 2])
        if not good.all():                               # collapsed facets would make false sharp edges
            tris = tris[good]
        if not len(tris):
            return None
        return Mesh(corners[first].astype(np.float64), tris)

    # ---------------------------------------------------------- picking helpers
    def nearest_triangle(self, p):
        """Facet closest to p (p is a picked point on the surface)."""
        p = np.asarray(p, dtype=np.float64)
        cand = None
        if self.prepare():
            h = self._grid[0]
            for r in (0.25 * h, h, 4 * h):
                cand = self._grid_tris(p, r)
                if cand is None or len(cand):
                    break
        if cand is None or not len(cand):
            cand = np.arange(len(self.tris))
        if len(cand) > 64:                                 # keep the 64 with the nearest centroids
            c = self.centroids[cand] - p
            cand = cand[np.argpartition(np.einsum("ij,ij->i", c, c), 63)[:64]]
        best, bd = int(cand[0]), float("inf")
        for t in cand.tolist():
            q = _closest_on_triangle(p, *(self.nodes[self.tris[t, i]] for i in range(3)))
            dd = float(np.dot(q - p, q - p))
            if dd < bd:
                best, bd = t, dd
        return best

    def nearest_vertex(self, p, r=None):
        """(vertex, distance) of the mesh vertex nearest to p; with r only vertices within ~r are searched
        (big meshes), (None, inf) when there is none."""
        p = np.asarray(p, dtype=np.float64)
        idx = None
        if r is not None and self.prepare():
            cand = self._grid_tris(p, r)
            if cand is not None:
                if not len(cand):
                    return None, float("inf")
                idx = np.unique(self.tris[cand])
        pts = self.nodes if idx is None else self.nodes[idx]
        d = np.einsum("ij,ij->i", pts - p, pts - p)
        i = int(np.argmin(d))
        return pts[i], math.sqrt(float(d[i]))

    # ---------------------------------------------------------- uniform grid of facets (big meshes)
    def prepare(self):
        """Build the facet grid for meshes where brute-force searching would lag the cursor; True = grid ready."""
        if self._grid is None:
            self._grid = self._grid_build() if len(self.tris) >= GRID_MIN_TRIS else False
        return self._grid is not False

    def _grid_build(self):
        T, P = self.tris, self.nodes
        v0, v1, v2 = P[T[:, 0]], P[T[:, 1]], P[T[:, 2]]
        bmin = np.minimum(np.minimum(v0, v1), v2)
        bmax = np.maximum(np.maximum(v0, v1), v2)
        del v0, v1, v2
        h = max(2.0 * float(np.median((bmax - bmin).max(axis=1))), 1e-9)   # cell ~ 2 typical facets
        o = bmin.min(axis=0)
        lo = np.floor((bmin - o) / h).astype(np.int64)
        n = np.floor((bmax - o) / h).astype(np.int64) - lo + 1
        dims = lo.max(axis=0) + n.max(axis=0) + 1
        small = (n <= 2).all(axis=1)                       # facets spanning <= 2 cells per axis go into cells
        si = np.nonzero(small)[0]
        keys, ids = [], []
        for dx in (0, 1):
            for dy in (0, 1):
                for dz in (0, 1):
                    t = si[(dx < n[si, 0]) & (dy < n[si, 1]) & (dz < n[si, 2])]
                    c = lo[t] + (dx, dy, dz)
                    keys.append((c[:, 0] * dims[1] + c[:, 1]) * dims[2] + c[:, 2])
                    ids.append(t.astype(np.int32))
        keys, ids = np.concatenate(keys), np.concatenate(ids)
        order = np.argsort(keys, kind="stable")
        big = np.nonzero(~small)[0]                        # long slivers: tested by their bounding box
        return (h, o, dims, keys[order], ids[order], big, bmin[big], bmax[big])

    def _grid_tris(self, p, r):
        """Facets whose cells / bounding boxes lie within r of p (None = too many cells, search everything)."""
        h, o, dims, keys, ids, big, bmin, bmax = self._grid
        lo = np.maximum(np.floor((p - r - o) / h).astype(np.int64), 0)
        hi = np.minimum(np.floor((p + r - o) / h).astype(np.int64), dims - 1)
        if (hi < lo).any():
            out = []
        elif (hi[0] - lo[0] + 1) * (hi[1] - lo[1] + 1) > 400:
            return None
        else:
            out = []
            for i in range(int(lo[0]), int(hi[0]) + 1):
                for j in range(int(lo[1]), int(hi[1]) + 1):
                    k0 = (i * int(dims[1]) + j) * int(dims[2])
                    a = int(np.searchsorted(keys, k0 + int(lo[2])))
                    b = int(np.searchsorted(keys, k0 + int(hi[2]), side="right"))
                    if b > a:
                        out.append(ids[a:b])
        if len(big):
            out.append(big[((bmin <= p + r) & (bmax >= p - r)).all(axis=1)])
        out = [a for a in out if len(a)]
        return np.unique(np.concatenate(out)).astype(np.int64) if out else np.zeros(0, dtype=np.int64)

    # ---------------------------------------------------------- sharp edges / loops
    def loops(self):
        if self._loops is None:
            self._loops = self._build_loops()
            pts, ids, sa, sb, sid = [], [], [], [], []
            for i, lp in enumerate(self._loops):
                pts.append(lp.pts)
                ids.append(np.full(len(lp.pts), i))
                b = np.roll(lp.pts, -1, axis=0) if lp.closed else lp.pts[1:]
                a = lp.pts if lp.closed else lp.pts[:-1]
                sa.append(a)
                sb.append(b)
                sid.append(np.full(len(a), i))
            self._loop_pts = np.vstack(pts) if pts else np.zeros((0, 3))
            self._loop_ids = np.concatenate(ids) if ids else np.zeros(0, dtype=np.int64)
            self._seg = ((np.vstack(sa), np.vstack(sb), np.concatenate(sid)) if sa else
                         (np.zeros((0, 3)), np.zeros((0, 3)), np.zeros(0, dtype=np.int64)))
        return self._loops

    def loop_near(self, p):
        """Loop whose polyline passes closest to p (e.g. the midpoint of a picked feature segment)."""
        self.loops()
        if not len(self._loop_pts):
            return None
        d = np.einsum("ij,ij->i", self._loop_pts - p, self._loop_pts - p)
        return self._loops[int(self._loop_ids[int(np.argmin(d))])]

    def loop_at(self, p, tol):
        """Loop with a segment within tol of p (the cursor is on a sharp edge), else None."""
        self.loops()
        a, b, ids = self._seg
        if not len(a) or tol <= 0:
            return None
        p = np.asarray(p, dtype=np.float64)
        ab = b - a
        L2 = np.einsum("ij,ij->i", ab, ab)
        t = np.clip(np.einsum("ij,ij->i", p - a, ab) / np.maximum(L2, 1e-300), 0.0, 1.0)
        q = a + ab * t[:, None]
        d = np.einsum("ij,ij->i", q - p, q - p)
        i = int(np.argmin(d))
        return self._loops[int(ids[i])] if d[i] <= tol * tol else None

    def _topology(self):
        """Facet neighbours across manifold edges (self._nbr), open / non-manifold edge counts and the sharp edges
        (node pairs, kept only until the loops are chained)."""
        if self._nbr is not None:
            return
        T, N = self.tris, len(self.nodes)
        M = len(T)
        E = np.concatenate([T[:, [0, 1]], T[:, [1, 2]], T[:, [2, 0]]])      # position = side * M + facet
        E.sort(axis=1)
        key = E[:, 0].astype(np.int64) * N + E[:, 1]
        order = np.argsort(key, kind="stable")
        _, start, counts = np.unique(key[order], return_index=True, return_counts=True)
        del key
        self.boundary_edges = int((counts == 1).sum())
        self.nonmanifold_edges = int((counts > 2).sum())
        two = counts == 2
        p1, p2 = order[start[two]], order[start[two] + 1]
        f1, f2 = p1 % M, p2 % M
        sharp = ~two                                       # open boundary / non-manifold edges are sharp too
        sharp[two] = np.einsum("ij,ij->i", self.normals[f1], self.normals[f2]) < math.cos(math.radians(FEATURE_DEG))
        self._sharp = E[order[start[sharp]]]
        nbr = np.full((M, 3), -1, dtype=np.int32)          # facet neighbours across its 3 manifold edges
        nbr[f1, p1 // M] = f2
        nbr[f2, p2 // M] = f1
        self._nbr = nbr

    # ---------------------------------------------------------- flat patches (planar faces of the model)
    def flat_patch(self, t):
        """(facet indices, area, is_face, centroid) of the flat patch containing facet t. is_face = the patch is bounded
        mostly by sharp edges, i.e. a real planar face, not one facet strip of a tessellated curved surface."""
        if self._region is None:
            self._region = np.full(len(self.tris), -1, dtype=np.int32)
        rid = int(self._region[t])
        if rid >= 0:
            return self._regions[rid]
        self._topology()
        nbr, N = self._nbr, self.normals
        if self._dtol is None:                             # STL floats: normals of small facets are rough
            self._dtol = max(1e-3, 1e-4 * float(np.ptp(self.nodes)))
        n0 = N[t]
        d0 = float(n0 @ self.centroids[t])
        cmin = math.cos(math.radians(FLAT_DEG))
        inside = np.zeros(len(self.tris), dtype=bool)
        inside[t] = True
        front = np.array([t], dtype=np.int32)
        parts = [front]
        while len(front):                                  # breadth-first flood over coplanar neighbours
            nb = nbr[front].ravel()
            nb = nb[nb >= 0]
            nb = np.unique(nb[~inside[nb]])
            nb = nb[(N[nb] @ n0 >= cmin) & (np.abs(self.centroids[nb] @ n0 - d0) <= self._dtol)]
            inside[nb] = True
            parts.append(nb)
            front = nb
        facets = np.sort(np.concatenate(parts))
        v0, v1, v2 = (self.nodes[self.tris[facets, i]] for i in range(3))
        a = 0.5 * np.linalg.norm(np.cross(v1 - v0, v2 - v0), axis=1)
        area = float(a.sum())
        centroid = (self.centroids[facets] * a[:, None]).sum(axis=0) / max(area, 1e-300)
        # facet sides on the patch border, and which of them are sharp edges / open boundary
        g = nbr[facets]
        g0 = np.maximum(g, 0)
        border = ~np.where(g >= 0, inside[g0], False)
        sharp = (g < 0) | (np.einsum("ij,ikj->ik", N[facets], N[g0]) < math.cos(math.radians(FEATURE_DEG)))
        nb = int(border.sum())
        is_face = len(facets) >= 2 and nb > 0 and float((border & sharp).sum()) / nb >= 0.6
        self._regions.append((facets, area, is_face, centroid))
        self._region[facets] = len(self._regions) - 1
        return self._regions[-1]

    def _build_loops(self):
        self._topology()
        fe, self._sharp = self._sharp, None
        if fe is None or len(fe) == 0 or len(fe) > MAX_FEATURE_EDGES:
            return []
        # chain the feature edges into polylines
        adj = {}
        for a, b in fe.tolist():
            adj.setdefault(a, []).append(b)
            adj.setdefault(b, []).append(a)
        used = set()
        loops = []
        for a, b in fe.tolist():
            if (a, b) in used or (b, a) in used:
                continue
            used.add((a, b))
            chain = [a, b]
            closed = False
            for direction in (0, 1):                      # extend forward, then backward from a
                if direction == 1:
                    chain.reverse()
                while True:
                    cur, prev = chain[-1], chain[-2]
                    nb = adj.get(cur, ())
                    if len(nb) != 2:                       # junction or end
                        break
                    nxt = nb[0] if nb[1] == prev else nb[1]
                    if (cur, nxt) in used or (nxt, cur) in used:
                        if nxt == chain[0]:
                            closed = True
                        break
                    used.add((cur, nxt))
                    chain.append(nxt)
                    if nxt == chain[0]:
                        closed = True
                        break
                if closed:
                    break
            if closed and chain[-1] == chain[0]:
                chain = chain[:-1]
            if len(chain) >= 2:
                loops.append(Loop(self.nodes[chain], closed))
        return loops

    # ---------------------------------------------------------- info
    def info_rows(self):
        lps = self.loops()
        holes = [lp for lp in lps if lp.circle is not None and lp.kind() == "kružnice"]
        rows = [("Síť", f"{len(self.tris):,} trojúhelníků, {len(self.nodes):,} vrcholů".replace(",", " ")),
                ("Uzavřená síť", "ano" if self.boundary_edges == 0 and self.nonmanifold_edges == 0 else
                 f"ne ({self.boundary_edges} otevřených hran" +
                 (f", {self.nonmanifold_edges} nemanifoldních" if self.nonmanifold_edges else "") + ")"),
                ("Ostré hrany", f"{len(lps)} obrysů, {len(holes)} kružnic (otvory / čepy)")]
        if holes:
            ds = sorted({round(2 * lp.circle[2], 2) for lp in holes})
            rows.append(("Průměry kružnic", ", ".join(_p(d, 2) for d in ds[:12]) + (" …" if len(ds) > 12 else "")))
        return rows


def _closest_on_triangle(p, a, b, c):
    """Closest point to p on triangle abc (Ericson, Real-Time Collision Detection 5.1.5)."""
    ab, ac, ap = b - a, c - a, p - a
    d1, d2 = ab @ ap, ac @ ap
    if d1 <= 0 and d2 <= 0:
        return a
    bp = p - b
    d3, d4 = ab @ bp, ac @ bp
    if d3 >= 0 and d4 <= d3:
        return b
    vc = d1 * d4 - d3 * d2
    if vc <= 0 and d1 >= 0 and d3 <= 0:
        return a + ab * (d1 / (d1 - d3))
    cp = p - c
    d5, d6 = ab @ cp, ac @ cp
    if d6 >= 0 and d5 <= d6:
        return c
    vb = d5 * d2 - d1 * d6
    if vb <= 0 and d2 >= 0 and d6 <= 0:
        return a + ac * (d2 / (d2 - d6))
    va = d3 * d6 - d5 * d4
    if va <= 0 and (d4 - d3) >= 0 and (d5 - d6) >= 0:
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)))
    den = 1.0 / (va + vb + vc)
    return a + ab * (vb * den) + ac * (vc * den)


def mesh_for_file(path, max_tris=MAX_TRIS):
    """Mesh of an STL file for measuring, None if unreadable / too big. Not cached: a module-level cache kept
    hundreds of MB alive after the STL was closed, and re-parsing takes ~1 s per million facets."""
    try:
        return Mesh.from_stl(path, max_tris)
    except (OSError, ValueError, MemoryError):
        return None
