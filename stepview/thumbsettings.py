"""Windows: per-format on/off switch for the Explorer thumbnail handler (current user, HKCU)."""
import ctypes
import os
import subprocess
import sys

from PySide6.QtWidgets import (QCheckBox, QDialog, QDialogButtonBox, QGroupBox, QHBoxLayout, QLabel,
                               QPushButton, QVBoxLayout)

CLSID = "{7A1F3C52-8E0B-4C6D-9F2A-5B3E1D4C6A70}"
IID_THUMB = "{e357fccd-a995-4576-b01f-234630154e96}"
FORMATS = [("STEP", (".step", ".stp")), ("IGES", (".iges", ".igs")), ("STL", (".stl",)),
           ("DXF", (".dxf",)), ("DWG", (".dwg",))]


def available():
    return sys.platform == "win32"


def _winreg():
    import winreg
    return winreg


def _key(ext):
    return rf"Software\Classes\SystemFileAssociations\{ext}\ShellEx\{IID_THUMB}"


def is_on(ext):
    """True if our handler is registered for ext (current user or machine-wide)."""
    w = _winreg()
    for root in (w.HKEY_CURRENT_USER, w.HKEY_LOCAL_MACHINE):
        try:
            with w.OpenKey(root, _key(ext)) as k:
                if w.QueryValueEx(k, "")[0].lower() == CLSID.lower():
                    return True
        except OSError:
            pass
    return False


def machine_wide(ext):
    w = _winreg()
    try:
        with w.OpenKey(w.HKEY_LOCAL_MACHINE, _key(ext)) as k:
            return w.QueryValueEx(k, "")[0].lower() == CLSID.lower()
    except OSError:
        return False


def _install_roots():
    """Folders where a program is installed by the installer (per user or for everybody)."""
    roots = [os.path.join(os.environ.get("LOCALAPPDATA", ""), "Programs")] if os.environ.get("LOCALAPPDATA") else []
    roots += [os.environ.get(v) for v in ("ProgramFiles", "ProgramFiles(x86)", "ProgramW6432")]
    return {os.path.normcase(os.path.realpath(r)) for r in roots if r}


def installed_dir():
    """Real folder of the running program if it is an installed (frozen) copy under %LOCALAPPDATA%\\Programs or
    Program Files, else None - a portable copy may sit in a synced / shared folder that others can write to."""
    if not getattr(sys, "frozen", False):
        return None
    here = os.path.normcase(os.path.realpath(os.path.dirname(sys.executable)))
    for r in _install_roots():
        if here == r or here.startswith(r.rstrip("\\") + "\\"):
            return os.path.realpath(os.path.dirname(sys.executable))
    return None


def _ensure_clsid():
    """Register the COM server for the current user if the installer did not. Explorer loads the registered DLL
    into its own process, so this is done only for the copy the installer put into Program Files / Local Programs
    (never for a DLL beside a program that runs from a downloaded, synced or shared folder).
    Returns None on success, else the (Czech) reason why thumbnails can't be enabled."""
    w = _winreg()
    for root in (w.HKEY_CURRENT_USER, w.HKEY_LOCAL_MACHINE):
        try:
            w.OpenKey(root, rf"Software\Classes\CLSID\{CLSID}\InprocServer32").Close()
            return None
        except OSError:
            pass
    folder = installed_dir()
    if folder is None:
        return ("Náhledy v Průzkumníku jde zapnout jen z nainstalované verze (instalátor CadProhlizec_setup).\n\n"
                "Tento program neběží ze složky, kam instalátor program ukládá (Program Files nebo "
                "%LOCALAPPDATA%\\Programs), a Průzkumník by načítal knihovnu z umístění, do kterého může zapisovat "
                "i někdo jiný. Nainstaluj program a náhledy zapni v něm.")
    dll = os.path.join(folder, "StepThumb.dll")
    if not os.path.exists(dll):
        return "Chybí StepThumb.dll vedle programu – náhledy nejde zapnout."
    with w.CreateKey(w.HKEY_CURRENT_USER, rf"Software\Classes\CLSID\{CLSID}") as k:
        w.SetValueEx(k, "", 0, w.REG_SZ, "CAD prohlížeč – náhledy")
    with w.CreateKey(w.HKEY_CURRENT_USER, rf"Software\Classes\CLSID\{CLSID}\InprocServer32") as k:
        for name, val in (("", "mscoree.dll"), ("ThreadingModel", "Both"),
                          ("Class", "StepThumb.StepThumbnailProvider"),
                          ("Assembly", "StepThumb, Version=1.3.0.0, Culture=neutral, PublicKeyToken=null"),
                          ("RuntimeVersion", "v4.0.30319"),
                          ("CodeBase", "file:///" + dll.replace("\\", "/"))):
            w.SetValueEx(k, name, 0, w.REG_SZ, val)
    return None


