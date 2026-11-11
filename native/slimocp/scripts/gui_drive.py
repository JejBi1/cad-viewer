#!/usr/bin/env python3
"""Drive the real MainWindow (Qt + OCCT viewer) like a user: open a STEP, hover/click parts, measure mode with entity picking,
section plane, view cube click, screenshots.  Run under xvfb (QT_QPA_PLATFORM=xcb on Linux).
usage: gui_drive.py <stepview dir> <file> <outdir>"""
import os, sys, time, traceback
SV, F, OUT = [os.path.abspath(a) for a in sys.argv[1:4]]
os.makedirs(OUT, exist_ok=True)
os.environ.setdefault('HOME', OUT)
sys.path.insert(0, SV); os.chdir(SV)
from PySide6.QtCore import Qt, QPoint, QTimer, QEventLoop
from PySide6.QtTest import QTest
from PySide6.QtWidgets import QApplication
import main
app = None
log = []
def pump(sec):
    loop = QEventLoop()
    QTimer.singleShot(int(sec * 1000), loop.quit)
    loop.exec()
def step(name, fn):
    print('>>', name, flush=True)
    try:
        r = fn(); log.append((name, 'ok', r))
    except BaseException:
        log.append((name, 'FAIL', traceback.format_exc(limit=5)))
def run_all(win):
    global w, v
    w = win
    t = time.time()
    while w.doc is None and time.time() - t < 60: pump(0.2)
    pump(1.5)
    v = w.view
    step('model on screen', lambda: 'doc=%s nodes=%d view=%s' % (w.doc is not None, len(w.nodes), v is not None))
    def orient(): v.set_orientation('iso'); v.fit_all(); pump(0.8); return 'ok'
    step('fit_all / iso', orient)
    def shot(name):
        p = os.path.join(OUT, name); v.screenshot(p); return os.path.getsize(p)
    step('screenshot (View.Dump)', lambda: shot('view1.png'))
    def hover_click():
        n = 0; sel = 0
        W, H = v.width(), v.height()
        for fx in (0.35, 0.45, 0.5, 0.55, 0.65):
            for fy in (0.4, 0.5, 0.6):
                pt = QPoint(int(W * fx), int(H * fy))
                QTest.mouseMove(v, pt); pump(0.15)
                QTest.mouseClick(v, Qt.LeftButton, pos=pt); pump(0.15); n += 1
                if len(v.selected_objects()): sel += 1
        return 'clicks=%d with selection=%d' % (n, sel)
    step('hover + click selection (AIS_InteractiveContext)', hover_click)
    step('measure mode: pick entities', lambda: (v.set_measure_mode('pick', list(v.objects) if hasattr(v, 'objects') else ()), pump(0.5), 'mode=%s' % v.measure_mode)[2])
    def pick_entities():
        W, H = v.width(), v.height(); k = 0
        for fx, fy in ((0.5, 0.5), (0.45, 0.5), (0.55, 0.45), (0.5, 0.6), (0.4, 0.55)):
            pt = QPoint(int(W * fx), int(H * fy))
            QTest.mouseMove(v, pt); pump(0.3)
            QTest.mouseClick(v, Qt.LeftButton, pos=pt); pump(0.3); k += 1
        return 'picked=%d' % len(getattr(v, '_picked', []))
    step('measure: click entities', pick_entities)
    step('screenshot measure', lambda: shot('view2.png'))
    step('measure off', lambda: (v.set_measure_mode(None), pump(0.3), 'ok')[2])
    step('section plane (Graphic3d_ClipPlane)', lambda: (v.set_section(0, 0.0), pump(0.6), shot('view3.png'))[2])
    step('section off', lambda: (v.set_section(None, 0.0), pump(0.3), 'ok')[2])
    def cube():
        # click the ViewCube (top-right corner) -> AIS_ViewCubeOwner path
        W, H = v.width(), v.height()
        for fx, fy in ((0.93, 0.13), (0.95, 0.10), (0.92, 0.12)):
            pt = QPoint(int(W * fx), int(H * fy)); QTest.mouseMove(v, pt); pump(0.2); QTest.mouseClick(v, Qt.LeftButton, pos=pt); pump(0.8)
        return 'ok'
    step('view cube click', cube)
    step('wireframe toggle / perspective', lambda: (w.a_wire.setChecked(True), pump(0.5), v.set_perspective(True), pump(0.5), v.set_perspective(False), w.a_wire.setChecked(False), pump(0.3), shot('view4.png'))[-1])
    step('orbit / zoom / wheel', lambda: (v.orbit(0.3, 0.2), v.roll(0.1), v._zoom_at(100, 100, 1.5), pump(0.4), 'ok')[-1])
    print()
    bad = 0
    for n, s, r in log:
        print('%-52s %-4s %s' % (n, s, r if s == 'ok' else '')); 
        if s != 'ok': print(r); bad += 1
    print('FAILED steps:', bad)
    sys.stdout.flush(); os._exit(1 if bad else 0)

_orig_show = main.MainWindow.show
def _show(self):
    _orig_show(self)
    app = QApplication.instance()
    t0 = time.time()
    timer = QTimer(self)
    def poll():                    # start driving only when the model is really on screen (no nested event loop before)
        if (self.doc is not None and not self._showing and self._proc is None and not self.st_prog.isVisible()) or time.time() - t0 > 90:
            timer.stop(); QTimer.singleShot(800, lambda: run_all(self))
    timer.timeout.connect(poll); timer.start(200)      # the real main() opens the file by itself (argv[1]) after 50 ms
main.MainWindow.show = _show
sys.argv = [sys.argv[0], F]
main.main()
