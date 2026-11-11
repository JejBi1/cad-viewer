"""3Dconnexion 3D mouse (SpaceMouse, SpaceNavigator, SpacePilot ...) on Windows via Raw Input.

No 3Dconnexion SDK: every HID multi-axis controller (usage page 1, usage 8) sends its reports as WM_INPUT to the
main window while the program is in the foreground. The 3DxWare driver keeps running next to it - its keyboard /
mouse emulator is a separate HID joystick (usage 4) and is not read.
HID reports: 1 = translation X, Y, Z as int16 (newer devices append the rotation -> all 6 axes in one report),
2 = rotation, 3 = buttons. HID axes: X right, Y towards the user, Z down. They are converted to the usual screen
frame X right, Y up, Z towards the user (rotations alike, right-handed), and the views move the model like the cap
("object mode", the 3Dconnexion default).
The device reports only when the values change, so the last state holds until a zero report arrives; a timer
turns it into a velocity (frame-rate independent).
"""
import ctypes
import html
import math
import struct
import sys
import time
from ctypes import wintypes

from PySide6.QtCore import QObject, Qt, QTimer, Signal
from PySide6.QtWidgets import (QCheckBox, QDialog, QDialogButtonBox, QGridLayout, QGroupBox, QHBoxLayout, QLabel,
                               QProgressBar, QSlider, QVBoxLayout)

FULL_SCALE = 350.0          # full deflection of the cap (int16 units)
DEAD_ZONE = 0.04
CURVE = 1.6                 # response exponent: fine control near the rest position
WM_INPUT = 0x00FF
RID_INPUT = 0x10000003
RIDI_DEVICEINFO = 0x2000000B
RIDI_DEVICENAME = 0x20000007
RIM_TYPEHID = 2

PRODUCTS = {0xC603: "SpaceMouse Plus", 0xC621: "SpaceBall 5000", 0xC623: "SpaceTraveler",
            0xC625: "SpacePilot", 0xC626: "SpaceNavigator", 0xC627: "SpaceExplorer",
            0xC628: "SpaceNavigator for Notebooks", 0xC629: "SpacePilot Pro", 0xC62B: "SpaceMouse Pro",
            0xC62E: "SpaceMouse Wireless", 0xC62F: "SpaceMouse Wireless", 0xC631: "SpaceMouse Pro Wireless",
            0xC632: "SpaceMouse Pro Wireless", 0xC633: "SpaceMouse Enterprise", 0xC635: "SpaceMouse Compact",
            0xC636: "SpaceMouse Module", 0xC652: "3Dconnexion Universal Receiver"}
# Devices whose button bits are the 3Dconnexion "virtual keys" (bit = key - 1): Menu, Fit, Top, Left, Right,
# Front, Bottom, Back, Roll CW, Roll CCW, ISO1, ISO2 ... (2-button devices: left = Menu, right = Fit).
VK_LAYOUT = {0xC626, 0xC628, 0xC62B, 0xC62E, 0xC62F, 0xC631, 0xC632, 0xC633, 0xC635, 0xC652}
BUTTONS = {1: "fit", 2: "top", 3: "left", 4: "right", 5: "front", 6: "bottom", 7: "back",
           8: "roll_cw", 9: "roll_ccw", 10: "iso", 11: "iso"}      # bit 0 = Menu: left to the 3DxWare driver
AXES = ("Posun vlevo / vpravo", "Posun nahoru / dolů", "Zoom (tah k sobě / od sebe)",
        "Klopení (vpřed / vzad)", "Otáčení (kroucení)", "Naklápění (do stran)")


def available():
    return sys.platform == "win32"


# ------------------------------------------------------------------ Win32 raw input
class RAWINPUTDEVICE(ctypes.Structure):
    _fields_ = [("usUsagePage", wintypes.USHORT), ("usUsage", wintypes.USHORT),
                ("dwFlags", wintypes.DWORD), ("hwndTarget", wintypes.HWND)]


class RAWINPUTHEADER(ctypes.Structure):
    _fields_ = [("dwType", wintypes.DWORD), ("dwSize", wintypes.DWORD),
                ("hDevice", wintypes.HANDLE), ("wParam", wintypes.WPARAM)]


class RAWINPUTDEVICELIST(ctypes.Structure):
    _fields_ = [("hDevice", wintypes.HANDLE), ("dwType", wintypes.DWORD)]


class RID_DEVICE_INFO_HID(ctypes.Structure):
    _fields_ = [("dwVendorId", wintypes.DWORD), ("dwProductId", wintypes.DWORD), ("dwVersionNumber", wintypes.DWORD),
                ("usUsagePage", wintypes.USHORT), ("usUsage", wintypes.USHORT)]


