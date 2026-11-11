"""Material library (density) and per-file material assignments."""
import hashlib
import json
import os

from PySide6.QtWidgets import (QComboBox, QDialog, QDialogButtonBox, QDoubleSpinBox, QFormLayout, QLabel,
                               QVBoxLayout)

# name, density g/cm³
LIBRARY = [
    ("Ocel (konstrukční)", 7.85), ("Nerez (1.4301)", 7.90), ("Litina", 7.20), ("Hliník (slitiny)", 2.70),
    ("Mosaz", 8.50), ("Měď", 8.96), ("Bronz", 8.80), ("Titan", 4.51), ("Zinek", 7.14),
    ("PLA", 1.24), ("PETG", 1.27), ("ABS", 1.04), ("ASA", 1.07), ("PA6 / PA12", 1.14), ("POM", 1.41),
    ("PTFE", 2.20), ("PE-HD", 0.95), ("PP", 0.91), ("PVC", 1.40), ("PC / plexi (PMMA)", 1.19),
    ("FR4 (DPS)", 1.85), ("Pryž / silikon", 1.20), ("Sklo", 2.50), ("Dřevo (smrk)", 0.45),
    ("Dřevo (buk)", 0.72), ("MDF / HDF", 0.75), ("Promasil / izolační deska", 0.24),
    ("Minerální vata", 0.10),
]
LIB = dict(LIBRARY)
CUSTOM = "Vlastní hustota…"


def _valid(m):
    """[name, density g/cm³] with a sane density (a damaged / edited file must not break the mass math)."""
    try:
        return (isinstance(m, (list, tuple)) and len(m) == 2 and isinstance(m[0], str)
                and 0.0 < float(m[1]) < 100.0)
    except (TypeError, ValueError):
        return False


def store_path(cache_dir, model_path):
    key = hashlib.sha1(os.path.normcase(os.path.abspath(model_path)).encode("utf-8")).hexdigest()
    return os.path.join(cache_dir, "materials", key + ".json")


class Assignments:
    """Material per part *prototype*, keyed by the part (document) name so it survives re-exports."""

    def __init__(self, cache_dir, model_path):
        self.path = store_path(cache_dir, model_path)
        self.by_name = {}          # proto name -> [material name, density]
        self.default = None        # [name, density] for unassigned parts, or None
        try:
            with open(self.path, encoding="utf-8") as f:
                d = json.load(f)
            parts = d.get("parts", {}) if isinstance(d, dict) else {}
            self.by_name = ({str(k): [v[0], float(v[1])] for k, v in parts.items() if _valid(v)}
                            if isinstance(parts, dict) else {})
            dflt = d.get("default") if isinstance(d, dict) else None
            self.default = [dflt[0], float(dflt[1])] if _valid(dflt) else None
        except (OSError, ValueError, AttributeError, TypeError):
            pass

    def save(self):
        os.makedirs(os.path.dirname(self.path), exist_ok=True)
        with open(self.path, "w", encoding="utf-8") as f:
            json.dump({"parts": self.by_name, "default": self.default}, f, ensure_ascii=False, indent=1)

    def get(self, proto_name, use_default=True):
        """(material name, density g/cm³, explicit?) or None."""
        m = self.by_name.get(proto_name)
        if m:
            return m[0], m[1], True
        if use_default and self.default:
            return self.default[0], self.default[1], False
        return None

    def set(self, proto_names, name, density):
        for p in proto_names:
            if name is None:
                self.by_name.pop(p, None)
            else:
                self.by_name[p] = [name, density]
        self.save()


class MaterialDialog(QDialog):
    def __init__(self, parent, title, current=None, show_default=False, default=None):
        super().__init__(parent)
        self.setWindowTitle(title)
        lay = QVBoxLayout(self)
        form = QFormLayout()
        self.combo = QComboBox()
        self.combo.addItem("(bez materiálu)", None)
        for n, d in LIBRARY:
            self.combo.addItem(f"{n}  –  {str(d).replace('.', ',')} g/cm³", n)
        self.combo.addItem(CUSTOM, CUSTOM)
        self.dens = QDoubleSpinBox()
        self.dens.setDecimals(3)
        self.dens.setRange(0.001, 30.0)
        self.dens.setSuffix(" g/cm³")
        self.dens.setValue(7.85)
        form.addRow("Materiál:", self.combo)
        form.addRow("Hustota:", self.dens)
        lay.addLayout(form)
        self.combo.currentIndexChanged.connect(self._changed)
        if current:
            i = self.combo.findData(current[0])
            if i < 0:
                i = self.combo.findData(CUSTOM)
            self.combo.setCurrentIndex(i)
            self.dens.setValue(current[1])
        self.default_combo = None
        if show_default:
            self.default_combo = QComboBox()
            self.default_combo.addItem("(žádný – nepočítat hmotnost)", None)
            for n, d in LIBRARY:
                self.default_combo.addItem(n, n)
            if default:
                self.default_combo.setCurrentIndex(max(0, self.default_combo.findData(default[0])))
            f2 = QFormLayout()
            f2.addRow("Díly bez materiálu:", self.default_combo)
            lay.addLayout(f2)
            hint = QLabel("Výchozí materiál se použije pro všechny díly, kterým nebyl přiřazen žádný.")
            hint.setWordWrap(True)
            lay.addWidget(hint)
        bb = QDialogButtonBox(QDialogButtonBox.Ok | QDialogButtonBox.Cancel)
        bb.accepted.connect(self.accept)
        bb.rejected.connect(self.reject)
        lay.addWidget(bb)
        self._changed()

    def _changed(self, *_):
        d = self.combo.currentData()
        self.dens.setEnabled(d == CUSTOM)
        if d in LIB:
            self.dens.setValue(LIB[d])

    def result_material(self):
        """(name, density) or (None, None) for 'no material'."""
        d = self.combo.currentData()
        if d is None:
            return None, None
        if d == CUSTOM:
            return f"Vlastní ({self.dens.value():g} g/cm³)".replace(".", ","), self.dens.value()
        return d, LIB[d]

    def result_default(self):
        if self.default_combo is None:
            return None
        d = self.default_combo.currentData()
        return [d, LIB[d]] if d else None
