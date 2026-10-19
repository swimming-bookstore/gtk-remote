#!/usr/bin/env python3
"""Record unmodified LibreOffice Calc filling a sheet.

The grid itself is a VCL widget, so cell writes are `gtk-remote sheet-set`.
That command is the Rust client's UNO bridge (LibreOffice must be started
with `--accept`). The window is still launched under gtk-remote-launch, and
the clip is a ~/demo WindowRecord of the whole window.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

os.environ.setdefault("DISPLAY", ":0.0")
os.environ.setdefault("XAUTHORITY", str(Path.home() / ".Xauthority"))
sys.path.insert(0, str(Path.home() / "demo"))

from record.window import WindowRecord

ROOT = Path(__file__).resolve().parents[1]
LAUNCH = ROOT / "gtk-remote-launch"
CLIENT = ROOT / "target" / "release" / "gtk-remote"
SOFFICE = Path("/usr/lib/libreoffice/program/soffice.bin")
OUT = ROOT / "docs" / "demo-calc-sheet.mp4"
TITLE = "LibreOffice Calc"


def window_title() -> str | None:
    try:
        tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True, stderr=subprocess.DEVNULL)
    except (subprocess.CalledProcessError, OSError):
        return None
    for line in tree.splitlines():
        if TITLE not in line or '"' not in line:
            continue
        name = line.split('"')[1]
        if not name.endswith("Calc"):
            continue
        return name
    return None


def unmaximize(title: str) -> None:
    from ctypes import POINTER, Structure, byref, c_char_p, c_int, c_long, c_uint, c_ulong, c_void_p

    from record import x11cap

    class XClientMessageEvent(Structure):
        _fields_ = [
            ("type", c_int),
            ("serial", c_ulong),
            ("send_event", c_int),
            ("display", c_void_p),
            ("window", c_ulong),
            ("message_type", c_ulong),
            ("format", c_int),
            ("data0", c_long),
            ("data1", c_long),
            ("data2", c_long),
            ("data3", c_long),
            ("data4", c_long),
        ]

    x11 = x11cap.x11
    x11.XInternAtom.restype = c_ulong
    x11.XInternAtom.argtypes = [c_void_p, c_char_p, c_int]
    x11.XSendEvent.argtypes = [c_void_p, c_ulong, c_int, c_long, c_void_p]
    x11.XRootWindow.restype = c_ulong
    x11.XRootWindow.argtypes = [c_void_p, c_int]
    x11.XDefaultScreen.restype = c_int
    x11.XDefaultScreen.argtypes = [c_void_p]
    dpy = x11.XOpenDisplay(b":0.0")
    if not dpy:
        return
    wid = x11cap.find_window(dpy, title, 400, 300, None)
    if not wid:
        return
    root = x11.XRootWindow(dpy, x11.XDefaultScreen(dpy))
    ev = XClientMessageEvent()
    ev.type = 33
    ev.window = wid
    ev.message_type = x11.XInternAtom(dpy, b"_NET_WM_STATE", 0)
    ev.format = 32
    ev.data0 = 0
    ev.data1 = x11.XInternAtom(dpy, b"_NET_WM_STATE_MAXIMIZED_HORZ", 0)
    ev.data2 = x11.XInternAtom(dpy, b"_NET_WM_STATE_MAXIMIZED_VERT", 0)
    ev.data3 = 1
    x11.XSendEvent(dpy, root, 0, (1 << 20) | (1 << 19), byref(ev))
    x11.XFlush(dpy)
    time.sleep(0.3)
    # wid is the client. Resize the WM frame, otherwise the client stays maximized.
    parent = c_ulong()
    junk = c_ulong()
    kids = POINTER(c_ulong)()
    nkids = c_uint()
    x11.XQueryTree(dpy, wid, byref(junk), byref(parent), byref(kids), byref(nkids))
    frame = int(parent.value) or wid
    x11.XResizeWindow(dpy, frame, 980, 680)
    x11.XMoveWindow(dpy, frame, 60, 40)
    x11.XFlush(dpy)


def call(args: list[str]) -> dict:
    proc = subprocess.run([str(CLIENT), *args], capture_output=True, text=True)
    line = (proc.stdout or "").strip().splitlines()
    if not line:
        return {"ok": False, "error": (proc.stderr or "no reply").strip()}
    try:
        return json.loads(line[-1])
    except json.JSONDecodeError:
        return {"ok": False, "error": line[-1]}


def main() -> None:
    subprocess.run(["pkill", "-f", "soffice.bin"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.5)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["HOME"] = "/tmp/gtk-remote-demo/lo-home"
    env["DISPLAY"] = os.environ["DISPLAY"]
    env["GSK_RENDERER"] = "cairo"
    env["GDK_BACKEND"] = "x11"
    Path(env["HOME"]).mkdir(parents=True, exist_ok=True)

    rec = WindowRecord(
        TITLE,
        OUT,
        fps=8,
        max_sec=40.0,
        min_w=800,
        min_h=500,
        overlay_pointer=False,
        hide_cursor=False,
        swap_rb=False,
    )
    rec.spawn(
        [
            str(SOFFICE),
            "--calc",
            "--norestore",
            "--nologo",
            "--nolockcheck",
            "--accept=socket,host=127.0.0.1,port=2002;urp;",
        ],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    ready = None
    deadline = time.time() + 25
    while time.time() < deadline:
        reply = call(["sheet-goto", "--cell", "A1"])
        if reply.get("ok"):
            ready = reply
            break
        time.sleep(0.4)
    if ready is None:
        raise SystemExit("Calc UNO not ready")

    title = None
    deadline = time.time() + 8
    while time.time() < deadline:
        title = window_title()
        if title:
            break
        time.sleep(0.15)
    if not title:
        raise SystemExit("Calc window not found")
    rec.title = title
    rec.attach(timeout=12)

    from record import x11cap

    x11cap.x11.XMapRaised(rec._dpy, rec._wid)
    x11cap.x11.XRaiseWindow(rec._dpy, rec._wid)
    x11cap.x11.XFlush(rec._dpy)
    time.sleep(0.4)

    rec.start_capture()
    rec.hold(0.6)
    rows = [
        ("Item", 2, "=B1*3"),
        ("Lesson", 4, "=B2*3"),
        ("Pass", 6, "=B3*3"),
    ]
    for i, (name, qty, formula) in enumerate(rows):
        row = i + 1
        reply = call(["sheet-set", "--cell", f"A{row}", "--kind", "string", "--text", name])
        if not reply.get("ok"):
            print(f"sheet-set A{row} failed: {reply}", file=sys.stderr)
        rec.hold(0.45)
        reply = call(["sheet-set", "--cell", f"B{row}", "--kind", "value", "--text", str(qty)])
        if not reply.get("ok"):
            print(f"sheet-set B{row} failed: {reply}", file=sys.stderr)
        rec.hold(0.45)
        reply = call(["sheet-set", "--cell", f"C{row}", "--kind", "formula", "--text", formula])
        if not reply.get("ok"):
            print(f"sheet-set C{row} failed: {reply}", file=sys.stderr)
        rec.hold(0.55)
    rec.hold(2.4)
    rec._stop.set()
    if rec._cap:
        rec._cap.join(timeout=8)
    if rec._ff and rec._ff.poll() is None:
        try:
            rec._ff.wait(timeout=20)
        except subprocess.TimeoutExpired:
            rec._ff.kill()
    rec.kill_all()
    if not OUT.exists() or OUT.stat().st_size < 20_000:
        size = OUT.stat().st_size if OUT.exists() else 0
        raise SystemExit(f"recording failed: {OUT} size={size} frames={rec.nframes}")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes, {rec.nframes} frames, {rec._w}x{rec._h})")


if __name__ == "__main__":
    main()