class RID_DEVICE_INFO(ctypes.Structure):
    _fields_ = [("cbSize", wintypes.DWORD), ("dwType", wintypes.DWORD), ("hid", RID_DEVICE_INFO_HID),
                ("_pad", ctypes.c_byte * 8)]        # the union is as big as the keyboard info (24 bytes)


_u32 = None


def _user32():
    global _u32
    if _u32 is None:
        u = ctypes.WinDLL("user32", use_last_error=True)
        u.RegisterRawInputDevices.argtypes = [ctypes.POINTER(RAWINPUTDEVICE), wintypes.UINT, wintypes.UINT]
        u.RegisterRawInputDevices.restype = wintypes.BOOL
        u.GetRawInputData.argtypes = [wintypes.HANDLE, wintypes.UINT, ctypes.c_void_p,
                                      ctypes.POINTER(wintypes.UINT), wintypes.UINT]
        u.GetRawInputData.restype = wintypes.UINT
        u.GetRawInputDeviceList.argtypes = [ctypes.c_void_p, ctypes.POINTER(wintypes.UINT), wintypes.UINT]
        u.GetRawInputDeviceList.restype = wintypes.UINT
        u.GetRawInputDeviceInfoW.argtypes = [wintypes.HANDLE, wintypes.UINT, ctypes.c_void_p,
                                             ctypes.POINTER(wintypes.UINT)]
        u.GetRawInputDeviceInfoW.restype = wintypes.UINT
        _u32 = u
    return _u32


def _device_info(h):
    info = RID_DEVICE_INFO()
    info.cbSize = ctypes.sizeof(info)
    sz = wintypes.UINT(info.cbSize)
    if _user32().GetRawInputDeviceInfoW(h, RIDI_DEVICEINFO, ctypes.byref(info), ctypes.byref(sz)) in (0, 0xFFFFFFFF):
        return None
    return info.hid


def devices():
    """Connected multi-axis controllers: [(vendor id, product id, name)]."""
    u = _user32()
    n = wintypes.UINT(0)
    if u.GetRawInputDeviceList(None, ctypes.byref(n), ctypes.sizeof(RAWINPUTDEVICELIST)) == 0xFFFFFFFF:
        return []
    arr = (RAWINPUTDEVICELIST * max(n.value, 1))()
    cnt = u.GetRawInputDeviceList(arr, ctypes.byref(n), ctypes.sizeof(RAWINPUTDEVICELIST))
    out = []
    for d in arr[:cnt if cnt != 0xFFFFFFFF else 0]:
        if d.dwType != RIM_TYPEHID:
            continue
        h = _device_info(d.hDevice)
        if h is not None and h.usUsagePage == 1 and h.usUsage == 8:
            out.append((h.dwVendorId, h.dwProductId, PRODUCTS.get(h.dwProductId, "3D myš")))
    return out


