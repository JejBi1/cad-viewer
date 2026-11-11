"""CAD prohlížeč – rychlá kontrola STEP / IGES / STL modelů (3D) a DXF / DWG výkresů (2D)."""
import collections
import hashlib
import html
import json
import math
import os
import sys
import time
import traceback

# numpy (ezdxf drawing, thumbnails) would commit ~0.5 GB of OpenBLAS per-thread buffers; we never need BLAS
os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")

APP_NAME = "CAD prohlížeč"
VERSION = "1.9"
CACHE_MIN = 1 * 1024 * 1024           # files from this size get a cached (pre-meshed) copy
# bigger files load in a worker process (GUI stays responsive: OCCT holds the GIL while it reads, so loading in
# this process freezes the window even from a thread). Same limit as the cache; smaller files load in-process.
WORKER_THRESHOLD = int(os.environ.get("STEPVIEW_WORKER_MIN", CACHE_MIN))
CACHE_LIMIT = 3 * 1024 ** 3           # total cache size kept on disk
if sys.platform == "win32":
    CACHE_DIR = os.path.join(os.environ.get("LOCALAPPDATA", os.path.expanduser("~")), "StepViewer", "cache")
else:
    CACHE_DIR = os.path.join(os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache"), "cad-viewer")


# ====================================================================== worker
def run_worker(src, dst):
    log = dst + ".log"

    def report(msg):
        with open(log, "a", encoding="utf-8") as f:
            f.write(msg + "\n")

    try:
        import core
        doc = core.read_cad(src, report)
        report("Předávám model…")
        core.save_xbf(doc, dst)
        report("DONE")
        return 0
    except Exception as ex:  # noqa: BLE001
        report("ERROR " + (str(ex) or ex.__class__.__name__))
        return 1


WORKER_IDLE_S = 120                   # a pre-started worker waits this long for its job, then quits


def job_valid(src, dst):
    """A job is data read from a file: accept only an absolute source path and a ".xbf" destination inside the
    cache directory (the worker writes there, and next to it the "<dst>.log" progress file)."""
    try:
        if not (isinstance(src, str) and isinstance(dst, str) and src and dst):
            return False
        if "\0" in src or "\0" in dst or not os.path.isabs(src) or not os.path.isabs(dst):
            return False
        if not dst.lower().endswith(".xbf"):
            return False
        root = os.path.normcase(os.path.realpath(CACHE_DIR))
        d = os.path.normcase(os.path.realpath(dst))
        return d != root and os.path.commonpath([root, d]) == root
    except (ValueError, OSError):
        return False


def serve_worker(job):
    """Pre-started worker: does the slow OCCT import first, then waits for the job file (JSON [src, dst] – file
    names may contain any character, incl. newlines and U+2028 / U+2029 / U+0085 that str.splitlines() splits on)."""
    import core  # noqa: F401
    end = time.time() + WORKER_IDLE_S
    while time.time() < end:
        try:
            with open(job, encoding="utf-8") as f:
                raw = f.read()
        except OSError:
            time.sleep(0.05)
            continue
        _remove(job)                       # written atomically (os.replace): what we read is complete
        try:
            data = json.loads(raw)
            ok = isinstance(data, list) and len(data) == 2 and job_valid(data[0], data[1])
        except ValueError:
            ok = False
        if not ok:
            return 1
        return run_worker(data[0], data[1])
    return 0


def _remove(p):
    try:
        if p and os.path.exists(p):
            os.remove(p)
    except OSError:
        pass


def ensure_cache_dir():
    """Create the cache directory; on Linux private to the user (0700) - it holds job files and converted models."""
    if sys.platform == "win32":
        os.makedirs(CACHE_DIR, exist_ok=True)           # %LOCALAPPDATA% is per-user already
        return
    os.makedirs(CACHE_DIR, mode=0o700, exist_ok=True)    # mode applies to the leaf only, not to missing parents
    try:
        os.chmod(CACHE_DIR, 0o700)
    except OSError:
        pass


def cache_path(path):
    st = os.stat(path)
    key = f"{os.path.normcase(os.path.abspath(path))}|{st.st_size}|{st.st_mtime_ns}|{VERSION}"
    return os.path.join(CACHE_DIR, hashlib.sha1(key.encode("utf-8")).hexdigest() + ".xbf")


def part_path(cache):
    return cache[:-4] + ".tmp.xbf"   # OCCT appends .xbf when missing


_jobs = [0]


def spawn_worker():
    """Start an idle worker process; returns [Popen, job file, start time, submitted source or None]."""
    import subprocess
    _jobs[0] += 1
    ensure_cache_dir()
    job = os.path.join(CACHE_DIR, f"job_{os.getpid()}_{_jobs[0]}.tmp")
    _remove(job)
    cmd = [sys.executable] + ([] if getattr(sys, "frozen", False) else [os.path.abspath(__file__)])
    p = subprocess.Popen(cmd + ["--worker-serve", job], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL,
                         creationflags=subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0)
    return [p, job, time.time(), None]


def submit_job(job, src, dst):
    with open(job + ".w", "w", encoding="utf-8") as f:
        f.write(json.dumps([src, dst]))
    os.replace(job + ".w", job)          # atomic: the worker never sees half a job


if __name__ == "__main__" and len(sys.argv) >= 4 and sys.argv[1] == "--worker":
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    sys.exit(run_worker(sys.argv[2], sys.argv[3]))

if __name__ == "__main__" and len(sys.argv) >= 3 and sys.argv[1] == "--worker-serve":
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    sys.exit(serve_worker(sys.argv[2]))

def limit_thumbnail_resources(address_space=4 * 1024 ** 3, cpu_seconds=120):
    """Linux: the thumbnail renderer parses untrusted files automatically (file manager, indexers), so bound its
    memory (address space; generous - OCCT, Qt and numpy map a lot) and CPU time even when it is started by
    something other than our .thumbnailer (which also wraps it in timeout / prlimit). Limits are only lowered."""
    try:
        import resource
    except ImportError:
        return
    for res, want, hard_want in ((resource.RLIMIT_AS, address_space, address_space),
                                 (resource.RLIMIT_CPU, cpu_seconds, cpu_seconds + 5)):   # SIGXCPU, then SIGKILL
        try:
            soft, hard = resource.getrlimit(res)
            if soft != resource.RLIM_INFINITY and soft <= want:
                continue
            resource.setrlimit(res, (want, hard_want if hard == resource.RLIM_INFINITY else min(hard, hard_want)))
        except (ValueError, OSError):
            pass


if __name__ == "__main__" and len(sys.argv) >= 5 and sys.argv[1] == "--thumbnail":
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    if sys.platform.startswith("linux"):
        limit_thumbnail_resources()
    try:                      # never show any dialog from a thumbnail render
        import thumbs
        code = thumbs.run(sys.argv[2], sys.argv[3], sys.argv[4])
    except BaseException:  # noqa: BLE001
        code = 1
    os._exit(code)

_ezdxf_warm = []


def prewarm_ezdxf():
    """Import ezdxf and the drawing frontend (1.5 - 5 s, ~50 MB, mostly numpy) in a background thread, so they are ready
    when the drawing loader / renderer needs them. Started only when a drawing is about to be (or, in the open dialog,
    likely to be) opened - 3D-only sessions never load ezdxf. The thread only imports modules: it creates no Qt object
    (the ezdxf Qt backend defines classes and constants at import time, nothing that needs a QApplication). Imports are
    serialised by Python's import locks, so a concurrent real import just waits."""
    if _ezdxf_warm:
        return
    import threading

    def work():
        try:
            import ezdxf  # noqa: F401
            from ezdxf import path, recover  # noqa: F401
            from ezdxf.addons.drawing import Frontend, RenderContext  # noqa: F401
            from ezdxf.addons.drawing.config import BackgroundPolicy, Configuration, ImagePolicy  # noqa: F401
            from ezdxf.addons.drawing.pyqt import CorrespondingDXFEntity, PyQtBackend  # noqa: F401
            from ezdxf.entities import Face3d, Insert, Viewport  # noqa: F401
        except Exception:  # noqa: BLE001 – it is only a head start; the real import reports any problem
            pass

    t = threading.Thread(target=work, name="ezdxf-prewarm", daemon=True)
    _ezdxf_warm.append(t)
    t.start()


def wait_ezdxf_warm():
    """The first real use of ezdxf (the drawing loader thread) waits for a running prewarm to finish. ezdxf's package
    imports are circular: two threads entering them at different points can end in "cannot import name ... from
    partially initialized module" (reproduced), so the two must not overlap. The wait is never longer than the import
    the loader would have done itself, and it happens off the GUI thread."""
    for t in _ezdxf_warm:
        t.join()


# Opened with a big 3D file (double click in Explorer): start the load job right now, so the worker's OCCT
# import and the file reading run while this process is still importing Qt / OCCT and building the window.
EARLY_WORKER = None
EARLY_DRAWING = False
if __name__ == "__main__" and len(sys.argv) >= 2 and not sys.argv[1].startswith("--"):
    try:
        _p = os.path.abspath(sys.argv[1])
        if (os.path.splitext(_p)[1].lower() in (".step", ".stp", ".iges", ".igs", ".stl")
                and os.path.getsize(_p) >= WORKER_THRESHOLD and not os.path.exists(cache_path(_p))):
            EARLY_WORKER = spawn_worker()
            _remove(part_path(cache_path(_p)) + ".log")
            submit_job(EARLY_WORKER[1], _p, part_path(cache_path(_p)))
            EARLY_WORKER[3] = _p
        elif os.path.splitext(_p)[1].lower() in (".dxf", ".dwg"):
            EARLY_DRAWING = True               # ezdxf is imported in the background once Qt is imported (below)
    except OSError:
        pass


# ====================================================================== GUI
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from PySide6.QtCore import QSettings, QSize, Qt, QThread, QTimer, Signal  # noqa: E402
from PySide6.QtGui import QAction, QColor, QIcon, QKeySequence, QPixmap, QGuiApplication  # noqa: E402
from PySide6.QtWidgets import (QApplication, QCheckBox, QComboBox, QFileDialog, QHBoxLayout,  # noqa: E402
                               QHeaderView, QLabel, QLineEdit, QMainWindow, QMenu, QMessageBox,
                               QProgressBar, QPushButton, QSlider, QSplitter, QStackedWidget, QStyle,
                               QTextBrowser,
                               QToolBar, QToolButton, QTreeWidget, QTreeWidgetItem,
                               QTreeWidgetItemIterator, QVBoxLayout, QWidget)

import materials  # noqa: E402
import view2d  # noqa: E402
from view2d import DrawingView  # noqa: E402

if EARLY_DRAWING:              # opened with a drawing: numpy + ezdxf import overlaps with QApplication / window construction
    prewarm_ezdxf()            # (after the Qt imports above: the thread never imports Qt before the main thread has)

# The 3D stack (OCP / Open CASCADE, ~100 MB of DLLs, 1.1 s import) and the OpenGL context (~0.3-0.5 GB of
# driver memory) are loaded only when the first 3D model is opened - drawings and the empty window don't pay.
core = analysis = None
OccView = leaf_objects = rgb = standalone_object = None
AIS_Shaded = AIS_WireFrame = AIS_ConnectedInteractive = Bnd_Box = BRepBndLib = XCAFDoc_ShapeTool = None


def load_occ():
    global core, analysis, OccView, leaf_objects, rgb, standalone_object
    global AIS_Shaded, AIS_WireFrame, AIS_ConnectedInteractive, Bnd_Box, BRepBndLib, XCAFDoc_ShapeTool
    if core is not None:
        return
    import analysis as _analysis
    import core as _core
    import viewer3d as _v3
    from OCP.AIS import AIS_ConnectedInteractive as _conn, AIS_Shaded as _sh, AIS_WireFrame as _wf
    from OCP.Bnd import Bnd_Box as _bb
    from OCP.BRepBndLib import BRepBndLib as _bbl
    from OCP.XCAFDoc import XCAFDoc_ShapeTool as _st
    core, analysis = _core, _analysis
    OccView, leaf_objects, rgb, standalone_object = _v3.OccView, _v3.leaf_objects, _v3.rgb, _v3.standalone_object
    AIS_Shaded, AIS_WireFrame, AIS_ConnectedInteractive = _sh, _wf, _conn
    Bnd_Box, BRepBndLib, XCAFDoc_ShapeTool = _bb, _bbl, _st


DEFAULT_COLOR = (200, 204, 210)
CAD_EXT = (".step", ".stp", ".iges", ".igs", ".stl")       # = core.CAD_EXT (core is imported lazily)
ALL_EXT = CAD_EXT + view2d.DRAW_EXT


def _msg(icon, parent, title, text):
    """Message box that never interprets the text as HTML (error texts may quote file contents)."""
    b = QMessageBox(icon, title, str(text), QMessageBox.Ok, parent)
    b.setTextFormat(Qt.PlainText)
    return b.exec()


def msg_critical(parent, title, text):
    return _msg(QMessageBox.Critical, parent, title, text)


def msg_warning(parent, title, text):
    return _msg(QMessageBox.Warning, parent, title, text)


def lin2srgb(v):
    v = max(0.0, min(1.0, v))
    return 12.92 * v if v <= 0.0031308 else 1.055 * v ** (1 / 2.4) - 0.055


def qcolor(c):
    if c is None:
        return QColor(*DEFAULT_COLOR)
    return QColor.fromRgbF(lin2srgb(c.Red()), lin2srgb(c.Green()), lin2srgb(c.Blue()))


def fmt_len(v):
    return f"{v:,.2f}".replace(",", " ").replace(".", ",")


def fmt_num(v, dec=0):
    s = f"{v:,.{dec}f}".replace(",", " ")
    return s.replace(".", ",")


class Html(str):
    """Property value that is already HTML; plain str values get escaped (names like "Šroub<1>")."""


def esc(s):
    return s if isinstance(s, Html) else html.escape(str(s), quote=False)


def rich_tip(text):
    """Tooltip text that contains file-derived strings (part names, paths). Qt shows a tooltip as HTML whenever
    it merely looks like HTML, so a name such as <img src="//host/x.png"> would make Windows fetch the image from
    the server (NTLM leak). Escape everything and mark the result explicitly as rich text instead."""
    return "<qt>" + html.escape(str(text), quote=False).replace("\n", "<br>") + "</qt>"


def store_cache(doc, cache):
    try:
        ensure_cache_dir()
        core.save_xbf(doc, part_path(cache))
        os.replace(part_path(cache), cache)
        prune_cache()
    except Exception:  # noqa: BLE001 – cache is best effort
        _remove(part_path(cache))


def prune_cache():
    import shutil
    try:
        # leftovers of interrupted loads / crashed workers (*.tmp.xbf, *.tmp.dxf, *.log, job files)
        old = time.time() - 6 * 3600
        for f in os.listdir(CACHE_DIR):
            p = os.path.join(CACHE_DIR, f)
            if (".tmp." in f or f.endswith((".log", ".tmp", ".w"))) and os.path.isfile(p) and os.path.getmtime(p) < old:
                _remove(p)
            elif f.startswith("dwg_") and os.path.isdir(p) and os.path.getmtime(p) < old:
                shutil.rmtree(p, ignore_errors=True)       # private DWG conversion directory of a crashed load
    except OSError:
        pass
    try:
        files = [os.path.join(CACHE_DIR, f) for f in os.listdir(CACHE_DIR) if f.endswith((".xbf", ".dxf")) and ".tmp." not in f]
        files.sort(key=os.path.getmtime, reverse=True)
        total = 0
        for f in files:
            total += os.path.getsize(f)
            if total > CACHE_LIMIT:
                _remove(f)
    except OSError:
        pass


def clear_cache():
    import shutil
    try:
        for f in os.listdir(CACHE_DIR):
            p = os.path.join(CACHE_DIR, f)
            if f.startswith("dwg_") and os.path.isdir(p):
                shutil.rmtree(p, ignore_errors=True)
            else:
                _remove(p)
    except OSError:
        pass


def resource(name):
    base = getattr(sys, "_MEIPASS", os.path.dirname(os.path.abspath(__file__)))
    return os.path.join(base, name)


class DrawingLoader(QThread):
    """DWG conversion + DXF parsing off the GUI thread (pure Python / subprocess, shares the GIL fine)."""
    loaded = Signal(object, object)
    failed = Signal(str)
    progress = Signal(str)

    def __init__(self, path, dxf_cache, parent=None):
        super().__init__(parent)
        self.path, self.dxf_cache = path, dxf_cache

    def run(self):
        try:
            wait_ezdxf_warm()
            doc, info = view2d.load_drawing(self.path, self.dxf_cache, self.progress.emit)
            self.loaded.emit(doc, info)
        except Exception as ex:  # noqa: BLE001
            self.failed.emit(str(ex) or ex.__class__.__name__)


class MainWindow(QMainWindow):
    meshReady = Signal(object, object, object)      # (token, leaf node, meshmeasure.Mesh or None) from a thread

    def __init__(self):
        super().__init__()
        self._mesh_token = None
        self.meshReady.connect(self._mesh_ready)
        self.settings = QSettings("CadProhlizec", "StepViewer")
        self.spacemouse = None                   # 3Dconnexion 3D mouse (Windows raw input), see showEvent
        if sys.platform == "win32":
            try:
                import spacemouse
                self.spacemouse = spacemouse.SpaceMouse(self.settings, self)
                self.spacemouse.motion.connect(self._space_motion)
                self.spacemouse.button.connect(self._space_button)
            except Exception:  # noqa: BLE001 – the program works without it
                self.spacemouse = None
        self.setWindowTitle(APP_NAME)
        ico = resource("stepview.ico" if sys.platform == "win32" else "stepview.png")
        if os.path.exists(ico):
            self.setWindowIcon(QIcon(ico))
        self.setAcceptDrops(True)

        self.doc = None
        self.nodes = []
        self.roots = []
        self.proto_count = {}
        self.from_cache = False
        self.load_time = 0.0
        self.ais_node = {}
        self.protos = {}            # proto key -> shared XCAFPrs presentation (parts used more than once)
        self.leaf_cache = {}
        self.vol_cache = {}         # proto key -> (volume, area, centroid, solids, faces)
        self.mats = None            # materials.Assignments of the open model
        self.collide_dlg = None
        self.model_box = None
        self.path = None
        self._sync = False
        self._proc = None           # running load worker (subprocess.Popen)
        self._job = None
        self._warm = EARLY_WORKER   # pre-started idle worker (Popen, job file, start time)
        self._showing = False
        self._tmp = None
        self._t0 = 0
        self._vis_dirty = False
        self.mode = None            # "3d" / "2d"
        self.ddoc = None            # ezdxf document
        self.d_info = {}
        self.d_layout = "Model"
        self.d_vis = {}             # layer lower-name -> visible
        self.d_loader = None

        # ---------------- central: 3D / 2D + left panel
        self.view = None            # OccView, created by ensure_3d() with the first 3D model
        self.view2d = DrawingView()
        self.view2d.entityPicked.connect(self.on_entity_picked)
        self.view2d.contextMenuAt.connect(self.show_context_menu)
        self.view2d.measured.connect(self.on_measured2d)
        self.view2d.cursorMoved.connect(
            lambda x, y: self.st_coord.setText(f"X {fmt_len(x)}   Y {fmt_len(y)}"))
        self.view2d.angleMeasured.connect(self.on_angle)
        self.panel2d = view2d.DrawingPanel(self.view2d)
        self.empty = QLabel("Otevři model nebo výkres (Ctrl+O), nebo ho sem přetáhni.\n\n"
                            "3D: STEP, IGES, STL   ·   2D: DXF, DWG")
        self.empty.setAlignment(Qt.AlignCenter)
        self.empty.setStyleSheet("color: #8a919c; font-size: 13pt;")
        self.stack = QStackedWidget()
        self.stack.addWidget(self.empty)
        self.stack.addWidget(self.panel2d)
        self._rerender = QTimer(self)
        self._rerender.setSingleShot(True)
        self._rerender.setInterval(150)
        self._rerender.timeout.connect(self.refresh_layers)

        self.filter = QLineEdit()
        self.filter.setPlaceholderText("Hledat ve stromu…")
        self.filter.setClearButtonEnabled(True)
        self.filter.textChanged.connect(self.apply_filter)

        self.tree = QTreeWidget()
        self.tree.setHeaderLabels(["Název", "Info"])
        self.tree.setSelectionMode(QTreeWidget.ExtendedSelection)
        self.tree.header().setStretchLastSection(False)
        self.tree.header().setSectionResizeMode(0, QHeaderView.Stretch)
        self.tree.header().setSectionResizeMode(1, QHeaderView.ResizeToContents)
        self.tree.setUniformRowHeights(True)
        self.tree.setContextMenuPolicy(Qt.CustomContextMenu)
        self.tree.customContextMenuRequested.connect(
            lambda p: self.show_context_menu(self.tree.viewport().mapToGlobal(p)))
        self.tree.itemSelectionChanged.connect(self.on_tree_selection)
        self.tree.itemChanged.connect(self.on_item_changed)
        self.tree.itemDoubleClicked.connect(lambda *_: self.zoom_selected())

        self.props = QTextBrowser()
        self.props.setOpenLinks(False)
        self.props.anchorClicked.connect(self.on_props_link)

        left = QWidget()
        lv = QVBoxLayout(left)
        lv.setContentsMargins(4, 4, 0, 4)
        lv.addWidget(self.filter)
        lsplit = QSplitter(Qt.Vertical)
        lsplit.addWidget(self.tree)
        lsplit.addWidget(self.props)
        lsplit.setStretchFactor(0, 3)
        lsplit.setStretchFactor(1, 1)
        lv.addWidget(lsplit)
        self.lsplit = lsplit

        self.split = QSplitter(Qt.Horizontal)
        self.split.addWidget(left)
        self.split.addWidget(self.stack)
        self.split.setStretchFactor(0, 0)
        self.split.setStretchFactor(1, 1)
        self.split.setSizes([340, 1000])
        self.setCentralWidget(self.split)

        self._build_actions()
        self._build_status()

        g = self.settings.value("geometry")
        if g is not None:
            self.restoreGeometry(g)
        else:
            self.resize(1500, 900)
        s = self.settings.value("split")
        if s is not None:
            self.split.restoreState(s)
        self.show_welcome()

    def ensure_3d(self):
        """Load Open CASCADE and create the OpenGL 3D view on first use."""
        if self.view is not None:
            return self.view
        QApplication.setOverrideCursor(Qt.WaitCursor)
        try:
            load_occ()
            v = OccView()
            v.selectionChanged.connect(self.on_view_selection)
            v.contextMenuAt.connect(self.show_context_menu)
            v.measured.connect(self.on_measured)
            v.entitiesPicked.connect(self.on_entities)
            v.zoomSelected.connect(self.zoom_selected)
            v.angleMeasured.connect(self.on_angle)
            v.hoverText.connect(self.on_hover_text)
            v.face_normal = analysis.face_normal_at
            v.describe = self.describe_item
            v.scale_bar_on = self.chk_scale.isChecked()
            self.view = v
            self.stack.insertWidget(0, v)
            if self.a_persp.isChecked():
                v.set_perspective(True)
        finally:
            QApplication.restoreOverrideCursor()
        return v

    # ------------------------------------------------------------------ UI
    def _act(self, text, slot, shortcut=None, tip=None, checkable=False):
        a = QAction(text, self)
        if shortcut:
            a.setShortcut(QKeySequence(shortcut))
            a.setShortcutContext(Qt.WindowShortcut)
        a.setToolTip(f"{tip or text}" + (f"  [{shortcut}]" if shortcut else ""))
        a.setCheckable(checkable)
        if checkable:
            a.toggled.connect(slot)
        else:
            a.triggered.connect(slot)
        self.addAction(a)
        return a

    def _build_actions(self):
        st = self.style()
        self.a_open = self._act("Otevřít", self.open_dialog, "Ctrl+O", "Otevřít STEP / IGES / STL / DXF / DWG")
        self.a_open.setIcon(st.standardIcon(QStyle.SP_DialogOpenButton))
        self.a_reload = self._act("Znovu načíst", self.reload, "F5")
        self.a_reload.setIcon(st.standardIcon(QStyle.SP_BrowserReload))
        self.a_fit = self._act("Vše", self.fit, "F", "Zobrazit celý model / výkres")
        self.a_zoomsel = self._act("Na výběr", self.zoom_selected, "Z", "Přiblížit na vybrané")
        views = [("Iso", "iso", "0"), ("Přední", "front", "1"), ("Zadní", "back", "2"),
                 ("Horní", "top", "3"), ("Spodní", "bottom", "4"), ("Levý", "left", "5"),
                 ("Pravý", "right", "6")]
        # triggered(bool checked) would overwrite a k=k default -> swallow the flag explicitly
        self.view_acts = [self._act(t, (lambda _checked=False, k=k: self.view and self.view.set_orientation(k)),
                                    sc, f"Pohled {t.lower()}")
                          for t, k, sc in views]
        # Onshape view shortcuts in addition: Shift+1 front … Shift+7 isometric
        onshape = {"front": "Shift+1", "back": "Shift+2", "left": "Shift+3", "right": "Shift+4",
                   "top": "Shift+5", "bottom": "Shift+6", "iso": "Shift+7"}
        for a, (t, k, sc) in zip(self.view_acts, views):
            a.setShortcuts([QKeySequence(sc), QKeySequence(onshape[k])])
            a.setToolTip(f"Pohled {t.lower()}  [{sc} nebo {onshape[k]}]")
        self.a_edges = self._act("Hrany", self.toggle_edges, "E", "Zobrazit hrany", True)
        self.a_wire = self._act("Drátově", self.toggle_wire, "W", "Drátový model", True)
        self.a_persp = self._act("Perspektiva", lambda on: self.view and self.view.set_perspective(on), "P",
                                 "Perspektivní promítání", True)
        self.a_measure = self._act("Měřit", lambda on: self.set_measure("dist" if on else None), "M",
                                   "3D: klikej na vrcholy, hrany a plochy – délky, Ø / R, plochy, vzdálenosti "
                                   "s ΔX/ΔY/ΔZ, úhly. 2D: vzdálenost dvou bodů", True)
        self.a_angle = self._act("Úhel", lambda on: self.set_measure("angle" if on else None), "U",
                                 "Měření úhlu: klikni na konec ramene, vrchol a konec druhého ramene", True)
        self.a_copy_measure = self._act("Kopírovat výsledek měření", self.copy_measure, "Ctrl+C",
                                        "Zkopírovat výsledek měření do schránky")
        self.a_material = self._act("Materiál…", self.assign_material, None,
                                    "Přiřadit materiál (hustotu) vybraným dílům")
        self.a_mass = self._act("Hmotnost", self.show_mass, "G",
                                "Hmotnost, objem a těžiště vybraných dílů (bez výběru celého modelu)")
        self.a_collide = self._act("Kolize", self.run_collisions, "K",
                                   "Najít kolize (průniky) mezi díly – vybranými, jinak všemi viditelnými")
        self.a_shot = self._act("Snímek", self.screenshot, "Ctrl+S", "Uložit obrázek pohledu (PNG)")
        self.a_hide = self._act("Skrýt", self.hide_selected, "H", "Skrýt vybrané")
        self.a_iso = self._act("Jen vybrané", self.isolate_selected, "I", "Zobrazit jen vybrané")
        self.a_all = self._act("Zobrazit vše", self.show_all, "A", "Zobrazit všechny díly")
        self.a_transp = self._act("Průhledné", self.toggle_transp_selected, "T", "Přepnout průhlednost vybraných")
        self.a_esc = self._act("Zrušit výběr", self.clear_selection, "Esc")
        self.a_expand = self._act("Rozbalit vše", lambda: self.tree.expandAll(), None)
        self.a_collapse = self._act("Sbalit vše", self.collapse_tree, None)
        self.a_about = self._act("Nápověda", self.about, "F1", "Ovládání a klávesové zkratky")
        self.a_edges.setChecked(True)

        tb = QToolBar("Nástroje")
        tb.setMovable(False)
        tb.setIconSize(QSize(18, 18))
        tb.setToolButtonStyle(Qt.ToolButtonTextBesideIcon)
        self.addToolBar(tb)
        self.recent_btn = QToolButton()
        self.recent_btn.setDefaultAction(self.a_open)
        self.recent_btn.setToolButtonStyle(Qt.ToolButtonTextBesideIcon)
        self.recent_menu = QMenu(self)
        self.recent_menu.aboutToShow.connect(self.fill_recent)
        self.recent_btn.setMenu(self.recent_menu)
        self.recent_btn.setPopupMode(QToolButton.MenuButtonPopup)
        tb.addWidget(self.recent_btn)
        tb.addAction(self.a_reload)
        tb.addSeparator()
        tb.addAction(self.a_fit)
        tb.addAction(self.a_zoomsel)
        o3 = list(self.view_acts) + [self.a_edges, self.a_wire, self.a_persp, self.a_transp,
                                     self.a_material, self.a_mass, self.a_collide]
        for a in self.view_acts:
            tb.addAction(a)
        o3.append(tb.addSeparator())
        for a in (self.a_edges, self.a_wire, self.a_persp):
            tb.addAction(a)
        o3.append(tb.addSeparator())
        o3.append(tb.addWidget(QLabel(" Řez: ")))
        self.sec_axis = QComboBox()
        self.sec_axis.addItems(["vyp.", "X", "Y", "Z"])
        self.sec_axis.setToolTip("Řezová rovina kolmá na osu")
        self.sec_axis.currentIndexChanged.connect(self.update_section)
        o3.append(tb.addWidget(self.sec_axis))
        self.sec_slider = QSlider(Qt.Horizontal)
        self.sec_slider.setRange(0, 1000)
        self.sec_slider.setValue(500)
        self.sec_slider.setFixedWidth(140)
        self.sec_slider.setEnabled(False)
        self.sec_slider.valueChanged.connect(self.update_section)
        o3.append(tb.addWidget(self.sec_slider))
        self.sec_flip = QCheckBox("otočit")
        self.sec_flip.setEnabled(False)
        self.sec_flip.toggled.connect(self.update_section)
        o3.append(tb.addWidget(self.sec_flip))
        self.chk_scale = QCheckBox("měřítko")
        self.chk_scale.setToolTip("Měřítko v levém dolním rohu (jen v pravoúhlém promítání)")
        self.chk_scale.setChecked(self.settings.value("scale_bar", True) in (True, "true"))
        self.chk_scale.toggled.connect(self.on_scale_bar)
        o3.append(tb.addWidget(self.chk_scale))
        # 2D only
        o2 = [tb.addWidget(QLabel(" Layout: "))]
        self.layout_combo = QComboBox()
        self.layout_combo.setToolTip("Prostor modelu / výkresové listy (layouty)")
        self.layout_combo.setMinimumContentsLength(14)
        self.layout_combo.currentIndexChanged.connect(self.on_layout_changed)
        o2.append(tb.addWidget(self.layout_combo))
        self.light_bg = QCheckBox("bílé pozadí")
        self.light_bg.setToolTip("Bílé pozadí i v prostoru modelu")
        self.light_bg.setChecked(self.settings.value("light_bg", False) in (True, "true"))
        self.light_bg.toggled.connect(self.on_light_bg)
        o2.append(tb.addWidget(self.light_bg))
        self.chk_rulers = QCheckBox("pravítka")
        self.chk_rulers.setToolTip("Pravítka se souřadnicemi výkresu nahoře a vlevo")
        self.chk_rulers.setChecked(self.settings.value("rulers", True) in (True, "true"))
        self.chk_rulers.toggled.connect(self.on_rulers)
        o2.append(tb.addWidget(self.chk_rulers))
        self.panel2d.set_rulers(self.chk_rulers.isChecked())
        self.only3d, self.only2d = o3, o2
        for a in o2:
            a.setVisible(False)
        tb.addSeparator()
        tb.addAction(self.a_measure)
        tb.addAction(self.a_angle)
        tb.addAction(self.a_mass)
        tb.addAction(self.a_collide)
        tb.addAction(self.a_shot)
        spacer = QWidget()
        spacer.setSizePolicy(spacer.sizePolicy().horizontalPolicy().Expanding,
                             spacer.sizePolicy().verticalPolicy().Preferred)
        tb.addWidget(spacer)
        tb.addAction(self.a_about)

    def _build_status(self):
        sb = self.statusBar()
        self.st_model = QLabel("")
        self.st_model.setTextFormat(Qt.PlainText)     # shows file names: never let a QLabel guess "rich text"
        self.st_prog = QProgressBar()
        self.st_prog.setRange(0, 0)
        self.st_prog.setFixedWidth(160)
        self.st_prog.setVisible(False)
        self.st_cancel = QPushButton("Zrušit")
        self.st_cancel.setVisible(False)
        self.st_cancel.clicked.connect(lambda _checked=False: self.cancel_load())
        sb.addWidget(self.st_model, 1)
        self.st_coord = QLabel("")
        self.st_coord.setTextFormat(Qt.PlainText)
        self.st_coord.setMinimumWidth(200)
        sb.addPermanentWidget(self.st_coord)
        sb.addPermanentWidget(self.st_prog)
        sb.addPermanentWidget(self.st_cancel)

    def show_welcome(self):
        self.props.setHtml(
            f"<h3>{APP_NAME}</h3>"
            "<p>3D: <b>STEP, IGES, STL</b> · 2D: <b>DXF, DWG</b></p>"
            "<p>Otevři soubor <b>Ctrl+O</b>, nebo ho přetáhni do okna.</p>"
            "<p><b>3D myš (jako Onshape):</b> pravé tlačítko + tažení = otáčení kolem bodu pod kurzorem, "
            "prostřední nebo Ctrl+pravé = posun, kolečko = zoom, šipky = otočení o 15°. Klik levým = výběr, Ctrl+klik = přidat k výběru, "
            "dvojklik = přiblížit.</p>"
            "<p><b>2D myš:</b> tažení = posun, kolečko = zoom, klik = údaje o entitě (délka, poloměr, kóta…).</p>"
            "<p><b>3Dconnexion SpaceMouse:</b> model se pohybuje jako čepička – posun, tah k sobě = přiblížit, "
            "kroucení a naklápění = otáčení (kolem bodu uprostřed obrazovky); tlačítko Fit = vše, na SpaceMouse Pro "
            "i pohledy. Nastavení rychlosti a os: šipka u „Otevřít“ → 3D myš.</p>"
            "<p><b>Klávesy:</b> F vše · Z na výběr · H skrýt · I jen vybrané · A vše zobrazit · "
            "T průhledné · E hrany · W drátově · P perspektiva · M měřit (vrcholy, hrany, plochy) · "
            "G hmotnost a těžiště · K kolize · 0–6 nebo Shift+1…7 pohledy · F5 znovu načíst · F1 nápověda</p>")

    # ------------------------------------------------------------------ loading
    def open_dialog(self):
        self.prestart_worker()                 # imports OCCT while the user is still picking a file
        if self.drawing_likely():
            prewarm_ezdxf()                    # ... and ezdxf, when the last file was a drawing (3D sessions never pay)
        d = self.settings.value("lastdir", "")
        path, _ = QFileDialog.getOpenFileName(
            self, "Otevřít model / výkres", d,
            "Všechny CAD soubory (*.step *.stp *.iges *.igs *.stl *.dxf *.dwg);;"
            "3D modely (*.step *.stp *.iges *.igs *.stl);;Výkresy (*.dxf *.dwg);;Všechny soubory (*.*)")
        if path:
            self.open_file(path)

    def drawing_likely(self):
        """The next file is probably a drawing: one is on screen or was the most recent one opened."""
        if self.mode == "2d":
            return True
        r = self.settings.value("recent", [])
        r = [r] if isinstance(r, str) else (r or [])
        return bool(r) and os.path.splitext(str(r[0]))[1].lower() in view2d.DRAW_EXT

    def reload(self):
        if self.path:
            self.open_file(self.path)

    def fill_recent(self):
        self.prestart_worker()
        self.recent_menu.clear()
        rec = self.recent()
        if not rec:
            a = self.recent_menu.addAction("(žádné nedávné soubory)")
            a.setEnabled(False)
        for p in rec:
            a = self.recent_menu.addAction(os.path.basename(p).replace("&", "&&"))   # "&" would be a mnemonic
            a.setToolTip(rich_tip(p))
            a.setStatusTip(p)
            a.triggered.connect(lambda _=False, p=p: self.open_file(p))
        self.recent_menu.addSeparator()
        self.recent_menu.addAction("Vymazat mezipaměť načtených modelů", clear_cache)
        if sys.platform == "win32":
            self.recent_menu.addAction("Náhledy v Průzkumníku…", self.thumb_settings)
        if self.spacemouse is not None:
            self.recent_menu.addAction("3D myš (3Dconnexion)…", self.spacemouse_settings)

    def thumb_settings(self):
        import thumbsettings
        thumbsettings.ThumbDialog(self).exec()

    # ------------------------------------------------------------------ 3D mouse (3Dconnexion)
    def showEvent(self, e):
        super().showEvent(e)
        sm = self.spacemouse
        if sm is not None and not sm.registered:
            try:
                sm.attach(int(self.winId()))
            except Exception:  # noqa: BLE001
                pass

    def nativeEvent(self, eventType, message):
        sm = self.spacemouse
        if sm is not None and sm.registered:
            try:
                sm.handle(message)
            except Exception:  # noqa: BLE001 – a malformed report must not break the window procedure
                pass
        return super().nativeEvent(eventType, message)

    def _space_motion(self, v, dt):
        if self._showing or QApplication.activeModalWidget() is not None:
            return
        if self.mode == "3d" and self.view is not None:
            self.view.space_motion(v, dt)
        elif self.mode == "2d":
            self.view2d.space_motion(v, dt)

    def _space_button(self, action):
        if QApplication.activeModalWidget() is not None:
            return
        if action == "fit":
            self.fit()
        elif self.mode == "3d" and self.view is not None:
            if action in ("roll_cw", "roll_ccw"):
                self.view.roll(math.pi / 2 if action == "roll_ccw" else -math.pi / 2)
            else:
                self.view.set_orientation(action)

    def spacemouse_settings(self):
        import spacemouse
        spacemouse.SpaceMouseDialog(self.spacemouse, self).exec()

    def recent(self):
        r = self.settings.value("recent", [])
        if isinstance(r, str):
            r = [r]
        return [p for p in (r or []) if os.path.exists(p)]

    def add_recent(self, path):
        r = [p for p in self.recent() if os.path.normcase(p) != os.path.normcase(path)]
        self.settings.setValue("recent", [path] + r[:11])
        self.settings.setValue("lastdir", os.path.dirname(path))

    # ---- load workers (separate processes: STEP translation memory never ends up in the GUI process)
    def _warm_ok(self):
        """Idle pre-started worker that will still be waiting long enough for a job."""
        w = self._warm
        return (w is not None and w[3] is None and w[0].poll() is None
                and time.time() - w[2] < WORKER_IDLE_S - 15)

    def prestart_worker(self):
        try:
            if self._warm is not None and self._warm[0].poll() is not None:
                _remove(self._warm[1])
                self._warm = None
            if self._warm is None:
                self._warm = spawn_worker()
        except OSError:
            self._warm = None

    def _drop_warm(self):
        if self._warm is not None:
            try:
                self._warm[0].kill()
            except OSError:
                pass
            _remove(self._warm[1])
            self._warm = None

    def open_file(self, path):
        path = os.path.abspath(path)
        if not os.path.exists(path):
            msg_warning(self, APP_NAME, f"Soubor neexistuje:\n{path}")
            return
        if self._showing:                      # a model is being put on screen right now
            return
        if self._proc is not None or self.d_loader is not None:
            self.cancel_load(restore=False)
        self.add_recent(path)
        self._t0 = time.time()
        self._loading_path = path
        self.from_cache = False
        self.set_busy(True, "Načítám " + os.path.basename(path) + "…")
        if os.path.splitext(path)[1].lower() in view2d.DRAW_EXT:
            self.open_drawing(path)
            return
        size = os.path.getsize(path)
        cache = cache_path(path)
        if os.path.exists(cache):
            QApplication.setOverrideCursor(Qt.WaitCursor)
            QApplication.processEvents()
            self.ensure_3d()
            try:
                doc = core.load_xbf(cache)
                os.utime(cache)
                self.from_cache = True
            except Exception:  # noqa: BLE001  – broken cache entry, load the original
                doc = None
                _remove(cache)
            QApplication.restoreOverrideCursor()
            if doc is not None:
                self.show_doc(doc, path)
                return
        early = self._warm is not None and self._warm[3] is not None and \
            os.path.normcase(self._warm[3]) == os.path.normcase(path) and self._warm[0].poll() in (None, 0)
        if not early and self._warm is not None and self._warm[3] is not None:
            self._drop_warm()                  # started for another file
        # small files are quicker in-process, unless an idle worker is already waiting (keeps the GUI lean)
        if not early and size < WORKER_THRESHOLD and not (size >= CACHE_MIN and self._warm_ok()):
            QApplication.setOverrideCursor(Qt.WaitCursor)
            QApplication.processEvents()
            self.ensure_3d()
            try:
                doc = core.read_cad(path)
            except Exception as ex:  # noqa: BLE001
                QApplication.restoreOverrideCursor()
                self.set_busy(False)
                msg_critical(self, APP_NAME, f"Nepodařilo se načíst model:\n{ex}")
                return
            QApplication.restoreOverrideCursor()
            self.show_doc(doc, path)
            if size >= CACHE_MIN:                  # the model is on screen first; saving the copy can wait a moment
                QTimer.singleShot(0, lambda: self.doc is doc and store_cache(doc, cache))
            return
        ensure_cache_dir()
        self._cache = cache
        self._tmp = part_path(cache)
        try:
            if early:                              # job already submitted at program start
                w = self._warm
            else:
                _remove(self._tmp + ".log")
                w = self._warm if self._warm_ok() else None
                if w is None:
                    self._drop_warm()
                    w = spawn_worker()
                submit_job(w[1], path, self._tmp)
        except OSError as ex:
            self.set_busy(False)
            msg_critical(self, APP_NAME, f"Nepodařilo se spustit načítání:\n{ex}")
            return
        self._warm = None
        self._proc, self._job = w[0], w[1]
        self.st_cancel.setVisible(True)
        self._poll = QTimer(self)
        self._poll.timeout.connect(self.poll_worker)
        self._poll.start(150)
        QApplication.processEvents()           # paint the progress bar before the GUI thread gets busy below
        self.ensure_3d()                       # import OCCT here while the worker reads the file
        if self.mode != "2d":                  # (a drawing on screen stays until the model is ready)
            self.set_mode("3d")                # create the OpenGL context now too (0.3 - 1 s), not after the load

    def set_busy(self, on, text=""):
        self.st_prog.setVisible(on)
        self.st_cancel.setVisible(on and self._proc is not None)
        if on:
            self.st_model.setText(text)

    def _leave_empty_3d(self):
        """A load put the still empty 3D page on screen early (see open_file) and then failed / was cancelled / was
        refused: with nothing loaded, go back to the start screen instead of leaving an empty 3D view.
        (A model or drawing already on screen stays - the early switch is only made when there is no drawing.)"""
        if self.mode == "3d" and self.doc is None and self.ddoc is None and not self.nodes:
            self.set_measure(None)
            self.mode = None
            self.stack.setCurrentWidget(self.empty)
            self.st_coord.setText("")

    def _log_lines(self):
        try:
            with open(self._tmp + ".log", encoding="utf-8") as f:
                return [ln.strip() for ln in f if ln.strip()]
        except OSError:
            return []

    def poll_worker(self):
        code = self._proc.poll()
        if code is not None:
            self.on_worker_finished(code)
            return
        lines = self._log_lines()
        msg = lines[-1] if lines else "Spouštím načítání…"
        self.st_cancel.setVisible(True)
        self.st_model.setText(f"{os.path.basename(self._loading_path)}: {msg}  ({time.time() - self._t0:.0f} s)")

    def _cleanup_tmp(self):
        if self._tmp:
            _remove(self._tmp)
            _remove(self._tmp + ".log")
        self._tmp = None
        _remove(self._job)                      # left over only if the worker died before taking it
        self._job = None

    def cancel_load(self, restore=True):
        if self.d_loader is not None:          # a thread can't be killed – just drop its result
            self.d_loader.loaded.disconnect()
            self.d_loader.failed.disconnect()
            self.d_loader = None
        if self._proc is not None:
            self._poll.stop()
            try:
                self._proc.kill()
                self._proc.wait(3)
            except Exception:  # noqa: BLE001
                pass
            self._proc = None
            self._cleanup_tmp()
        self.set_busy(False)
        if restore:                            # (open_file cancels the old load only to start the next one)
            self._leave_empty_3d()
        self.update_status()

    def on_worker_finished(self, code):
        self._poll.stop()
        self._proc = None
        lines = self._log_lines()
        if code != 0 or not lines or lines[-1] != "DONE":
            err = next((ln[6:] for ln in lines if ln.startswith("ERROR ")), None)
            self._cleanup_tmp()
            self.set_busy(False)
            self._leave_empty_3d()
            self.update_status()
            msg_critical(self, APP_NAME, "Nepodařilo se načíst model:\n" +
                                 (err or f"načítací proces skončil chybou (kód {code})"))
            return
        try:
            self.st_model.setText("Zobrazuji model…")
            QApplication.setOverrideCursor(Qt.WaitCursor)
            QApplication.processEvents()
            os.replace(self._tmp, self._cache)
            doc = core.load_xbf(self._cache)
        except Exception as ex:  # noqa: BLE001
            QApplication.restoreOverrideCursor()
            _remove(self._cache)
            self.set_busy(False)
            self._leave_empty_3d()
            self.update_status()
            msg_critical(self, APP_NAME, f"Nepodařilo se načíst model:\n{ex}")
            return
        finally:
            self._cleanup_tmp()
        prune_cache()
        QApplication.restoreOverrideCursor()
        self.show_doc(doc, self._loading_path)

    # ------------------------------------------------------------------ 2D drawings
    def set_mode(self, mode):
        if mode == self.mode:
            return
        self.set_measure(None)
        self.mode = mode
        if mode == "3d":
            self.ensure_3d()
        self.stack.setCurrentWidget(self.panel2d if mode == "2d" else self.view)
        if mode == "3d":
            self.view._force_resize()
        for a in self.only3d:
            a.setVisible(mode != "2d")
        for a in self.only2d:
            a.setVisible(mode == "2d")
        self.st_coord.setText("")
        self.set_measure(None)
        self.tree.setHeaderLabels(["Hladina", "Entit"] if mode == "2d" else ["Název", "Info"])
        self.filter.setPlaceholderText("Hledat hladinu…" if mode == "2d" else "Hledat ve stromu…")

    def open_drawing(self, path):
        cache = None
        if path.lower().endswith(".dwg"):
            ensure_cache_dir()
            cache = cache_path(path)[:-4] + ".dxf"
            self.from_cache = os.path.exists(cache)
        self.d_loader = DrawingLoader(path, cache, self)
        self.d_loader.loaded.connect(lambda doc, info, p=path: self.on_drawing_loaded(doc, info, p))
        self.d_loader.failed.connect(self.on_drawing_failed)
        self.d_loader.progress.connect(
            lambda m: self.st_model.setText(f"{os.path.basename(path)}: {m}  ({time.time() - self._t0:.0f} s)"))
        self.d_loader.finished.connect(self.d_loader.deleteLater)
        self.st_cancel.setVisible(True)
        self.d_loader.start()

    def on_drawing_failed(self, msg):
        self.d_loader = None
        self.set_busy(False)
        self._leave_empty_3d()
        self.update_status()
        msg_critical(self, APP_NAME, f"Nepodařilo se načíst výkres:\n{msg}")

    def on_drawing_loaded(self, doc, info, path):
        self.d_loader = None
        prune_cache()
        QApplication.setOverrideCursor(Qt.WaitCursor)
        try:
            self.clear_model()
            self.set_mode("2d")
            self.ddoc, self.d_info, self.path = doc, info, path
            self.d_vis = {}
            self.d_layout = view2d.default_layout(doc)
            self.layout_combo.blockSignals(True)
            self.layout_combo.clear()
            for name in doc.layouts.names_in_taborder():
                n = view2d.drawable_count(doc.layouts.get(name))
                self.layout_combo.addItem(f"{'Model' if name == 'Model' else name}  ({n})", name)
            self.layout_combo.setCurrentIndex(max(0, self.layout_combo.findData(self.d_layout)))
            self.layout_combo.blockSignals(False)
            self.st_model.setText("Vykresluji…")
            QApplication.processEvents()
            self.render_drawing(keep_view=False)
        finally:
            QApplication.restoreOverrideCursor()
        self.load_time = time.time() - self._t0
        self.set_busy(False)
        self.setWindowTitle(f"{os.path.basename(path)} – {APP_NAME}")
        self.update_status()
        self.show_drawing_info()

    def render_drawing(self, keep_view=True):
        if self.ddoc is None:
            return
        QApplication.setOverrideCursor(Qt.WaitCursor)
        try:
            try:
                scene, _ = view2d.render_scene(self.ddoc, self.d_layout, self.d_vis, self.light_bg.isChecked(),
                                               track=True)     # layer toggles then only show / hide items
            except view2d.DrawingTooComplex as ex:      # nested-block bomb / cyclic block: draw nothing
                scene, keep_view = view2d.QGraphicsScene(), False
                text, key = str(ex), (id(self.ddoc), self.d_layout)
                if key != getattr(self, "_warned_complex", None):      # once per layout, not per layer toggle
                    self._warned_complex = key
                    QTimer.singleShot(0, lambda: msg_warning(self, APP_NAME, text))
            self.view2d.set_scene(scene, keep_view)
            self.build_layer_items()
        finally:
            QApplication.restoreOverrideCursor()

    def refresh_layers(self):
        """self.d_vis changed: show / hide the already drawn items (milliseconds); the layout is drawn again only
        when the scene has no layer map (viewports with their own layer states, a refused drawing)."""
        if self.ddoc is None:
            return
        if self.view2d.apply_layers(self.d_vis):
            self.sync_layer_checks()
        else:
            self.render_drawing(keep_view=True)

    def sync_layer_checks(self):
        """Layer list check boxes follow d_vis (hide selected / only these / show all)."""
        self.tree.blockSignals(True)
        try:
            for i in range(self.tree.topLevelItemCount()):
                it = self.tree.topLevelItem(i)
                v = self.d_vis.get(it.data(0, Qt.UserRole))
                if v is not None:
                    it.setCheckState(0, Qt.Checked if v else Qt.Unchecked)
        finally:
            self.tree.blockSignals(False)

    def build_layer_items(self):
        counts = collections.Counter(view2d.layer_of(e).lower()
                                     for e in self.ddoc.layouts.get(self.d_layout))
        sel = {it.data(0, Qt.UserRole) for it in self.tree.selectedItems()}
        self.tree.blockSignals(True)
        self.tree.clear()
        for name, color, on in sorted(view2d.layer_table(self.ddoc), key=lambda t: t[0].lower()):
            key = name.lower()
            it = QTreeWidgetItem([name, str(counts.get(key, 0)) if counts.get(key) else ""])
            it.setData(0, Qt.UserRole, key)
            it.setFlags(it.flags() | Qt.ItemIsUserCheckable)
            it.setCheckState(0, Qt.Checked if self.d_vis.get(key, on) else Qt.Unchecked)
            pm = QPixmap(14, 14)
            pm.fill(color)
            it.setIcon(0, QIcon(pm))
            if not on:
                it.setToolTip(0, "Ve výkresu vypnutá / zmrazená hladina")
            self.tree.addTopLevelItem(it)
            if key in sel:
                it.setSelected(True)
        self.tree.blockSignals(False)
        self.apply_filter(self.filter.text())

    def on_layout_changed(self, idx):
        if self.ddoc is None or idx < 0:
            return
        self.d_layout = self.layout_combo.itemData(idx)
        self.render_drawing(keep_view=False)
        self.update_status()
        self.show_drawing_info()

    def on_light_bg(self, on):
        self.settings.setValue("light_bg", on)
        if self.ddoc is not None:
            self.render_drawing(keep_view=True)

    def selected_layers(self):
        return [it.data(0, Qt.UserRole) for it in self.tree.selectedItems()]

    def set_layers_visible(self, keys, vis, others=None):
        """vis for keys; others (if not None) for every other layer."""
        for i in range(self.tree.topLevelItemCount()):
            it = self.tree.topLevelItem(i)
            k = it.data(0, Qt.UserRole)
            v = vis if k in keys else others
            if v is not None:
                self.d_vis[k] = v
        self.refresh_layers()

    def drawing_extents(self):
        r = self.view2d.content_rect
        return None if r.isEmpty() else r

    def show_drawing_info(self):
        doc = self.ddoc
        if doc is None:
            return
        rel, units = view2d.doc_info(doc)
        lay = doc.layouts.get(self.d_layout)
        types = collections.Counter(e.dxftype() for e in lay)
        size = os.path.getsize(self.path) / 1024 / 1024
        rows = [("Složka", os.path.dirname(self.path)), ("Velikost", f"{size:.2f} MB"),
                ("Verze", f"{rel} ({doc.dxfversion})"), ("Jednotky", units),
                ("Layout", "Model" if self.d_layout == "Model" else self.d_layout),
                ("Entit", f"{len(lay)}"), ("Hladin / bloků", f"{len(doc.layers)} / {len(doc.blocks)}")]
        r = self.drawing_extents()
        if r is not None:
            rows.append(("Rozměr výkresu", Html(f"<b>{fmt_len(r.width())} × {fmt_len(r.height())}</b> {esc(units)}")))
        rows.append(("Typy entit", ", ".join(f"{t} {n}" for t, n in types.most_common(8))))
        acis = sum(n for t, n in types.items() if t in ("3DSOLID", "BODY", "REGION", "SURFACE"))
        if acis:
            rows.append(("Upozornění", Html(f"<span style='color:#d08000'>{acis}× 3D těleso (ACIS) – "
                                            "v 2D náhledu se nezobrazí</span>")))
        blocked = view2d.remote_images(doc)
        if blocked:
            rows.append(("Obrázky", Html(f"<span style='color:#d08000'>{len(blocked)}× obrázek ze síťové cesty "
                                          "nenačten (bezpečnost) – zobrazen jen rámeček</span>")))
        if self.d_info.get("inventor"):
            rows.append(("Inventor", f"{self.d_info['inventor']} pohledů převedeno"))
        if self.d_info.get("converter"):
            rows.append(("Převod DWG", self.d_info["converter"]))
        if self.d_info.get("errors"):
            rows.append(("Opraveno chyb", str(self.d_info["errors"])))
        rows.append(("Načteno za", f"{self.load_time:.1f} s"))
        self.props.setHtml(f"<h3>{esc(os.path.basename(self.path))}</h3>" + self._rows_html(rows))

    def on_entity_picked(self, top, sub):
        if top is None:
            self.show_drawing_info()
            return
        rows = view2d.entity_rows(top)
        if sub is not None and sub is not top:
            rows.append(("<b>V bloku</b>", ""))
            rows += [(k, v) for k, v in view2d.entity_rows(sub) if k != "Handle"]
        self._show_rows(rows)

    def on_measured2d(self, a, b):
        dx, dy = b[0] - a[0], b[1] - a[1]
        d = (dx * dx + dy * dy) ** 0.5
        import math
        ang = math.degrees(math.atan2(dy, dx)) % 360
        self.st_model.setText(f"Vzdálenost: {fmt_len(d)}   (ΔX {fmt_len(abs(dx))}, ΔY {fmt_len(abs(dy))}, "
                              f"úhel {fmt_len(ang)}°)   – další klik = nové měření, Esc = konec")

    def fit(self):
        if self.mode == "2d":
            self.view2d.fit()
        elif self.view is not None:
            self.view.fit_all()

    # ------------------------------------------------------------------ model display
    def clear_model(self):
        self.set_measure(None)
        if self.collide_dlg is not None:
            self.collide_dlg.close()
            self.collide_dlg = None
        self.mats = None
        if self.view is None:                   # nothing 3D shown yet
            self.tree.clear()
            self.nodes, self.roots, self.ais_node, self.protos = [], [], {}, {}
            self.leaf_cache, self.vol_cache = {}, {}
            self.doc = self.model_box = self.ddoc = None
            self.view2d.set_scene(view2d.QGraphicsScene())
            return
        self.view.clear_overlay()
        ctx = self.view.ctx
        self._mesh_token = None                     # a mesh still being analysed belongs to this model
        for o in self.view.feature_objs:
            ctx.Remove(o, False)
        self.view.feature_objs, self.view.meshes = {}, {}
        for n in self.nodes:
            if n.ais is not None:
                ctx.Remove(n.ais, False)
        for p in self.protos.values():     # never displayed, but their presentations / selections live in ctx
            ctx.Remove(p, False)
        self.protos = {}
        self.tree.clear()
        self.nodes, self.roots, self.ais_node = [], [], {}
        self.leaf_cache, self.vol_cache = {}, {}
        self.doc = None
        self.model_box = None
        self.ddoc = None
        self.view2d.set_scene(view2d.QGraphicsScene())

    def _style(self, o):
        """Colour of uncoloured faces + edges, for every presentation (standalone or shared prototype)."""
        o.SetColor(rgb(*DEFAULT_COLOR))
        self.view.apply_edge_style(o, self.a_edges.isChecked())

    def show_doc(self, doc, path):
        try:
            core.count_instances(doc)       # cheap walk of the assembly DAG: instance bombs / cycles are refused
        except core.ModelTooComplex as ex:  # before anything is expanded (also for models read from the cache)
            self.set_busy(False)
            self._leave_empty_3d()
            self.update_status()
            msg_warning(self, APP_NAME, str(ex))
            return
        QApplication.setOverrideCursor(Qt.WaitCursor)
        self._showing = True            # processEvents below must not start another load meanwhile
        try:
            self.clear_model()
            self.set_mode("3d")
            self.doc = doc
            self.path = path
            self.mats = materials.Assignments(CACHE_DIR, path)
            self.roots, self.nodes, self.proto_count = core.build_tree(doc, check=False)   # counted above
            ctx = self.view.ctx
            wire = self.a_wire.isChecked()
            leaves = [n for n in self.nodes if not n.is_asm]
            objs, self.protos = leaf_objects(leaves, self.proto_count, self._style)
            t = time.time()
            for n, o in zip(leaves, objs):
                n.ais = o
                self.ais_node[o] = n
                ctx.Display(o, AIS_WireFrame if wire else AIS_Shaded, 0, False)
                if time.time() - t > 0.3:
                    QApplication.processEvents()
                    t = time.time()
            self.build_tree_items()
            self.setup_mesh()
            self.model_box = self.nodes_box(self.roots)
            self.view.fit_all(redraw=False)
            self.update_section_range(redraw=False)
            self.view.redraw()              # one redraw (= the first frame, GPU upload) instead of three
        finally:
            self._showing = False
            QApplication.restoreOverrideCursor()
        self.load_time = time.time() - self._t0
        self.set_busy(False)
        self.setWindowTitle(f"{os.path.basename(path)} – {APP_NAME}")
        self.update_status()
        self.show_model_info()

    def setup_mesh(self):
        """STL: numpy mesh for measuring (vertex snapping, flat faces, sharp-edge loops / holes, facet grid),
        built in a background thread - big meshes take a second or two and the model is usable meanwhile."""
        self._mesh_token = None
        leaves = [n for n in self.nodes if not n.is_asm and n.ais is not None]
        if len(leaves) != 1 or not self.path.lower().endswith(".stl"):
            return
        import threading
        import meshmeasure
        import viewer3d
        token, leaf, path = object(), leaves[0], self.path
        self._mesh_token = token

        def work():
            mesh = None
            try:
                m = meshmeasure.mesh_for_file(path)       # None above meshmeasure.MAX_TRIS facets
                if m is not None:
                    loops = m.loops()
                    m.prepare()
                    # the line shape is built here too: ~10 µs per point, a GUI freeze for big meshes
                    m.feature_shape = viewer3d.polylines_compound(loops) if loops else None
                    mesh = m
            except Exception:  # noqa: BLE001 – measuring falls back to plain picked points
                pass
            self.meshReady.emit(token, leaf, mesh)

        threading.Thread(target=work, name="stl-mesh", daemon=True).start()

    def _mesh_ready(self, token, leaf, mesh):
        """GUI thread: the mesh of the open STL is analysed - draw its sharp edges, enable mesh measuring."""
        if token is not self._mesh_token or mesh is None or self.view is None or leaf.ais is None:
            return
        self._mesh_token = None
        from OCP.AIS import AIS_Shape
        ctx = self.view.ctx
        feat = None
        shape, mesh.feature_shape = mesh.feature_shape, None
        if shape is not None:
            feat = AIS_Shape(shape)
            feat.SetColor(rgb(25, 25, 30))
            feat.SetWidth(1.0)
            ctx.Display(feat, 0, -1, False)
            ctx.Deactivate(feat)
            if not self.a_edges.isChecked():
                ctx.Erase(feat, False)
        self.view.add_mesh(leaf.ais, mesh, feat)
        ctx.UpdateCurrentViewer()
        if getattr(self, "_measure_kind", None) is None and self.selected_nodes() == [leaf]:
            self.show_props(leaf)                   # + mesh rows (facets, closed, holes)

    def build_tree_items(self):
        self.tree.blockSignals(True)
        self.tree.setUpdatesEnabled(False)
        pix_cache = {}

        def icon_for(n):
            c = qcolor(n.color) if n.color is not None else None
            key = ("asm" if n.is_asm else "part", c.name() if c else None)
            if key not in pix_cache:
                pm = QPixmap(14, 14)
                pm.fill(Qt.transparent)
                from PySide6.QtGui import QPainter
                p = QPainter(pm)
                p.setRenderHint(QPainter.Antialiasing)
                if n.is_asm:
                    p.setPen(QColor(90, 100, 120))
                    p.setBrush(QColor(255, 210, 120) if c is None else c)
                    p.drawRect(1, 3, 12, 9)
                    p.drawRect(1, 1, 6, 3)
                else:
                    p.setPen(QColor(70, 70, 80))
                    p.setBrush(c if c is not None else QColor(*DEFAULT_COLOR))
                    p.drawRoundedRect(1, 1, 12, 12, 3, 3)
                p.end()
                pix_cache[key] = QIcon(pm)
            return pix_cache[key]

        def add(n, parent_item):
            it = QTreeWidgetItem([n.name, ""])
            n.item = it
            it.setData(0, Qt.UserRole, n.idx)
            it.setFlags(it.flags() | Qt.ItemIsUserCheckable | Qt.ItemIsAutoTristate)
            it.setCheckState(0, Qt.Checked)
            it.setIcon(0, icon_for(n))
            if n.is_asm:
                it.setText(1, f"{len(self.leaves(n))} ks")
                tip = f"Sestava: {n.proto_name}"
            else:
                cnt = self.proto_count.get(n.key, 1)
                it.setText(1, f"×{cnt}" if cnt > 1 else "")
                tip = f"Díl: {n.proto_name}" + (f"\nPoužit {cnt}× v modelu" if cnt > 1 else "")
            if n.name != n.proto_name and n.proto_name:
                tip = f"Instance: {n.name}\n" + tip
            it.setToolTip(0, rich_tip(tip))
            if parent_item is None:
                self.tree.addTopLevelItem(it)
            else:
                parent_item.addChild(it)
            for c in n.children:
                add(c, it)
            return it

        for r in self.roots:
            add(r, None)
        # expand first two levels
        for i in range(self.tree.topLevelItemCount()):
            top = self.tree.topLevelItem(i)
            top.setExpanded(True)
            if self.tree.topLevelItemCount() == 1 and top.childCount() <= 30:
                for j in range(top.childCount()):
                    if top.child(j).childCount() <= 15:
                        top.child(j).setExpanded(False)
        self.update_tree_materials()
        self.tree.setUpdatesEnabled(True)
        self.tree.blockSignals(False)

    def leaves(self, n):
        r = self.leaf_cache.get(n.idx)
        if r is None:
            if not n.is_asm:
                r = [n]
            else:
                r = []
                for c in n.children:
                    r.extend(self.leaves(c))
            self.leaf_cache[n.idx] = r
        return r

    def node_of(self, item):
        return self.nodes[item.data(0, Qt.UserRole)]

    def located_shape(self, leaf):
        s = XCAFDoc_ShapeTool.GetShape_s(leaf.label)
        if not leaf.parent_loc.IsIdentity():
            s = s.Moved(leaf.parent_loc)
        return s

    def nodes_box(self, nodes):
        box = Bnd_Box()
        for n in nodes:
            for lf in self.leaves(n):
                BRepBndLib.AddOptimal_s(self.located_shape(lf), box, True, False)
        return None if box.IsVoid() else box

    # ------------------------------------------------------------------ status / info
    def update_status(self):
        if self.mode == "2d" and self.ddoc is not None:
            rel, units = view2d.doc_info(self.ddoc)
            lay = self.ddoc.layouts.get(self.d_layout)
            txt = f"{os.path.basename(self.path)}  |  {rel}  |  {len(lay)} entit, {len(self.ddoc.layers)} hladin"
            r = self.drawing_extents()
            if r is not None:
                txt += f"  |  rozměr {fmt_len(r.width())} × {fmt_len(r.height())} {units}"
            txt += f"  |  načteno za {self.load_time:.1f} s" + (" (DWG z mezipaměti)" if self.from_cache else "")
            self.st_model.setText(txt)
            return
        if not self.nodes:
            self.st_model.setText("Žádný model")
            return
        parts = sum(1 for n in self.nodes if not n.is_asm)
        asms = sum(1 for n in self.nodes if n.is_asm)
        txt = f"{os.path.basename(self.path)}  |  {parts} dílů ({len(self.proto_count)} unikátních), {asms} sestav"
        if self.model_box is not None:
            x0, y0, z0, x1, y1, z1 = core.box_limits(self.model_box)
            txt += f"  |  rozměr {fmt_len(x1 - x0)} × {fmt_len(y1 - y0)} × {fmt_len(z1 - z0)} mm"
        txt += f"  |  načteno za {self.load_time:.1f} s" + (" (z mezipaměti)" if self.from_cache else "")
        self.st_model.setText(txt)

    def show_model_info(self):
        if not self.nodes:
            return
        size = os.path.getsize(self.path) / 1024 / 1024
        parts = sum(1 for n in self.nodes if not n.is_asm)
        html = (f"<h3>{esc(os.path.basename(self.path))}</h3>"
                f"<table cellspacing=2>"
                f"<tr><td>Složka:</td><td>{esc(os.path.dirname(self.path))}</td></tr>"
                f"<tr><td>Velikost:</td><td>{size:.2f} MB</td></tr>"
                f"<tr><td>Dílů:</td><td>{parts} ({len(self.proto_count)} unikátních)</td></tr>"
                f"<tr><td>Sestav:</td><td>{sum(1 for n in self.nodes if n.is_asm)}</td></tr>")
        if self.model_box is not None:
            x0, y0, z0, x1, y1, z1 = core.box_limits(self.model_box)
            html += (f"<tr><td>Rozměr X×Y×Z:</td><td><b>{fmt_len(x1 - x0)} × {fmt_len(y1 - y0)} × "
                     f"{fmt_len(z1 - z0)} mm</b></td></tr>")
        html += (f"<tr><td>Načteno za:</td><td>{self.load_time:.1f} s" +
                 (" (z mezipaměti)" if self.from_cache else "") + "</td></tr></table>")
        self.props.setHtml(html)

    def show_props(self, n):
        is_asm = n.is_asm
        lv = self.leaves(n)
        rows = [("Název", n.name)]
        if n.proto_name and n.proto_name != n.name:
            rows.append(("Díl / dokument" if not is_asm else "Sestava", n.proto_name))
        rows.append(("Typ", "sestava" if is_asm else "díl"))
        if is_asm:
            rows.append(("Obsahuje", f"{len(lv)} dílů, {len(n.children)} přímých položek"))
        else:
            cnt = self.proto_count.get(n.key, 1)
            rows.append(("Počet v modelu", f"{cnt}×"))
        box = self.nodes_box([n])
        if box is not None:
            x0, y0, z0, x1, y1, z1 = core.box_limits(box)
            rows.append(("Rozměr X×Y×Z", Html(f"<b>{fmt_len(x1 - x0)} × {fmt_len(y1 - y0)} × {fmt_len(z1 - z0)} mm</b>")))
            rows.append(("Min (X, Y, Z)", f"{fmt_len(x0)}, {fmt_len(y0)}, {fmt_len(z0)}"))
            rows.append(("Max (X, Y, Z)", f"{fmt_len(x1)}, {fmt_len(y1)}, {fmt_len(z1)}"))
        if n.color is not None:
            c = qcolor(n.color)
            rows.append(("Barva", Html(f"<span style='background:{c.name()}'>&nbsp;&nbsp;&nbsp;&nbsp;</span> {c.name()}")))
        mesh = self.view.meshes.get(n.ais) if self.view is not None and not is_asm else None
        if mesh is not None:
            rows += mesh.info_rows()
        uncached = {lf.key for lf in lv if lf.key not in self.vol_cache}
        if not is_asm or len(uncached) <= 60:
            rows += self._mass_rows(self.mass_props(lv), n)
        else:
            rows.append(("Objem / hmotnost", Html(f"<a href='calc:{n.idx}'>spočítat ({len(uncached)} unikátních dílů)</a>")))
        self._show_rows(rows)

    def _mass_rows(self, d, n=None):
        rows = [("Objem", f"{fmt_num(d['volume'] / 1000.0, 3)} cm³"),
                ("Povrch", f"≈ {fmt_num(d['area'] / 100.0, 2)} cm²")]
        link = f"<a href='mat:{n.idx}'>přiřadit…</a>" if n is not None else ""
        if d["mats"]:
            rows.append(("Materiál", Html(esc(", ".join(sorted(d["mats"]))) + (f" ({link})" if link else ""))))
        if d["mass"] > 0:
            rows.append(("Hmotnost", Html(f"<b>{fmt_num(d['mass'], 4)} kg</b>")))
            c = d["cog"]
            rows.append(("Těžiště X; Y; Z", f"{fmt_len(c.X())}; {fmt_len(c.Y())}; {fmt_len(c.Z())} mm"))
        if d["missing"]:
            msg = f"{d['missing']} dílů bez materiálu" if d["mass"] > 0 else "bez materiálu"
            rows.append(("Materiál" if not d["mats"] else "Pozor", Html(esc(msg) + (f" ({link})" if link else ""))))
            if d["mass"] == 0:
                rows += [("Hmotnost (ocel 7,85)", f"{fmt_num(d['volume'] * 7.85e-6, 3)} kg"),
                         ("Hmotnost (Al 2,70)", f"{fmt_num(d['volume'] * 2.70e-6, 3)} kg")]
        if d["volume"] > 0 and d["mass"] == 0:
            c = d["vcog"]
            rows.append(("Těžiště objemu", f"{fmt_len(c.X())}; {fmt_len(c.Y())}; {fmt_len(c.Z())} mm"))
        rows.append(("Těles / ploch", f"{d['solids']} / {d['faces']}"))
        return rows

    # ------------------------------------------------------------------ materials / mass
    def material_of(self, lf):
        if self.mats is None:
            return None
        return self.mats.get(lf.proto_name or lf.key)

    def proto_props(self, lf):
        r = self.vol_cache.get(lf.key)
        if r is None:
            s = XCAFDoc_ShapeTool.GetShape_s(lf.proto)
            r = analysis.proto_props(s, core.is_mesh_only(s))
            self.vol_cache[lf.key] = r
        return r

    def mass_props(self, leaves):
        """Totals over leaf instances: volume, area, mass (assigned materials), centre of gravity (world)."""
        d = {"volume": 0.0, "area": 0.0, "solids": 0, "faces": 0, "mass": 0.0, "missing": 0, "mats": set()}
        mx = my = mz = vx = vy = vz = 0.0
        for lf in leaves:
            vol, area, cog, ns, nf = self.proto_props(lf)
            w = cog.Transformed(lf.loc.Transformation())
            d["volume"] += vol
            d["area"] += area
            d["solids"] += ns
            d["faces"] += nf
            vx, vy, vz = vx + vol * w.X(), vy + vol * w.Y(), vz + vol * w.Z()
            m = self.material_of(lf)
            if m is None:
                d["missing"] += 1
                continue
            kg = vol * m[1] * 1e-6                  # mm³ · g/cm³ -> kg
            d["mass"] += kg
            d["mats"].add(m[0])
            mx, my, mz = mx + kg * w.X(), my + kg * w.Y(), mz + kg * w.Z()
        from OCP.gp import gp_Pnt
        d["cog"] = gp_Pnt(mx / d["mass"], my / d["mass"], mz / d["mass"]) if d["mass"] > 0 else None
        v = d["volume"]
        d["vcog"] = gp_Pnt(vx / v, vy / v, vz / v) if v > 0 else gp_Pnt()
        return d

    def _unique_leaves(self, nodes):
        out = {}
        for n in nodes:
            for lf in self.leaves(n):
                out[lf.idx] = lf
        return list(out.values())

    def assign_material(self):
        if self.mode == "2d" or not self.nodes:
            return
        sel = self.selected_nodes() or [self.ais_node[o] for o in self.view.selected_objects() if o in self.ais_node]
        lv = self._unique_leaves(sel)
        if lv:
            cur = self.material_of(lv[0])
            dlg = materials.MaterialDialog(self, f"Materiál – {len(lv)} dílů", cur[:2] if cur else None,
                                           show_default=True, default=self.mats.default)
        else:                                    # nothing selected: the default for parts without material
            dlg = materials.MaterialDialog(self, "Výchozí materiál pro díly bez materiálu", self.mats.default)
        if not dlg.exec():
            return
        name, dens = dlg.result_material()
        if lv:
            self.mats.set({lf.proto_name or lf.key for lf in lv}, name, dens)
            self.mats.default = dlg.result_default()
        else:
            self.mats.default = [name, dens] if name else None
        self.mats.save()
        self.update_tree_materials()
        if len(sel) == 1:
            self.show_props(sel[0])
        else:
            self.show_mass()

    def run_collisions(self):
        if self.mode == "2d" or not self.nodes:
            return
        import collisions
        ctx = self.view.ctx
        sel = self.selected_nodes() or [self.ais_node[o] for o in self.view.selected_objects() if o in self.ais_node]
        lv = self._unique_leaves(sel)
        scope = "vybrané díly"
        if len(lv) < 2:
            lv, scope = [n for n in self.nodes if not n.is_asm], "všechny viditelné díly"
        lv = [lf for lf in lv if lf.ais is not None and ctx.IsDisplayed(lf.ais)]
        parts = []
        for lf in lv:
            s = self.located_shape(lf)
            if not core.is_mesh_only(s):             # STL meshes have no solids for a boolean check
                parts.append((lf, s))
        if len(parts) < 2:
            QMessageBox.information(self, APP_NAME, "Pro kontrolu kolizí jsou potřeba aspoň 2 viditelné díly.")
            return
        if self.collide_dlg is not None:
            self.collide_dlg.close()

        def pick(na, nb, common):
            self.view.clear_overlay()
            self._sync = True
            self.tree.clearSelection()
            for n in (na, nb):
                n.item.setSelected(True)
                p = n.item.parent()
                while p is not None:
                    p.setExpanded(True)
                    p = p.parent()
            self.tree.scrollToItem(na.item)
            self._sync = False
            ctx.ClearSelected(False)
            for n in (na, nb):
                if n.ais is not None and ctx.IsDisplayed(n.ais):
                    ctx.AddOrRemoveSelected(n.ais, False)
            ctx.UpdateCurrentViewer()
            if common is not None:
                self.view.show_shapes([common], (230, 20, 20))
            self.view.fit_box(self.nodes_box([na, nb]))

        def close():
            self.collide_dlg = None
            self.view.clear_overlay()

        self.collide_dlg = collisions.CollisionDialog(self, parts, scope, pick,
                                                      lambda shapes: self.view.show_shapes(shapes, (230, 20, 20)),
                                                      close)
        self.collide_dlg.show()
        self.collide_dlg.run()

    def update_tree_materials(self):
        for n in self.nodes:
            if n.is_asm or n.item is None:
                continue
            cnt = self.proto_count.get(n.key, 1)
            m = self.material_of(n)
            txt = f"×{cnt}" if cnt > 1 else ""
            if m:
                short = m[0].split(" (")[0].split(" /")[0]
                txt = (txt + " · " if txt else "") + (short if m[2] else short.lower() + "*")
            n.item.setText(1, txt)

    def show_mass(self):
        if self.mode == "2d" or not self.nodes:
            return
        sel = self.selected_nodes() or [self.ais_node[o] for o in self.view.selected_objects() if o in self.ais_node]
        lv = self._unique_leaves(sel or self.roots)
        QApplication.setOverrideCursor(Qt.WaitCursor)
        try:
            d = self.mass_props(lv)
        finally:
            QApplication.restoreOverrideCursor()
        rows = [("<b>Hmotnostní vlastnosti</b>", ""),
                ("Rozsah", f"{len(sel)} vybraných položek, {len(lv)} dílů" if sel else f"celý model, {len(lv)} dílů")]
        rows += self._mass_rows(d)
        rows.append(("", Html("<a href='mat:all'>Materiály…</a>")))
        self._show_rows(rows)
        self.view.clear_overlay()
        if d["cog"] is not None:
            self.view.show_point(d["cog"], "  T  " + fmt_num(d["mass"], 3) + " kg")
        elif d["volume"] > 0:
            self.view.show_point(d["vcog"], "  T (objem)")
        self.statusBar().showMessage("Těžiště zobrazeno modrou tečkou (T). Esc nebo nový výběr = skrýt", 5000)

    @staticmethod
    def _rows_html(rows):
        """rows: (label, value); labels are our own HTML, values are escaped unless Html(...)."""
        return "<table cellspacing=2>" + "".join(
            f"<tr><td style='color:#8a919c;padding-right:8px'>{k}{':' if v != '' else ''}</td><td>{esc(v)}</td></tr>"
            for k, v in rows) + "</table>"

    def _show_rows(self, rows):
        self.props.setHtml(self._rows_html(rows))

    def on_props_link(self, url):
        s = url.toString()
        if s.startswith("mat:"):
            if s != "mat:all":
                n = self.nodes[int(s[4:])]
                self._sync = True
                self.tree.clearSelection()
                n.item.setSelected(True)
                self._sync = False
            self.assign_material()
            return
        if s.startswith("calc:"):
            n = self.nodes[int(s[5:])]
            QApplication.setOverrideCursor(Qt.WaitCursor)
            try:
                self.mass_props(self.leaves(n))
            finally:
                QApplication.restoreOverrideCursor()
            self.show_props(n)

    # ------------------------------------------------------------------ selection sync
    def selected_nodes(self):
        return [self.node_of(it) for it in self.tree.selectedItems()]

    def on_tree_selection(self):
        if self._sync:
            return
        if self.mode == "2d":
            keys = self.selected_layers()
            if len(keys) == 1 and self.ddoc is not None:
                ents = [e for e in self.ddoc.layouts.get(self.d_layout)
                        if view2d.layer_of(e).lower() == keys[0]]
                types = collections.Counter(e.dxftype() for e in ents)
                lay = self.ddoc.layers.get(keys[0]) if self.ddoc.layers.has_entry(keys[0]) else None
                rows = [("Hladina", lay.dxf.name if lay else keys[0]), ("Entit v layoutu", str(len(ents)))]
                if lay is not None:
                    rows += [("Barva (ACI)", str(lay.color)), ("Typ čáry", lay.dxf.get("linetype", "")),
                             ("Stav ve výkresu", ("zapnutá" if lay.is_on() else "vypnutá") +
                              (", zmrazená" if lay.is_frozen() else "") + (", zamčená" if lay.is_locked() else ""))]
                if types:
                    rows.append(("Typy", ", ".join(f"{t} {n}" for t, n in types.most_common(8))))
                self._show_rows(rows)
            elif not keys:
                self.show_drawing_info()
            return
        ctx = self.view.ctx
        if self.collide_dlg is None:
            self.view.clear_overlay()            # centre of gravity of the previous selection
        ctx.ClearSelected(False)
        sel = self.selected_nodes()
        seen = set()
        for n in sel:
            for lf in self.leaves(n):
                if lf.idx in seen or lf.ais is None or not ctx.IsDisplayed(lf.ais):
                    continue
                seen.add(lf.idx)
                ctx.AddOrRemoveSelected(lf.ais, False)
        ctx.UpdateCurrentViewer()
        if len(sel) == 1:
            self.show_props(sel[0])
        elif not sel:
            self.show_model_info()
        else:
            lv = {lf.idx: lf for n in sel for lf in self.leaves(n)}
            rows = [("Vybráno", f"{len(sel)} položek, {len(lv)} dílů")]
            box = self.nodes_box(sel)
            if box is not None:
                x0, y0, z0, x1, y1, z1 = core.box_limits(box)
                rows.append(("Rozměr X×Y×Z", Html(f"<b>{fmt_len(x1 - x0)} × {fmt_len(y1 - y0)} × {fmt_len(z1 - z0)} mm</b>")))
            self._show_rows(rows)

    def on_view_selection(self):
        objs = self.view.selected_objects()
        nodes = [self.ais_node[o] for o in objs if o in self.ais_node]
        self._sync = True
        self.tree.clearSelection()
        first = None
        for n in nodes:
            it = n.item
            it.setSelected(True)
            p = it.parent()
            while p is not None:
                p.setExpanded(True)
                p = p.parent()
            first = first or it
        if first is not None:
            self.tree.scrollToItem(first)
        self._sync = False
        if len(nodes) == 1:
            self.show_props(nodes[0])
        elif not nodes:
            self.show_model_info()
        else:
            self.on_tree_selection()

    def clear_selection(self):
        if getattr(self, "_measure_kind", None):
            self.set_measure(None)
            return
        self.tree.clearSelection()
        if self.mode == "2d":
            self.view2d.clear_selection()
            self.show_drawing_info()
            return
        if self.view is None:
            return
        if self.collide_dlg is None:
            self.view.clear_overlay()
        self.view.ctx.ClearSelected(True)

    # ------------------------------------------------------------------ visibility
    def on_item_changed(self, item, col):
        if col != 0:
            return
        if self.mode == "2d":
            self.d_vis[item.data(0, Qt.UserRole)] = item.checkState(0) == Qt.Checked
            self._rerender.start(0 if self.view2d.layer_map is not None else 150)   # redraw: debounce clicks
            return
        n = self.node_of(item)
        if n.is_asm:
            return
        ctx = self.view.ctx
        vis = item.checkState(0) == Qt.Checked
        if vis and not ctx.IsDisplayed(n.ais):
            ctx.Display(n.ais, False)
        elif not vis and ctx.IsDisplayed(n.ais):
            ctx.Erase(n.ais, False)
        if not self._vis_dirty:
            self._vis_dirty = True
            QTimer.singleShot(0, self._flush_vis)

    def _flush_vis(self):
        self._vis_dirty = False
        self.view.ctx.UpdateCurrentViewer()

    def set_visible(self, nodes, vis):
        state = Qt.Checked if vis else Qt.Unchecked
        for n in nodes:
            n.item.setCheckState(0, state)

    def hide_selected(self):
        if self.mode == "2d":
            keys = self.selected_layers()
            if keys:
                self.set_layers_visible(set(keys), False)
            return
        if self.view is None:
            return
        sel = self.selected_nodes() or [self.ais_node[o] for o in self.view.selected_objects() if o in self.ais_node]
        self.tree.clearSelection()
        self.view.ctx.ClearSelected(False)
        self.set_visible(sel, False)

    def isolate_selected(self):
        if self.mode == "2d":
            keys = self.selected_layers()
            if keys:
                self.set_layers_visible(set(keys), True, others=False)
            return
        sel = self.selected_nodes()
        if not sel:
            return
        self.set_visible(self.roots, False)
        self.set_visible(sel, True)
        self.on_tree_selection()
        self.zoom_selected()

    def show_all(self):
        if self.mode == "2d":
            self.set_layers_visible(set(), True, others=True)
            return
        self.set_visible(self.roots, True)

    def _own_presentation(self, lf):
        """A shared instance (AIS_ConnectedInteractive) can't have its own transparency – replace it by a
        standalone presentation of the leaf. Returns the leaf's (possibly new) AIS object."""
        old = lf.ais
        if not isinstance(old, AIS_ConnectedInteractive):
            return old
        ctx = self.view.ctx
        o = standalone_object(lf)
        self._style(o)
        o.SetDisplayMode(AIS_WireFrame if self.a_wire.isChecked() else AIS_Shaded)
        shown = ctx.IsDisplayed(old)
        ctx.Remove(old, False)
        del self.ais_node[old]
        lf.ais = o
        self.ais_node[o] = lf
        if shown:
            ctx.Display(o, o.DisplayMode(), 0, False)
        snap = self.view._snap_objs
        if old in snap:                          # measuring: keep the pick modes on the new object
            snap[snap.index(old)] = o
            self.view.activate_snap(o)
        return o

    def toggle_transp_selected(self):
        if self.mode == "2d" or self.view is None:
            return
        ctx = self.view.ctx
        lv = {lf.idx: lf for n in self.selected_nodes() for lf in self.leaves(n)}
        if not lv:
            return
        on = not any(lf.ais.Transparency() > 0 for lf in lv.values())
        for lf in lv.values():
            if on:
                ctx.SetTransparency(self._own_presentation(lf), 0.75, False)
            else:
                ctx.UnsetTransparency(lf.ais, False)
        self.tree.clearSelection()
        ctx.ClearSelected(False)
        ctx.UpdateCurrentViewer()

    def zoom_selected(self):
        if self.mode == "2d":
            items = [it for it, _ in self.view2d._sel_items]
            if items:
                self.view2d.zoom_to_items(items)
            return
        sel = self.selected_nodes()
        if sel and self.view is not None:
            self.view.fit_box(self.nodes_box(sel))

    def collapse_tree(self):
        self.tree.collapseAll()
        for i in range(self.tree.topLevelItemCount()):
            self.tree.topLevelItem(i).setExpanded(True)

    def show_context_menu(self, gpos):
        m = QMenu(self)
        if self.mode == "3d" and getattr(self, "_measure_kind", None) == "dist":
            m.addAction("Ponechat měření a měřit dál  (Enter)", self.view.pin_measure)
            m.addAction(self.a_copy_measure)
            m.addAction("Vyčistit výběr", self.view.clear_picked)
            m.addAction("Smazat vše a ukončit měření  (Esc)", lambda: self.set_measure(None))
            m.exec(gpos)
            return
        if self.mode == "2d":
            has = bool(self.selected_layers())
            for text, slot in (("Skrýt hladinu", self.hide_selected), ("Jen tato hladina", self.isolate_selected)):
                a = m.addAction(text, slot)
                a.setEnabled(has)
            m.addAction("Zobrazit všechny hladiny", self.show_all)
            m.addSeparator()
            m.addAction(self.a_zoomsel)
            m.addAction(self.a_fit)
            m.exec(gpos)
            return
        has = bool(self.selected_nodes())
        for a in (self.a_zoomsel, self.a_hide, self.a_iso, self.a_transp):
            a.setEnabled(has)
            m.addAction(a)
        m.addAction(self.a_all)
        m.addSeparator()
        m.addAction(self.a_material)
        m.addAction(self.a_mass)
        m.addAction(self.a_collide)
        m.addSeparator()
        if has:
            cp = m.addAction("Kopírovat název")
            cp.triggered.connect(lambda: QGuiApplication.clipboard().setText(
                "\n".join(n.name for n in self.selected_nodes())))
        m.addAction(self.a_expand)
        m.addAction(self.a_collapse)
        m.addSeparator()
        m.addAction(self.a_fit)
        m.exec(gpos)
        for a in (self.a_zoomsel, self.a_hide, self.a_iso, self.a_transp):
            a.setEnabled(True)

    # ------------------------------------------------------------------ filter
    def apply_filter(self, text):
        text = text.strip().lower()
        self.tree.setUpdatesEnabled(False)

        def rec(it):
            child_match = False
            for i in range(it.childCount()):
                child_match |= rec(it.child(i))
            names = [it.text(0)]
            if self.mode != "2d":
                names.append(self.node_of(it).proto_name or "")
            match = not text or any(text in s.lower() for s in names)
            show = match or child_match
            it.setHidden(not show)
            if text and child_match:
                it.setExpanded(True)
            return show

        for i in range(self.tree.topLevelItemCount()):
            rec(self.tree.topLevelItem(i))
        self.tree.setUpdatesEnabled(True)

    # ------------------------------------------------------------------ display options
    def _redisplay_all(self):
        if self.view is None:
            return
        QApplication.setOverrideCursor(Qt.WaitCursor)
        try:
            ctx = self.view.ctx
            # presentation only - selection structures don't depend on edges (Redisplay: 3.8 s -> 0.3 s on
            # 871 parts); shared prototypes first, then every instance incl. the connected ones (they must be
            # recomputed too, verified pixel-exact against a full Redisplay)
            for p in self.protos.values():
                ctx.RecomputePrsOnly(p, False, False)
            for n in self.nodes:
                if n.ais is not None:
                    ctx.RecomputePrsOnly(n.ais, False, False)
            ctx.UpdateCurrentViewer()
        finally:
            QApplication.restoreOverrideCursor()

    def toggle_edges(self, on):
        if self.view is not None:                 # STL sharp-edge lines follow the edges button
            self.view.show_features(on)
        for p in self.protos.values():
            self.view.apply_edge_style(p, on)
        for n in self.nodes:
            if n.ais is not None and not isinstance(n.ais, AIS_ConnectedInteractive):
                self.view.apply_edge_style(n.ais, on)
        self._redisplay_all()

    def toggle_wire(self, on):
        if self.view is None:
            return
        ctx = self.view.ctx
        for n in self.nodes:
            if n.ais is not None:
                ctx.SetDisplayMode(n.ais, AIS_WireFrame if on else AIS_Shaded, False)
        self.view.set_dark_background(on)
        ctx.UpdateCurrentViewer()

    def set_measure(self, kind):
        """kind: None / "dist" / "angle" – the two toolbar toggles are mutually exclusive."""
        if getattr(self, "_measure_kind", None) == kind:
            return
        self._measure_kind = kind
        for a, k in ((self.a_measure, "dist"), (self.a_angle, "angle")):
            a.blockSignals(True)
            a.setChecked(kind == k)
            a.blockSignals(False)
        if self.mode == "2d":
            self.view2d.set_measure_mode(kind)
        elif self.view is not None:
            # 3D "Měřit" = pick vertices / edges / faces (Onshape-like); sub-shape picking is prepared
            # lazily per part under the cursor (all 871 parts at once: 2.2 s and +165 MB)
            snap = [n.ais for n in self.nodes if n.ais is not None] if kind else ()
            self.view.set_measure_mode("pick" if kind == "dist" else kind, snap)
            if kind == "dist":
                self.on_entities([])
        if kind == "dist" and self.mode != "2d":
            self.st_model.setText("Měřit: klikej na vrcholy, hrany a plochy (další klik přidá, opakovaný odebere, "
                                  "prázdno = vyčistit), Shift+klik = bod na ploše, Enter = ponechat, Ctrl+C = kopírovat, "
                                  "Esc = konec")
        elif kind == "dist":
            self.st_model.setText("Vzdálenost: klikni na 2 body – přichytí se k vrcholům / bodům geometrie" +
                                  ", Shift = vodorovně / svisle.  Měření zůstávají, Esc = smazat a konec")
        elif kind == "angle":
            self.st_model.setText("Úhel: klikni na konec 1. ramene, vrchol a konec 2. ramene.  Esc = smazat a konec")
        else:
            self.st_coord.setText("")
            self.update_status()

    def describe_item(self, it):
        """Hover preview of the measure tool."""
        return analysis.Entity.from_item(it).short()

    def on_hover_text(self, text):
        if self.mode == "3d" and getattr(self, "_measure_kind", None) == "dist":
            self.st_coord.setText(text)

    def copy_measure(self):
        rows = getattr(self, "_measure_rows", None)
        if not rows or getattr(self, "_measure_kind", None) is None:
            return
        import re
        txt = "\n".join(f"{re.sub('<[^>]+>', '', str(k))}\t{re.sub('<[^>]+>', '', str(v))}".strip()
                        for k, v in rows if str(k) or str(v))
        QGuiApplication.clipboard().setText(txt)
        self.statusBar().showMessage("Výsledek měření zkopírován do schránky", 3000)

    def on_entities(self, items):
        """Measure tool: properties of 1 entity, relation of 2 (min distance + ΔX/ΔY/ΔZ, centres, angle),
        totals of 3+."""
        self._measure_rows = None
        if not items:
            is_mesh = bool(self.view is not None and self.view.meshes)
            self._show_rows([("<b>Měření</b>", ""),
                             ("", "Klikni na vrchol, hranu nebo plochu. Druhý klik přidá další entitu – "
                                  "ukáže se vzdálenost (nejmenší i ΔX/ΔY/ΔZ v globálních souřadnicích), "
                                  "vzdálenost středů / os, tloušťka a úhel. Více ploch / hran = součet ploch / délek."),
                             ("", "Shift+klik = přesný bod na ploše (s normálou). Enter = ponechat měření a měřit "
                                  "dál, Ctrl+C = zkopírovat výsledek, Esc = konec.")] +
                            ([("STL", "klik na síť = bod (přichytí se k vrcholu, modrá tečka); na rovinné ploše "
                                      "ukáže i její plochu v mm². Klik na ostrou hranu = obrys, okraj otvoru dá Ø "
                                      "a střed. Dva body na protilehlých stěnách = tloušťka. Shift+klik = jen bod "
                                      "(bez přichycení k hraně).")] if is_mesh else []))
            return
        try:
            ents = [analysis.Entity.from_item(it) for it in items]
        except Exception as ex:  # noqa: BLE001 – unusual geometry
            self._show_rows([("Měření", f"nelze vyhodnotit: {ex}")])
            return
        rows = []
        if len(ents) == 1:
            rows = ents[0].rows()
            e = ents[0]
            p, txt = e.label_pos()
            if p is not None:
                self.view.label_at(p, txt)
            if e.kind == "edge":
                self.st_model.setText(f"{e.type}: délka {fmt_len(e.length)} mm" +
                                      (f", Ø {fmt_len(2 * e.radius)} / R {fmt_len(e.radius)} mm" if e.radius else ""))
            elif e.kind == "face":
                self.st_model.setText(f"{e.type}: plocha {fmt_len(e.area)} mm²" +
                                      (f", Ø {fmt_len(2 * e.radius)} / R {fmt_len(e.radius)} mm" if e.radius else ""))
        elif len(ents) == 2:
            rel, draw = analysis.relation(*ents)
            for i, e in enumerate(ents, 1):
                key = {"edge": ("Délka", e.length), "face": ("Plocha", e.area)}.get(e.kind)
                desc = f"{e.kind_cz()} – {e.type}"
                if e.radius is not None:
                    desc += f", Ø {fmt_len(2 * e.radius)}"
                if key:
                    desc += f", {key[0].lower()} {fmt_len(key[1])}"
                rows.append((f"Entita {i}", desc))
            rows += [(Html(k) if "<" in k else k, Html(v) if "<" in v else v) for k, v in rel]
            label = None
            if "min" in draw:
                p1, p2 = draw["min"]
                label = f"{p1.Distance(p2):.3f} mm".replace(".", ",")
                dv = (p2.X() - p1.X(), p2.Y() - p1.Y(), p2.Z() - p1.Z())
                self.st_model.setText(f"Vzdálenost {fmt_len(p1.Distance(p2))} mm  (ΔX {fmt_len(dv[0])}, "
                                      f"ΔY {fmt_len(dv[1])}, ΔZ {fmt_len(dv[2])})")
            self.view.draw_relation(draw, label)
        else:
            rows = analysis.summary(ents)
            for i, e in enumerate(ents[:12], 1):
                rows.append((f"{i}.", f"{e.kind_cz()} – {e.type}"))
        self._measure_rows = rows
        self._show_rows([("<b>Měření</b>", "")] + rows +
                        [("", Html("<span style='color:#8a919c'>Enter = ponechat · Ctrl+C = kopírovat</span>"))])

    def on_angle(self, deg):
        self.st_model.setText(f"Úhel: {fmt_len(deg)}°   – další 3 kliky = nové měření, Esc = konec")

    def on_scale_bar(self, on):
        self.settings.setValue("scale_bar", on)
        if self.view is not None:
            self.view.set_scale_bar(on)

    def on_rulers(self, on):
        self.settings.setValue("rulers", on)
        self.panel2d.set_rulers(on)

    def on_measured(self, p1, p2):
        d = p1.Distance(p2)
        self.st_model.setText(
            f"Vzdálenost: {fmt_len(d)} mm   (ΔX {fmt_len(abs(p2.X() - p1.X()))}, ΔY {fmt_len(abs(p2.Y() - p1.Y()))}, "
            f"ΔZ {fmt_len(abs(p2.Z() - p1.Z()))})   – další klik = nové měření, Esc = konec")

    def update_section_range(self, redraw=True):
        self.update_section(redraw=redraw)

    def update_section(self, *_, redraw=True):
        ax = self.sec_axis.currentIndex() - 1
        on = ax >= 0 and self.model_box is not None
        self.sec_slider.setEnabled(on)
        self.sec_flip.setEnabled(on)
        if not on:
            if self.view is not None:
                self.view.set_section(None, 0, redraw=redraw)
            return
        lim = core.box_limits(self.model_box)
        lo, hi = lim[ax], lim[ax + 3]
        pos = lo + (hi - lo) * self.sec_slider.value() / 1000.0
        self.view.set_section(ax, pos, self.sec_flip.isChecked(), redraw=redraw)
        self.statusBar().showMessage(f"Řez {'XYZ'[ax]} = {fmt_len(pos)} mm", 2000)

    def screenshot(self):
        d = self.settings.value("lastdir", "")
        base = os.path.splitext(os.path.basename(self.path or "pohled"))[0]
        path, _ = QFileDialog.getSaveFileName(self, "Uložit snímek", os.path.join(d, base + ".png"), "PNG (*.png)")
        if path and self.mode in ("2d", "3d"):
            ok = self.view2d.grab().save(path) if self.mode == "2d" else self.view.screenshot(path)
            self.statusBar().showMessage(("Uloženo: " + path) if ok else "Snímek se nepodařilo uložit", 4000)

    def about(self):
        QMessageBox.about(self, APP_NAME,
                          f"<h3>{APP_NAME} {VERSION}</h3>"
                          "<p>Prohlížení STEP / IGES / STL modelů a DXF / DWG výkresů.<br>"
                          "Jádro Open CASCADE (OCCT), ezdxf, GUI Qt.</p>"
                          "<p><b>Myš (jako Onshape):</b> pravé + tažení = otáčení kolem bodu pod kurzorem,<br>"
                          "prostřední nebo Ctrl+pravé = posun, kolečko = zoom, šipky = 15°, Shift+šipky = 90°.<br>"
                          "Klik = výběr, Ctrl+klik = přidat, dvojklik = přiblížit, pravé (klik) = menu.<br>"
                          "Klik na kostku vpravo nahoře = natočení pohledu.</p>"
                          "<p><b>Měřit (M):</b> klikej na vrcholy, hrany a plochy (opakovaný klik odebere, klik do "
                          "prázdna vyčistí). 1 entita = délka, Ø / R, úhel oblouku, plocha, normála / osa. "
                          "2 entity = nejmenší vzdálenost + ΔX / ΔY / ΔZ v globálních souřadnicích (barevně X / Y / Z), "
                          "vzdálenost středů a os, vzdálenost rovin, úhel. Více = součet délek / ploch.<br>"
                          "<b>Materiál…</b> (pravé menu) přiřadí hustotu dílům (pamatuje se k souboru), "
                          "<b>Hmotnost (G)</b> = hmotnost, objem a těžiště výběru / modelu, "
                          "<b>Kolize (K)</b> = průniky mezi díly (vybranými, jinak všemi viditelnými).</p>"
                          "<p><b>Klávesy:</b><br>"
                          "F = celý model, Z = na výběr, pohledy 0–6 nebo jako Onshape Shift+1 přední, Shift+2 zadní, "
                          "Shift+3 levý, Shift+4 pravý, Shift+5 horní, Shift+6 spodní, Shift+7 izometrie<br>"
                          "H = skrýt, I = jen vybrané, A = zobrazit vše, T = průhledné<br>"
                          "E = hrany, W = drátově, P = perspektiva, M = měřit, U = úhel 3 body, G = hmotnost, "
                          "K = kolize, Esc = zrušit<br>"
                          "Ctrl+O = otevřít, F5 = znovu načíst, Ctrl+S = snímek</p>"
                          "<p>Rozměry jsou obalový kvádr v souřadnicích modelu (mm). Bez přiřazeného materiálu "
                          "se ukazuje jen odhad hmotnosti pro ocel / hliník.</p>")

    # ------------------------------------------------------------------ window events
    def dragEnterEvent(self, e):
        if e.mimeData().hasUrls():
            for u in e.mimeData().urls():
                ext = os.path.splitext(u.toLocalFile())[1].lower()
                if ext in ALL_EXT:
                    e.acceptProposedAction()
                    if ext in CAD_EXT:
                        self.prestart_worker()
                    else:
                        prewarm_ezdxf()
                    return

    def dropEvent(self, e):
        for u in e.mimeData().urls():
            p = u.toLocalFile()
            if os.path.splitext(p)[1].lower() in ALL_EXT:
                self.open_file(p)
                return

    def closeEvent(self, e):
        loader = self.d_loader                  # DWG conversion / DXF parsing thread
        if self._proc is not None or loader is not None:
            self.cancel_load()
        self._drop_warm()
        self.settings.setValue("geometry", self.saveGeometry())
        self.settings.setValue("split", self.split.saveState())
        self.settings.sync()
        super().closeEvent(e)
        if loader is not None and loader.isRunning() and not loader.wait(1500):
            # a Python thread can't be stopped; destroying a running QThread aborts the process with a
            # crash dialog - the settings are saved, so just leave
            os._exit(0)


def main():
    if sys.platform.startswith("linux") and "QT_QPA_PLATFORM" not in os.environ:
        os.environ["QT_QPA_PLATFORM"] = "xcb"     # OCCT renders into a native X11 window (XWayland on Wayland)
    app = QApplication(sys.argv)
    if sys.platform.startswith("linux"):
        app.setDesktopFileName("cad-viewer")      # taskbar icon / window grouping
    app.setApplicationName(APP_NAME)

    def excepthook(t, v, tb):
        msg = "".join(traceback.format_exception(t, v, tb))
        if sys.stderr is not None:
            sys.stderr.write(msg)
        QApplication.restoreOverrideCursor()
        msg_critical(None, APP_NAME, "Neočekávaná chyba:\n\n" + msg[-1800:])

    sys.excepthook = excepthook
    w = MainWindow()
    QTimer.singleShot(5000, prune_cache)           # stale temp files of interrupted loads
    w.show()
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if args:
        QTimer.singleShot(50, lambda: w.open_file(args[0]))
    shot = os.environ.get("STEPVIEW_SHOT")   # testing: save a screenshot and quit
    if shot:
        def _shot():
            if w._proc is not None or w.st_prog.isVisible():
                QTimer.singleShot(500, _shot)
                return
            pick = os.environ.get("STEPVIEW_SELECT")
            if pick and not getattr(w, "_picked", False):
                w._picked = True
                for n in w.nodes:
                    if pick.lower() in n.name.lower():
                        w.tree.setCurrentItem(n.item)
                        break
                QTimer.singleShot(800, _shot)
                return
            g = w.frameGeometry()
            w.screen().grabWindow(0, g.x(), g.y(), g.width(), g.height()).save(shot)
            app.quit()
        QTimer.singleShot(int(os.environ.get("STEPVIEW_SHOT_DELAY", "4000")), _shot)
    try:
        sys.exit(app.exec())
    except Exception:  # noqa: BLE001
        traceback.print_exc()


if __name__ == "__main__":
    main()
