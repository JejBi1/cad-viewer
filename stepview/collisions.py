"""Interference (collision) check dialog."""
import time

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (QApplication, QCheckBox, QDialog, QHBoxLayout, QHeaderView, QLabel, QProgressBar,
                               QPushButton, QTreeWidget, QTreeWidgetItem, QVBoxLayout)

import analysis


def _num(v, d=2):
    return f"{v:,.{d}f}".replace(",", " ").replace(".", ",")


class CollisionDialog(QDialog):
    """parts: [(node, world TopoDS_Shape)]. on_pick(node_a, node_b, common_shape or None) shows a result,
    on_show_all(list of shapes) shows all interference volumes, on_close() cleans up."""

    def __init__(self, parent, parts, scope, on_pick, on_show_all, on_close):
        super().__init__(parent)
        self.setWindowTitle("Kolize dílů")
        self.setModal(False)
        self.resize(640, 460)
        self.parts, self.on_pick, self.on_show_all, self.on_close = parts, on_pick, on_show_all, on_close
        self.results = []
        self._stop = False
        self._running = False
        lay = QVBoxLayout(self)
        lay.addWidget(QLabel(f"Kontrola průniků: {scope} ({len(parts)} dílů)"))
        row = QHBoxLayout()
        self.touch = QCheckBox("hlásit i dotyky (plochy na sobě, bez průniku)")
        row.addWidget(self.touch)
        row.addStretch(1)
        self.run_btn = QPushButton("Spustit")
        self.run_btn.clicked.connect(self.toggle_run)
        row.addWidget(self.run_btn)
        lay.addLayout(row)
        self.prog = QProgressBar()
        self.prog.setVisible(False)
        lay.addWidget(self.prog)
        self.status = QLabel("")
        lay.addWidget(self.status)
        self.list = QTreeWidget()
        self.list.setHeaderLabels(["Díl A", "Díl B", "Typ", "Průnik [mm³]"])
        self.list.setRootIsDecorated(False)
        h = self.list.header()
        h.setSectionResizeMode(0, QHeaderView.Stretch)
        h.setSectionResizeMode(1, QHeaderView.Stretch)
        h.setSectionResizeMode(2, QHeaderView.ResizeToContents)
        h.setSectionResizeMode(3, QHeaderView.ResizeToContents)
        self.list.itemSelectionChanged.connect(self._picked)
        lay.addWidget(self.list, 1)
        bottom = QHBoxLayout()
        self.all_btn = QPushButton("Zobrazit všechny průniky")
        self.all_btn.setEnabled(False)
        self.all_btn.clicked.connect(lambda: self.on_show_all([r[3] for r in self.results if r[3] is not None]))
        bottom.addWidget(self.all_btn)
        bottom.addStretch(1)
        close = QPushButton("Zavřít")
        close.clicked.connect(self.close)
        bottom.addWidget(close)
        lay.addLayout(bottom)

    def toggle_run(self):
        if self._running:
            self._stop = True
            return
        self.run()

    def run(self):
        self._running, self._stop = True, False
        self.run_btn.setText("Zastavit")
        self.touch.setEnabled(False)
        self.list.clear()
        self.results = []
        t0 = time.time()
        self.status.setText("Hledám dvojice těles s překrývajícími se obálkami…")
        QApplication.processEvents()
        # work per SOLID, not per part: multi-body parts (Fusion components) would otherwise run booleans on
        # whole compounds, although usually only one or two bodies of each part actually overlap
        items = []                                   # (part index, solid)
        for pi, (_, s) in enumerate(self.parts):
            for so in analysis.solids_of(s):
                items.append((pi, so))
        boxes = [analysis.shape_box(so, 0.01) for _, so in items]
        pairs = [(i, j) for i, j in analysis.candidate_pairs(boxes) if items[i][0] != items[j][0]]
        part_pairs = {tuple(sorted((items[i][0], items[j][0]))) for i, j in pairs}
        self.prog.setRange(0, max(1, len(pairs)))
        self.prog.setValue(0)
        self.prog.setVisible(True)
        touch = self.touch.isChecked()
        found = {}                                   # (part a, part b) -> [kind, volume, [common shapes]]
        last = 0.0
        for k, (i, j) in enumerate(pairs):
            if self._stop:
                break
            key = tuple(sorted((items[i][0], items[j][0])))
            have = found.get(key)
            kind, vol, common = analysis.interference(items[i][1], items[j][1],
                                                      touch and (have is None))   # one touch per part pair is enough
            if kind:
                if have is None:
                    have = found[key] = [kind, 0.0, []]
                if kind == "kolize":
                    have[0] = "kolize"
                    have[1] += vol
                    have[2].append(common)
            if time.time() - last > 0.1:
                self.prog.setValue(k + 1)
                n = sum(1 for v in found.values() if v[0] == "kolize")
                self.status.setText(f"Tělesa {k + 1} / {len(pairs)}   ·   nalezeno kolizí {n}")
                QApplication.processEvents()
                last = time.time()
        for (a, b), (kind, vol, commons) in found.items():
            common = commons[0] if len(commons) == 1 else (analysis.compound(commons) if commons else None)
            self.results.append((self.parts[a][0], self.parts[b][0], kind, common, vol))
        n_col = sum(1 for r in self.results if r[2] == "kolize")
        n_touch = len(self.results) - n_col
        state = "Zastaveno" if self._stop else "Hotovo"
        self.status.setText(f"{state} za {time.time() - t0:.1f} s: {len(part_pairs)} dvojic dílů / {len(pairs)} "
                            f"dvojic těles s překrytím obálek, "
                            f"<b>{n_col} kolizí</b>" + (f", {n_touch} dotyků" if touch else "") +
                            (". Klikni na řádek = ukázat v modelu." if self.results else "."))
        self.prog.setVisible(False)
        self.run_btn.setText("Spustit znovu")
        self.touch.setEnabled(True)
        self.all_btn.setEnabled(any(r[3] is not None for r in self.results))
        self._running = False
        self._fill_sorted()

    def _fill_sorted(self):
        order = sorted(range(len(self.results)), key=lambda i: -self.results[i][4])
        self.list.clear()
        for i in order:
            na, nb, kind, _, vol = self.results[i]
            it = QTreeWidgetItem([na.name, nb.name, kind, _num(vol) if kind == "kolize" else "–"])
            it.setData(0, Qt.UserRole, i)
            self.list.addTopLevelItem(it)

    def _picked(self):
        items = self.list.selectedItems()
        if not items:
            return
        na, nb, kind, common, vol = self.results[items[0].data(0, Qt.UserRole)]
        self.on_pick(na, nb, common if kind == "kolize" else None)

    def closeEvent(self, e):
        self._stop = True
        self.on_close()
        super().closeEvent(e)