# ------------------------------------------------------------------ device state -> motion
class SpaceMouse(QObject):
    """motion(v, dt): v = (tx, ty, tz, rx, ry, rz) in -1..1 (shaped, settings applied, times the speed),
    screen frame X right, Y up, Z towards the user; button(action) for the view / fit keys."""
    motion = Signal(object, float)
    button = Signal(str)

    def __init__(self, settings, parent=None):
        super().__init__(parent)
        self.settings = settings
        self.load()
        self._hid = [0] * 6                  # last raw HID values tx, ty, tz, rx, ry, rz
        self._buttons = 0
        self._pid = {}                       # device handle -> product id
        self._last_tick = 0.0
        self.last_active = 0.0               # time of the last motion (the views ignore wheel events meanwhile)
        self.last_button = None
        self.registered = False
        self._timer = QTimer(self)
        self._timer.setInterval(16)
        self._timer.timeout.connect(self._tick)
        self._hdr = ctypes.sizeof(RAWINPUTHEADER)
        self._buf = ctypes.create_string_buffer(256)
        try:
            from PySide6.QtGui import QGuiApplication
            QGuiApplication.instance().applicationStateChanged.connect(self._app_state)
        except Exception:  # noqa: BLE001
            pass

    # ---- settings
    def load(self):
        s = self.settings

        def b(key, default):
            v = s.value(f"spacemouse/{key}", default)
            return v if isinstance(v, bool) else str(v).lower() in ("1", "true", "yes")

        self.enabled = b("enabled", True)
        self.rotate = b("rotate", True)
        self.pan = b("pan", True)
        self.dominant = b("dominant", False)
        self.invert = [b(f"invert{i}", False) for i in range(6)]
        try:
            self.speed = min(max(float(s.value("spacemouse/speed", 1.0)), 0.1), 10.0)
        except (TypeError, ValueError):
            self.speed = 1.0

    def save(self):
        s = self.settings
        for k in ("enabled", "rotate", "pan", "dominant", "speed"):
            s.setValue(f"spacemouse/{k}", getattr(self, k))
        for i, v in enumerate(self.invert):
            s.setValue(f"spacemouse/invert{i}", v)

    # ---- raw input
    def attach(self, hwnd):
        """Ask Windows for the reports of all multi-axis controllers (also ones plugged in later)."""
        rid = RAWINPUTDEVICE(1, 8, 0, hwnd)
        self.registered = bool(_user32().RegisterRawInputDevices(ctypes.byref(rid), 1, ctypes.sizeof(rid)))
        return self.registered

    def handle(self, message):
        """Called with every native message of the main window (address of a MSG)."""
        msg = wintypes.MSG.from_address(message)
        if msg.message != WM_INPUT:
            return
        u = _user32()
        sz = wintypes.UINT(0)
        h = wintypes.HANDLE(msg.lParam & 0xFFFFFFFFFFFFFFFF)
        if u.GetRawInputData(h, RID_INPUT, None, ctypes.byref(sz), self._hdr) != 0 or sz.value == 0:
            return
        if sz.value > len(self._buf):
            self._buf = ctypes.create_string_buffer(sz.value)
        if u.GetRawInputData(h, RID_INPUT, self._buf, ctypes.byref(sz), self._hdr) != sz.value:
            return
        hdr = RAWINPUTHEADER.from_buffer(self._buf)
        if hdr.dwType != RIM_TYPEHID:
            return
        raw = self._buf.raw[self._hdr:sz.value]
        size, count = struct.unpack_from("<II", raw, 0)
        for i in range(count):
            self.report(hdr.hDevice, raw[8 + i * size:8 + (i + 1) * size])

    def report(self, device, data):
        """One HID report of a device (the state is kept also when switched off - the dialog shows it)."""
        if not data:
            return
        rid = data[0]
        if rid == 1 and len(data) >= 7:
            self._hid[0:3] = struct.unpack_from("<hhh", data, 1)
            if len(data) >= 13:                      # 6-axis report (SpaceMouse Wireless / Compact / Pro ...)
                self._hid[3:6] = struct.unpack_from("<hhh", data, 7)
        elif rid == 2 and len(data) >= 7:
            self._hid[3:6] = struct.unpack_from("<hhh", data, 1)
        elif rid == 3:
            bits = int.from_bytes(data[1:], "little")
            pressed = bits & ~self._buttons
            self._buttons = bits
            for bit in range(len(data[1:]) * 8):
                if pressed >> bit & 1:
                    self.last_button = bit + 1
                    act = BUTTONS.get(bit)
                    if act and self.enabled and self._product(device) in VK_LAYOUT:
                        self.button.emit(act)
            return
        else:
            return
        if self.enabled and any(self._hid) and not self._timer.isActive():
            self._last_tick = time.monotonic()
            self._timer.start()

    def _product(self, device):
        key = int(device or 0)
        if key not in self._pid:
            h = _device_info(device) if device else None
            self._pid[key] = h.dwProductId if h is not None else None
        return self._pid[key]

    def levels(self):
        """Current deflection per axis, screen frame, -1..1 (no dead zone / curve) - for the settings dialog."""
        h = self._hid
        return [max(-1.0, min(1.0, x / FULL_SCALE)) for x in (h[0], -h[2], h[1], h[3], -h[5], h[4])]

    def shaped(self):
        v = []
        for i, x in enumerate(self.levels()):
            a = abs(x)
            y = 0.0 if a < DEAD_ZONE else math.copysign(((a - DEAD_ZONE) / (1 - DEAD_ZONE)) ** CURVE, x)
            v.append(-y if self.invert[i] else y)
        if not self.pan:
            v[0:3] = [0.0] * 3
        if not self.rotate:
            v[3:6] = [0.0] * 3
        if self.dominant and any(v):
            k = max(range(6), key=lambda i: abs(v[i]))
            v = [v[i] if i == k else 0.0 for i in range(6)]
        return [x * self.speed for x in v]

    def _tick(self):
        now = time.monotonic()
        dt = min(max(now - self._last_tick, 0.0), 0.1)
        self._last_tick = now
        if not any(self._hid) or not self.enabled:
            self._timer.stop()
            return
        v = self.shaped()
        if any(v):
            self.last_active = now
            self.motion.emit(tuple(v), dt)

    def reset(self):
        self._hid = [0] * 6
        self._buttons = 0
        self._timer.stop()

    def _app_state(self, state):
        if state != Qt.ApplicationActive:          # no WM_INPUT in the background -> the zero report gets lost
            self.reset()


