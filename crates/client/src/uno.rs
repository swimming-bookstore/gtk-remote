//! LibreOffice sheet access.
//!
//! Calc's grid is a VCL widget, not a GTK widget, so AT-SPI has no cell to
//! write. LibreOffice already publishes those cells on its UNO socket when it
//! is started with `--accept`. This module is that bridge, inside the Rust
//! client: the agent still calls `gtk-remote`, and this process asks the
//! in-process pyuno runtime (the only stable UNO ABI shipped with
//! LibreOffice) to resolve the document and set the cell.
//!
//! Writer's document view is a GTK accessible, so `type_text` / `select_text`
//! go through the preload socket. Character weight does not: toolbar Bold
//! writes a UNO text portion, which is the `text-style` op below.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

const DRIVER: &str = r#"
import json, os, sys
sys.path.append("/usr/lib/libreoffice/program")
os.environ.setdefault("URE_BOOTSTRAP", "vnd.sun.star.pathname:/usr/lib/libreoffice/program/fundamentalrc")
lp = os.environ.get("LD_LIBRARY_PATH", "")
prog = "/usr/lib/libreoffice/program"
if prog not in lp.split(":"):
    os.environ["LD_LIBRARY_PATH"] = prog + (":" + lp if lp else "")
import uno

req = json.loads(sys.stdin.read() or "{}")
url = req.get("url") or "uno:socket,host=127.0.0.1,port=2002;urp;StarOffice.ComponentContext"
local = uno.getComponentContext()
resolver = local.ServiceManager.createInstanceWithContext("com.sun.star.bridge.UnoUrlResolver", local)
try:
    ctx = resolver.resolve(url)
except Exception as e:
    sys.stdout.write(json.dumps({"ok": False, "error": "uno connect: %s" % e}))
    sys.exit(0)
desktop = ctx.ServiceManager.createInstanceWithContext("com.sun.star.frame.Desktop", ctx)
doc = desktop.getCurrentComponent()
if doc is None:
    sys.stdout.write(json.dumps({"ok": False, "error": "no document"}))
    sys.exit(0)
op = req.get("op")
if op == "text-style":
    # Model CharWeight does not repaint. Toolbar Bold is .uno:Bold on the
    # view cursor. A model cursor's goRight counts differently from the view
    # (Writer inserts a trailing paragraph mark), so move the view cursor by
    # the same character offsets the agent used for select_text.
    start = int(req.get("start") or 0)
    end = int(req.get("end") or 0)
    weight = float(req.get("weight") or 150)
    text = req.get("text") or ""
    ctrl = doc.getCurrentController()
    view = ctrl.getViewCursor()
    view.gotoStart(False)
    if start:
        view.goRight(start, False)
    view.goRight(max(0, end - start), True)
    selected = view.getString()
    # A view cursor's goRight does not match the AT-SPI offsets (Writer counts
    # a trailing paragraph mark). If the selection is empty or wrong, find the
    # phrase in the paragraph and select that.
    if text and text not in selected:
        body = ""
        try:
            body = doc.Text.String
        except Exception:
            body = ""
        at = body.find(text) if body else -1
        if at >= 0:
            view.gotoStart(False)
            if at:
                view.goRight(at, False)
            view.goRight(len(text), True)
            selected = view.getString()
            start, end = at, at + len(text)
    frame = ctrl.getFrame()
    helper = ctx.ServiceManager.createInstanceWithContext(
        "com.sun.star.frame.DispatchHelper", ctx)
    current = float(view.CharWeight or 100)
    if (weight >= 140 and current < 140) or (weight < 140 and current >= 140):
        helper.executeDispatch(frame, ".uno:Bold", "", 0, ())
    elif abs(current - weight) > 1:
        for prop in ("CharWeight", "CharWeightAsian", "CharWeightComplex"):
            view.setPropertyValue(prop, weight)
    # Drop the selection so the highlight does not hide the new weight.
    view.collapseToEnd()
    sys.stdout.write(json.dumps({
        "ok": True, "selected": selected, "weight": float(view.CharWeight or 0),
        "start": start, "end": end,
    }))
    sys.exit(0)
try:
    sheets = doc.Sheets
except Exception:
    sys.stdout.write(json.dumps({"ok": False, "error": "current document is not a spreadsheet"}))
    sys.exit(0)
index = int(req.get("sheet") or 0)
grid = sheets.getByIndex(index)
col = int(req.get("col") or 0)
row = int(req.get("row") or 0)
cell = grid.getCellByPosition(col, row)

def show():
    # Model writes do not move the cursor. The controller does: select the
    # cell and scroll it into view, which is the same path as the Name Box.
    try:
        controller = doc.getCurrentController()
        controller.setActiveSheet(grid)
        controller.select(cell)
    except Exception as e:
        return str(e)
    return None

if op == "goto":
    err = show()
    if err:
        sys.stdout.write(json.dumps({"ok": False, "error": "select: %s" % err, "col": col, "row": row}))
    else:
        sys.stdout.write(json.dumps({
            "ok": True, "col": col, "row": row, "selected": True,
            "string": cell.String, "value": cell.Value, "formula": cell.Formula,
        }))
elif op == "set":
    kind = req.get("kind") or "string"
    if kind == "value":
        cell.Value = float(req.get("text") or 0)
    elif kind == "formula":
        cell.Formula = str(req.get("text") or "")
    else:
        cell.String = str(req.get("text") or "")
    show()
    sys.stdout.write(json.dumps({
        "ok": True, "col": col, "row": row, "kind": kind, "selected": True,
        "string": cell.String, "value": cell.Value, "formula": cell.Formula,
    }))
elif op == "get":
    sys.stdout.write(json.dumps({
        "ok": True, "col": col, "row": row,
        "string": cell.String, "value": cell.Value, "formula": cell.Formula,
    }))
else:
    sys.stdout.write(json.dumps({"ok": False, "error": "unknown uno op"}))
"#;

fn python() -> String {
    std::env::var("GTK_REMOTE_PYTHON").unwrap_or_else(|_| "python3".into())
}

pub fn sheet_call(req: &serde_json::Value) -> Result<serde_json::Value, String> {
    let mut child = Command::new(python())
        .arg("-c")
        .arg(DRIVER)
        .env(
            "URE_BOOTSTRAP",
            "vnd.sun.star.pathname:/usr/lib/libreoffice/program/fundamentalrc",
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("start uno bridge: {e}"))?;
    {
        let mut stdin = child.stdin.take().ok_or("uno bridge stdin")?;
        stdin
            .write_all(req.to_string().as_bytes())
            .map_err(|e| e.to_string())?;
    }
    let mut stdout = child.stdout.take().ok_or("uno bridge stdout")?;
    let mut stderr = child.stderr.take().ok_or("uno bridge stderr")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        let mut err = String::new();
        let _ = stderr.read_to_string(&mut err);
        let _ = tx.send((out, err));
    });
    let (out, err) = rx
        .recv_timeout(Duration::from_secs(8))
        .map_err(|_| "uno bridge timed out".to_string())?;
    let status = child.wait().map_err(|e| e.to_string())?;
    if out.trim().is_empty() {
        return Err(format!(
            "uno bridge failed ({status}): {}",
            err.trim().lines().last().unwrap_or("no output")
        ));
    }
    serde_json::from_str(out.trim()).map_err(|e| format!("uno bridge bad json: {e}: {out}"))
}