def set_on(ext, on):
    w = _winreg()
    if on:
        with w.CreateKey(w.HKEY_CURRENT_USER, _key(ext)) as k:
            w.SetValueEx(k, "", 0, w.REG_SZ, CLSID)
    else:
        try:
            w.DeleteKey(w.HKEY_CURRENT_USER, _key(ext))
        except OSError:
            pass


def notify_shell():
    try:
        ctypes.windll.shell32.SHChangeNotify(0x08000000, 0, None, None)   # SHCNE_ASSOCCHANGED
    except Exception:  # noqa: BLE001
        pass


class ThumbDialog(QDialog):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setWindowTitle("Náhledy v Průzkumníku")
        lay = QVBoxLayout(self)
        lay.addWidget(QLabel("Zobrazovat v Průzkumníku Windows náhled modelu / výkresu místo ikony u souborů:"))
        box = QGroupBox()
        bl = QVBoxLayout(box)
        self.checks = []
        for name, exts in FORMATS:
            cb = QCheckBox(f"{name}  ({', '.join(exts)})" +
                           ("   – méně bezpečné (převodník LibreDWG)" if name == "DWG" else ""))
            if name == "DWG":
                cb.setToolTip("Náhled spouští převodník LibreDWG na každý zobrazený soubor. Ten je méně odolný "
                              "vůči poškozeným nebo podvrženým souborům – zapínej jen, když pracuješ s DWG "
                              "z důvěryhodných zdrojů. Vykreslování náhledů běží v omezeném procesu (nízká "
                              "integrita, limit paměti).")
            cb.setChecked(any(is_on(e) for e in exts))
            if any(machine_wide(e) for e in exts):
                cb.setEnabled(False)
                cb.setToolTip("Zapnuto instalací pro všechny uživatele – změnit jde jen přeinstalací")
            bl.addWidget(cb)
            self.checks.append((cb, exts))
        lay.addWidget(box)
        info = QLabel("Pozn.: ve složce SynologyDrive náhledy nefungují (Synology tam používá vlastní).<br>"
                      "Už vytvořené náhledy si Windows pamatují – po vypnutí je smaž tlačítkem níže "
                      "(Vyčištění disku → <b>Miniatury</b>).")
        info.setWordWrap(True)
        lay.addWidget(info)
        row = QHBoxLayout()
        clean = QPushButton("Vymazat uložené náhledy…")
        # full path: a bare name is searched in the program's / current folder first (exe planting)
        cleanmgr = os.path.join(os.environ.get("SystemRoot", r"C:\Windows"), "System32", "cleanmgr.exe")
        clean.clicked.connect(lambda: subprocess.Popen([cleanmgr]))
        row.addWidget(clean)
        row.addStretch(1)
        lay.addLayout(row)
        bb = QDialogButtonBox(QDialogButtonBox.Ok | QDialogButtonBox.Cancel)
        bb.accepted.connect(self.apply)
        bb.rejected.connect(self.reject)
        lay.addWidget(bb)

    def apply(self):
        want_any = any(cb.isChecked() and cb.isEnabled() for cb, _ in self.checks)
        problem = _ensure_clsid() if want_any else None
        if problem:
            from PySide6.QtWidgets import QMessageBox
            QMessageBox.warning(self, self.windowTitle(), problem)      # nothing is registered
            return
        for cb, exts in self.checks:
            if cb.isEnabled():
                for e in exts:
                    set_on(e, cb.isChecked())
        notify_shell()
        self.accept()