# ------------------------------------------------------------------ settings dialog
class SpaceMouseDialog(QDialog):
    def __init__(self, sm, parent=None):
        super().__init__(parent)
        self.sm = sm
        self.setWindowTitle("3D myš (3Dconnexion)")
        lay = QVBoxLayout(self)
        devs = devices()
        if devs:
            txt = "Připojeno: " + ", ".join(f"<b>{html.escape(str(n), quote=False)}</b> ({v:04X}:{p:04X})" for v, p, n in devs)
        else:
            txt = ("<b>Žádná 3D myš nenalezena.</b> Připoj ji – funguje i připojení za běhu programu.")
        st = QLabel(txt)
        st.setWordWrap(True)
        lay.addWidget(st)
        self.en = QCheckBox("Ovládat pohled 3D myší")
        self.en.setChecked(sm.enabled)
        lay.addWidget(self.en)

        row = QHBoxLayout()
        row.addWidget(QLabel("Rychlost:"))
        self.speed = QSlider(Qt.Horizontal)
        self.speed.setRange(0, 100)                 # 0.25x .. 4x, logarithmic
        self.speed.setValue(int(round(50 + 25 * math.log2(sm.speed))))
        self.speed_lbl = QLabel()
        self.speed.valueChanged.connect(self._speed_text)
        self._speed_text()
        row.addWidget(self.speed, 1)
        row.addWidget(self.speed_lbl)
        lay.addLayout(row)

        self.rot = QCheckBox("Otáčení")
        self.rot.setChecked(sm.rotate)
        self.pan = QCheckBox("Posun a zoom")
        self.pan.setChecked(sm.pan)
        self.dom = QCheckBox("Jen převažující pohyb (dominantní osa)")
        self.dom.setChecked(sm.dominant)
        r2 = QHBoxLayout()
        for w in (self.rot, self.pan, self.dom):
            r2.addWidget(w)
        r2.addStretch(1)
        lay.addLayout(r2)

        box = QGroupBox("Osy – vychýlení čepičky a obrácení směru")
        g = QGridLayout(box)
        self.bars, self.inv = [], []
        for i, name in enumerate(AXES):
            g.addWidget(QLabel(name), i, 0)
            bar = QProgressBar()
            bar.setRange(-100, 100)
            bar.setValue(0)
            bar.setTextVisible(False)
            bar.setFixedWidth(160)
            g.addWidget(bar, i, 1)
            cb = QCheckBox("obrátit")
            cb.setChecked(sm.invert[i])
            g.addWidget(cb, i, 2)
            self.bars.append(bar)
            self.inv.append(cb)
        lay.addWidget(box)
        self.btn_lbl = QLabel("Tlačítko: –")
        lay.addWidget(self.btn_lbl)
        info = QLabel("Model se pohybuje jako čepička (tlač doprava = model doprava, tah k sobě = přiblížit, "
                      "kroucení = otáčení). Tlačítka: Fit = zobrazit vše, na SpaceMouse Pro / Enterprise i pohledy "
                      "(Top, Front, Right, …) a Roll. Levé tlačítko (Menu) obsluhuje ovladač 3DxWare.")
        info.setWordWrap(True)
        lay.addWidget(info)
        bb = QDialogButtonBox(QDialogButtonBox.Ok | QDialogButtonBox.Cancel)
        bb.accepted.connect(self.apply)
        bb.rejected.connect(self.reject)
        lay.addWidget(bb)
        self._t = QTimer(self)
        self._t.setInterval(50)
        self._t.timeout.connect(self._live)
        self._t.start()

    def _speed_text(self):
        self.speed_lbl.setText(f"{2 ** ((self.speed.value() - 50) / 25):.2f}×".replace(".", ","))

    def _live(self):
        for bar, x in zip(self.bars, self.sm.levels()):
            bar.setValue(int(round(100 * x)))
        if self.sm.last_button is not None:
            self.btn_lbl.setText(f"Tlačítko: {self.sm.last_button}")

    def apply(self):
        sm = self.sm
        sm.enabled = self.en.isChecked()
        sm.speed = 2 ** ((self.speed.value() - 50) / 25)
        sm.rotate, sm.pan, sm.dominant = self.rot.isChecked(), self.pan.isChecked(), self.dom.isChecked()
        sm.invert = [cb.isChecked() for cb in self.inv]
        sm.save()
        if not sm.enabled:
            sm.reset()
        self.accept()
