"""Thumbnail renderer for the Explorer thumbnail handler.

    StepViewer.exe --thumbnail <input file> <output.png> <size>

3D (STEP/IGES/STL): off-screen OCCT render on black and white backgrounds, the difference gives
a clean alpha channel (anti-aliased edges included). 2D (DXF/DWG): ezdxf render on white paper.
Never shows any window; exit code 0 = PNG written.
"""
import ctypes
import os
import shutil
import sys
import tempfile


def _qt_app():
    from PySide6.QtWidgets import QApplication
    return QApplication.instance() or QApplication(["thumb"])


def _matte(path_black, path_white, out, size):
    """alpha = 1 - (white - black); colour = black / alpha (premultiplied -> straight)."""
    import numpy as np
    from PySide6.QtGui import QImage
    ib = QImage(path_black).convertToFormat(QImage.Format_RGBA8888)
    iw = QImage(path_white).convertToFormat(QImage.Format_RGBA8888)
    w, h = ib.width(), ib.height()
    b = np.frombuffer(ib.constBits(), np.uint8).reshape(h, ib.bytesPerLine())[:, :w * 4].reshape(h, w, 4)
    wt = np.frombuffer(iw.constBits(), np.uint8).reshape(h, iw.bytesPerLine())[:, :w * 4].reshape(h, w, 4)
    b = b[..., :3].astype(np.float32) / 255.0
    wt = wt[..., :3].astype(np.float32) / 255.0
    alpha = np.clip(1.0 - (wt - b).mean(axis=2), 0.0, 1.0)
    col = np.where(alpha[..., None] > 1e-3, b / np.maximum(alpha[..., None], 1e-3), 0.0)
    rgba = np.dstack([np.clip(col, 0, 1) * 255, alpha * 255]).astype(np.uint8)
    rgba = np.ascontiguousarray(rgba)
    img = QImage(rgba.data, w, h, w * 4, QImage.Format_RGBA8888).copy()
    # crop to content and centre into a square of the requested size
    ys, xs = np.nonzero(alpha > 0.02)
    if len(xs):
        from PySide6.QtCore import QRect, Qt
        img = img.copy(QRect(int(xs.min()), int(ys.min()), int(xs.max() - xs.min() + 1), int(ys.max() - ys.min() + 1)))
        img = img.scaled(int(size * 0.94), int(size * 0.94), Qt.KeepAspectRatio, Qt.SmoothTransformation)
        canvas = QImage(size, size, QImage.Format_RGBA8888)
        canvas.fill(0)
        from PySide6.QtGui import QPainter
        p = QPainter(canvas)
        p.drawImage((size - img.width()) // 2, (size - img.height()) // 2, img)
        p.end()
        img = canvas
    return img.save(out)


def render_3d(path, out, size):
    import core
    from OCP.AIS import AIS_InteractiveContext, AIS_Shaded
    from OCP.Aspect import Aspect_DisplayConnection, Aspect_TOL_SOLID
    from OCP.Graphic3d import Graphic3d_BufferType
    from OCP.Image import Image_AlienPixMap
    from OCP.OpenGl import OpenGl_GraphicDriver
    from OCP.Prs3d import Prs3d_LineAspect
    from OCP.TCollection import TCollection_AsciiString
    from OCP.V3d import V3d_TypeOfOrientation_Zup_AxoRight, V3d_Viewer
    from OCP.WNT import WNT_Window
    from viewer3d import _capsule, leaf_objects, rgb

    doc = core.read_cad(path)
    roots, nodes, proto_count = core.build_tree(doc)
    _qt_app()

    user32 = ctypes.windll.user32
    user32.CreateWindowExW.restype = ctypes.c_void_p
    user32.CreateWindowExW.argtypes = [ctypes.c_uint32, ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_uint32,
                                       ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                                       ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p]
    hwnd = user32.CreateWindowExW(0, "STATIC", "thumb", 0x80000000, 0, 0, size, size, None, None, None, None)
    driver = OpenGl_GraphicDriver(Aspect_DisplayConnection(), False)    # context is created below, after the caps
    driver.ChangeOptions().swapInterval = 0         # off-screen render: don't wait for vsync (-0.6 s per process)
    if not driver.InitContext():                    # (the constructor used to raise: run() then falls back to software)
        raise RuntimeError("OpenGL context")
    viewer = V3d_Viewer(driver)
    viewer.SetDefaultLights()
    viewer.SetLightOn()
    ctx = AIS_InteractiveContext(viewer)
    ctx.DefaultDrawer().SetAutoTriangulation(False)
    view = viewer.CreateView()
    win = WNT_Window(_capsule(hwnd))
    win.SetVirtual(True)
    view.SetWindow(win)
    view.ChangeRenderingParams().NbMsaaSamples = 8
    edge = Prs3d_LineAspect(rgb(40, 40, 46), Aspect_TOL_SOLID, 1.0)

    def style(o):
        o.SetColor(rgb(200, 204, 210))
        dr = o.Attributes()
        dr.SetFaceBoundaryDraw(True)
        dr.SetFaceBoundaryAspect(edge)

    objs, _protos = leaf_objects([n for n in nodes if not n.is_asm], proto_count, style)
    for o in objs:
        ctx.Display(o, AIS_Shaded, 0, False)
    view.SetProj(V3d_TypeOfOrientation_Zup_AxoRight)
    view.FitAll(0.01, False)
    view.ZFitAll()
    rs = min(max(size, 64), 1024) * 2                        # render 2x, downscale = extra AA
    tmp = tempfile.mkdtemp(prefix="stepthumb_")
    paths = []
    for name, c in (("b.png", rgb(0, 0, 0)), ("w.png", rgb(255, 255, 255))):
        view.SetBackgroundColor(c)
        pm = Image_AlienPixMap()
        if not view.ToPixMap(pm, rs, rs, Graphic3d_BufferType.Graphic3d_BT_RGB):
            raise RuntimeError("render failed")
        p = os.path.join(tmp, name)
        pm.Save(TCollection_AsciiString(p))
        paths.append(p)
    ok = _matte(paths[0], paths[1], out, size)
    for p in paths:
        os.remove(p)
    os.rmdir(tmp)
    user32.DestroyWindow(ctypes.c_void_p(hwnd))
    return ok


def _lin2srgb(v):
    v = max(0.0, min(1.0, v))
    return 12.92 * v if v <= 0.0031308 else 1.055 * v ** (1 / 2.4) - 0.055


def _collect_triangles(doc):
    """All mesh triangles in world coordinates: (tris (N,3,3), colours (N,3) sRGB 0..1, face ids (N,))."""
    import numpy as np
    import core
    from OCP.BRep import BRep_Tool
    from OCP.TopAbs import TopAbs_FACE
    from OCP.TopExp import TopExp_Explorer
    from OCP.TopLoc import TopLoc_Location
    from OCP.TopoDS import TopoDS
    from OCP.XCAFDoc import XCAFDoc_ShapeTool

    _, nodes, _ = core.build_tree(doc)
    tris, cols, fids = [], [], []
    fid = 0
    for n in nodes:
        if n.is_asm:
            continue
        shape = XCAFDoc_ShapeTool.GetShape_s(n.label)
        if not n.parent_loc.IsIdentity():
            shape = shape.Moved(n.parent_loc)
        if n.color is not None:
            base = (_lin2srgb(n.color.Red()), _lin2srgb(n.color.Green()), _lin2srgb(n.color.Blue()))
        else:
            base = (0.78, 0.80, 0.82)
        e = TopExp_Explorer(shape, TopAbs_FACE)
        while e.More():
            loc = TopLoc_Location()
            tri = BRep_Tool.Triangulation_s(TopoDS.Face(e.Current()), loc)
            e.Next()
            if tri is None or tri.NbTriangles() == 0:
                continue
            trsf = loc.Transformation()
            ident = loc.IsIdentity()
            pts = np.empty((tri.NbNodes(), 3))
            for i in range(tri.NbNodes()):
                p = tri.Node(i + 1)
                if not ident:
                    p = p.Transformed(trsf)
                pts[i] = (p.X(), p.Y(), p.Z())
            idx = np.array([tri.Triangle(i + 1).Get() for i in range(tri.NbTriangles())], dtype=np.int64) - 1
            tris.append(pts[idx])
            cols.append(np.tile(base, (len(idx), 1)))
            fids.append(np.full(len(idx), fid))
            fid += 1
    if not tris:
        raise RuntimeError("model has no mesh")
    return np.concatenate(tris), np.concatenate(cols), np.concatenate(fids)


def render_3d_soft(doc, out, size):
    """GPU-less renderer (Linux thumbnail sandboxes, fallback on Windows): stratified point sampling of
    the mesh into a z-buffer at 2x resolution, Lambert shading, face-boundary outlines, 2x2 downsample."""
    import numpy as np
    from PIL import Image

    T, C, F = _collect_triangles(doc)
    S = int(size) * 2
    back = np.array([1.0, -1.0, 1.0]) / np.sqrt(3)            # isometric view as in the viewer (Zup_AxoRight)
    right = np.array([1.0, 1.0, 0.0]) / np.sqrt(2)
    up = np.cross(back, right)
    X, Y, Z = T @ right, T @ up, T @ back                       # (N,3) each
    x0, x1, y0, y1 = X.min(), X.max(), Y.min(), Y.max()
    k = (S * 0.94) / max(x1 - x0, y1 - y0, 1e-9)
    ox = (S - (x1 - x0) * k) / 2
    oy = (S - (y1 - y0) * k) / 2
    sx = (X - x0) * k + ox
    sy = (y1 - Y) * k + oy
    # shading (two-sided Lambert, light from upper left of the viewer)
    nrm = np.cross(T[:, 1] - T[:, 0], T[:, 2] - T[:, 0])
    nl = np.linalg.norm(nrm, axis=1)
    nrm /= np.maximum(nl, 1e-12)[:, None]
    light = back * 0.75 + up * 0.55 - right * 0.35
    light /= np.linalg.norm(light)
    lam = np.abs(nrm @ light)
    shade = (0.30 + 0.70 * lam)[:, None]
    col = np.clip(C * shade + 0.08 * lam[:, None] ** 8, 0, 1)
    # samples per triangle ~ 3 per pixel of projected area
    area = 0.5 * np.abs((sx[:, 1] - sx[:, 0]) * (sy[:, 2] - sy[:, 0]) - (sx[:, 2] - sx[:, 0]) * (sy[:, 1] - sy[:, 0]))
    cnt = np.clip(np.ceil(area * 3.0), 1, 200000).astype(np.int64)
    if cnt.sum() > 12_000_000:                                   # safety for gigantic meshes
        cnt = np.maximum(1, (cnt * (12_000_000 / cnt.sum())).astype(np.int64))
    ti = np.repeat(np.arange(len(T)), cnt)
    j = np.arange(len(ti)) - np.repeat(np.cumsum(cnt) - cnt, cnt)   # sample index within its triangle
    a = (j * 0.7548776662466927 + 0.5) % 1.0                     # R2 low-discrepancy sequence
    b = (j * 0.5698402909980532 + 0.5) % 1.0
    single = cnt[ti] == 1
    a[single], b[single] = 1 / 3, 1 / 3
    flip = a + b > 1
    a[flip], b[flip] = 1 - a[flip], 1 - b[flip]
    px = sx[ti, 0] + a * (sx[ti, 1] - sx[ti, 0]) + b * (sx[ti, 2] - sx[ti, 0])
    py = sy[ti, 0] + a * (sy[ti, 1] - sy[ti, 0]) + b * (sy[ti, 2] - sy[ti, 0])
    pz = Z[ti, 0] + a * (Z[ti, 1] - Z[ti, 0]) + b * (Z[ti, 2] - Z[ti, 0])
    ix = np.clip(px.astype(np.int64), 0, S - 1)
    iy = np.clip(py.astype(np.int64), 0, S - 1)
    pix = iy * S + ix
    order = np.lexsort((pz, pix))                                # nearest (max z) last per pixel
    ps = pix[order]
    last = np.r_[ps[1:] != ps[:-1], True]
    win = order[last]
    fidimg = np.full(S * S, -1, np.int64)
    rgb = np.zeros((S * S, 3))
    fidimg[pix[win]] = F[ti[win]]
    rgb[pix[win]] = col[ti[win]]
    fidimg = fidimg.reshape(S, S)
    rgb = rgb.reshape(S, S, 3)
    alpha = (fidimg >= 0).astype(np.float64)
    # fill single-pixel sampling holes from neighbours
    hole = (alpha == 0)
    nb = np.zeros((S, S))
    acc = np.zeros((S, S, 3))
    for dy, dx in ((0, 1), (0, -1), (1, 0), (-1, 0)):
        sh_a = np.roll(alpha, (dy, dx), (0, 1))
        nb += sh_a
        acc += np.roll(rgb, (dy, dx), (0, 1)) * sh_a[..., None]
    fill = hole & (nb >= 3)
    rgb[fill] = acc[fill] / nb[fill][:, None]
    alpha[fill] = 1.0
    fid_f = fidimg.copy()
    fid_f[fill] = np.roll(fidimg, (0, 1), (0, 1))[fill]
    # face boundaries + silhouette
    edge = np.zeros((S, S), bool)
    for dy, dx in ((0, 1), (1, 0)):
        other = np.roll(fid_f, (dy, dx), (0, 1))
        edge |= (other != fid_f) & ((other >= 0) | (fid_f >= 0))
    edge &= alpha > 0
    rgb[edge] *= 0.25
    # 2x2 downsample with premultiplied alpha
    pre = rgb * alpha[..., None]
    pre = pre.reshape(size, 2, size, 2, 3).mean(axis=(1, 3))
    al = alpha.reshape(size, 2, size, 2).mean(axis=(1, 3))
    outc = np.where(al[..., None] > 0, pre / np.maximum(al, 1e-6)[..., None], 0)
    rgba = np.dstack([np.clip(outc, 0, 1) * 255, al * 255]).astype(np.uint8)
    Image.fromarray(rgba, "RGBA").save(out)
    return True


def render_2d(path, out, size):
    import view2d
    _qt_app()
    d = None
    try:
        dxf = None
        if path.lower().endswith(".dwg"):
            # private directory instead of mkstemp + remove + reuse of the name (a race on the file name)
            d = tempfile.mkdtemp(prefix="stepthumb_")
            dxf = os.path.join(d, "o.dxf")
        doc, _ = view2d.load_drawing(path, dxf)
        return view2d.render_thumbnail(doc, size, out)
    finally:
        if d:
            shutil.rmtree(d, ignore_errors=True)


def _headless():
    """Thumbnail requests may come from contexts without a desktop (Windows Search indexer):
    no Qt windows, no system error dialogs, no Qt message boxes – just fail quietly."""
    os.environ["QT_QPA_PLATFORM"] = "offscreen"
    try:
        # SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX
        ctypes.windll.kernel32.SetErrorMode(0x0001 | 0x0002 | 0x8000)
    except Exception:  # noqa: BLE001
        pass
    try:
        from PySide6.QtCore import qInstallMessageHandler
        qInstallMessageHandler(lambda *args: None)
    except Exception:  # noqa: BLE001
        pass


MAX_INPUT = 80 * 1024 * 1024       # bigger inputs get no thumbnail (same limit as the Explorer handler)


_EXTERNAL_KEYWORDS = (b"DOCUMENT_FILE", b"EXTERNAL_SOURCE", b"EXTERNALLY_DEFINED")
_PIECE = 8 << 20


def _has_external_keyword(path):
    """True when the file contains one of the external-document keywords (case-insensitive, ASCII). Cheap pre-filter:
    a case-folded copy of each piece and plain substring searches instead of a regular expression over every byte."""
    tail = b""
    with open(path, "rb") as f:
        while True:
            data = f.read(_PIECE)
            if not data:
                return False
            raw = tail + data
            buf = raw.upper()
            if any(k in buf for k in _EXTERNAL_KEYWORDS):
                return True
            tail = raw[-32:]               # longest keyword has 18 bytes: one may straddle two pieces


def remote_step_refs(path):
    """True when a STEP file references external documents on a network path / URL. Open CASCADE tries to
    read external references, and touching \\\\server\\share from an automatic thumbnail would make Windows
    send the user's NTLM credentials to that server. The file is scanned in 8 MB pieces, never read whole."""
    import re
    # a quote followed by two backslashes, // or scheme://, or the STEP escapes that spell a backslash / slash
    # (\X\5C, \X\2F, \X2\...005C...\X0\ - OCCT decodes them in file names)
    net = re.compile(rb"'(?:\\\\|//|[A-Za-z][A-Za-z0-9+.-]*://)|\\X\\(?:5C|2F)|\\X[24]\\(?:[0-9A-F]{4})*?(?:005C|002F)",
                     re.IGNORECASE)

    def found(rx):
        tail = b""
        with open(path, "rb") as f:
            while True:
                data = f.read(_PIECE)
                if not data:
                    return False
                buf = tail + data
                if rx.search(buf):
                    return True
                tail = buf[-256:]           # a match can straddle two pieces

    # first pass: external-document keywords (absent from ordinary files - then that's all); only if present, the
    # second pass looks for a network path / URL / escaped slash anywhere in the file
    return _has_external_keyword(path) and found(net)


def run(src, out, size):
    _headless()
    size = max(16, min(int(size), 1024))
    ext = os.path.splitext(src)[1].lower()
    try:
        if os.path.getsize(src) > MAX_INPUT:
            return 1
    except OSError:
        return 1
    if ext in (".step", ".stp") and remote_step_refs(src):
        return 1
    try:
        if ext in (".dxf", ".dwg"):
            ok = render_2d(src, out, size)
        elif sys.platform == "win32" and os.environ.get("STEPVIEW_SOFT_THUMBS") is None:
            try:
                ok = render_3d(src, out, size)              # OpenGL (better shading)
            except Exception:  # noqa: BLE001 – no GPU context (e.g. indexer) -> software
                import core
                ok = render_3d_soft(core.read_cad(src), out, size)
        else:
            import core
            ok = render_3d_soft(core.read_cad(src), out, size)
        return 0 if ok and os.path.exists(out) else 1
    except Exception as ex:  # noqa: BLE001
        if sys.stderr is not None:
            sys.stderr.write(f"thumbnail failed: {ex}\n")
        return 1
