//! Agent client for gtk-remote.
//!
//! Talks newline-delimited JSON to the per-process socket a preloaded GTK app
//! publishes. Spreadsheet cells are the exception: Calc's grid is VCL, so
//! `sheet-set` / `sheet-get` go through LibreOffice's UNO socket (`uno` module)
//! instead of the GTK tree.

mod uno;

use std::env;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn runtime_dir() -> PathBuf {
    let base = env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc_getuid() }));
    PathBuf::from(base).join("gtk-remote")
}

fn libc_getuid() -> u32 {
    unsafe { libc_getuid_sys() }
}

unsafe fn libc_getuid_sys() -> u32 {
    libc::getuid()
}

fn list_socks() -> Vec<PathBuf> {
    if let Ok(p) = env::var("GTK_REMOTE_SOCK") {
        if !p.is_empty() {
            return vec![PathBuf::from(p)];
        }
    }
    let d = runtime_dir();
    let mut socks: Vec<PathBuf> = match std::fs::read_dir(&d) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("sock"))
            .collect(),
        Err(_) => Vec::new(),
    };
    socks.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    socks.reverse();
    socks
}

fn request(path: &Path, payload: &serde_json::Value) -> Result<serde_json::Value, String> {
    let mut s = UnixStream::connect(path).map_err(|e| format!("connect {}: {e}", path.display()))?;
    s.set_read_timeout(Some(Duration::from_secs(4))).ok();
    s.set_write_timeout(Some(Duration::from_secs(4))).ok();
    let line = payload.to_string() + "\n";
    s.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        match s.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                if !buf.is_empty() {
                    break;
                }
                return Err(e.to_string());
            }
            Err(e) => {
                if !buf.is_empty() {
                    break;
                }
                return Err(e.to_string());
            }
        }
        if buf.len() > 8 * 1024 * 1024 {
            break;
        }
    }
    if buf.is_empty() {
        return Err(format!("empty reply from {}", path.display()));
    }
    let line = String::from_utf8_lossy(&buf);
    let line = line.split('\n').next().unwrap_or("");
    serde_json::from_str(line).map_err(|e| format!("bad json: {e}: {line}"))
}

fn pick_sock(explicit: Option<&str>) -> Result<PathBuf, String> {
    if let Some(p) = explicit {
        return Ok(PathBuf::from(p));
    }
    list_socks().into_iter().next().ok_or_else(|| {
        "no gtk-remote socket found.\nLaunch the app with gtk-remote-launch, or set LD_PRELOAD=libgtk_remote.so".into()
    })
}

fn emit(obj: &serde_json::Value, pretty: bool) {
    let s = if pretty {
        serde_json::to_string_pretty(obj).unwrap_or_else(|_| obj.to_string())
    } else {
        obj.to_string()
    };
    println!("{s}");
    if obj.get("ok").and_then(|v| v.as_bool()) == Some(false) {
        std::process::exit(1);
    }
}

struct Flags {
    pretty: bool,
    sock: Option<String>,
    id: Option<u64>,
    name: Option<String>,
    role: Option<String>,
    placeholder: Option<String>,
    description: Option<String>,
    klass: Option<String>,
    css: Option<String>,
    index: i64,
    depth: i64,
    visible_only: bool,
    limit: i64,
    action: Option<String>,
    text: Option<String>,
    value: Option<String>,
    x: f64,
    y: f64,
    button: i64,
    cell: Option<String>,
    kind: Option<String>,
    uno: Option<String>,
    start: i64,
    end: i64,
    weight: f64,
}

fn selector(f: &Flags) -> serde_json::Map<String, serde_json::Value> {
    let mut d = serde_json::Map::new();
    let put = |d: &mut serde_json::Map<String, serde_json::Value>, k: &str, v: &Option<String>| {
        if let Some(v) = v {
            d.insert(k.into(), serde_json::Value::String(v.clone()));
        }
    };
    put(&mut d, "name", &f.name);
    put(&mut d, "role", &f.role);
    put(&mut d, "placeholder", &f.placeholder);
    put(&mut d, "description", &f.description);
    put(&mut d, "css", &f.css);
    if let Some(c) = &f.klass {
        d.insert("class".into(), serde_json::Value::String(c.clone()));
    }
    if let Some(id) = f.id {
        d.insert("widget".into(), serde_json::json!(id));
    }
    if f.index != 0 {
        d.insert("index".into(), serde_json::json!(f.index));
    }
    d
}

fn cmd_raw(f: &Flags, cmd: &str, extra: serde_json::Value) -> Result<(), String> {
    let sock = pick_sock(f.sock.as_deref())?;
    let mut payload = serde_json::json!({"id": 1, "cmd": cmd});
    if let Some(obj) = payload.as_object_mut() {
        for (k, v) in selector(f) {
            obj.insert(k, v);
        }
        if let Some(extra) = extra.as_object() {
            for (k, v) in extra {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    let reply = request(&sock, &payload)?;
    emit(&reply, f.pretty);
    Ok(())
}

fn parse_cell(cell: &str) -> Result<(i64, i64), String> {
    let cell = cell.trim();
    let split = cell
        .find(|c: char| c.is_ascii_digit())
        .ok_or_else(|| format!("bad cell {cell}"))?;
    if split == 0 {
        return Err(format!("bad cell {cell}"));
    }
    let (col_s, row_s) = cell.split_at(split);
    let mut col = 0i64;
    for c in col_s.chars() {
        if !c.is_ascii_alphabetic() {
            return Err(format!("bad cell {cell}"));
        }
        col = col * 26 + (c.to_ascii_uppercase() as i64 - 'A' as i64 + 1);
    }
    let row: i64 = row_s.parse().map_err(|_| format!("bad cell {cell}"))?;
    if row < 1 {
        return Err(format!("bad cell {cell}"));
    }
    Ok((col - 1, row - 1))
}

fn sheet_cmd(f: &Flags, op: &str) -> Result<(), String> {
    let cell = f.cell.clone().unwrap_or_else(|| "A1".into());
    let (col, row) = parse_cell(&cell)?;
    let mut req = serde_json::json!({
        "op": op,
        "col": col,
        "row": row,
        "kind": f.kind.clone().unwrap_or_else(|| "string".into()),
        "text": f.text.clone().or_else(|| f.value.clone()).unwrap_or_default(),
    });
    if let Some(url) = &f.uno {
        req.as_object_mut().unwrap().insert("url".into(), serde_json::Value::String(url.clone()));
    }
    let mut reply = crate::uno::sheet_call(&req)?;
    if let Some(obj) = reply.as_object_mut() {
        obj.insert("cell".into(), serde_json::Value::String(cell));
    }
    emit(&reply, f.pretty);
    Ok(())
}

fn text_style_cmd(f: &Flags) -> Result<(), String> {
    let mut req = serde_json::json!({
        "op": "text-style",
        "start": f.start,
        "end": f.end,
        "weight": f.weight,
        "text": f.text.clone().unwrap_or_default(),
    });
    if let Some(url) = &f.uno {
        req.as_object_mut().unwrap().insert("url".into(), serde_json::Value::String(url.clone()));
    }
    let reply = crate::uno::sheet_call(&req)?;
    emit(&reply, f.pretty);
    Ok(())
}

fn parse() -> (String, Flags) {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let mut f = Flags {
        pretty: false,
        sock: None,
        id: None,
        name: None,
        role: None,
        placeholder: None,
        description: None,
        klass: None,
        css: None,
        index: 0,
        depth: 6,
        visible_only: false,
        limit: 50,
        action: None,
        text: None,
        value: None,
        x: 0.0,
        y: 0.0,
        button: 1,
        cell: None,
        kind: None,
        uno: None,
        start: 0,
        end: 0,
        weight: 150.0,
    };
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].clone();
        let mut next = || {
            i += 1;
            args.get(i).cloned()
        };
        match a.as_str() {
            "--pretty" => f.pretty = true,
            "--visible-only" => f.visible_only = true,
            "--sock" => f.sock = next(),
            "--id" => f.id = next().and_then(|s| s.parse().ok()),
            "--name" => f.name = next(),
            "--role" => f.role = next(),
            "--placeholder" => f.placeholder = next(),
            "--description" => f.description = next(),
            "--class" => f.klass = next(),
            "--css" => f.css = next(),
            "--index" => f.index = next().and_then(|s| s.parse().ok()).unwrap_or(0),
            "--depth" => f.depth = next().and_then(|s| s.parse().ok()).unwrap_or(6),
            "--limit" => f.limit = next().and_then(|s| s.parse().ok()).unwrap_or(50),
            "--action" => f.action = next(),
            "--text" => f.text = next(),
            "--value" => f.value = next(),
            "--x" => f.x = next().and_then(|s| s.parse().ok()).unwrap_or(0.0),
            "--y" => f.y = next().and_then(|s| s.parse().ok()).unwrap_or(0.0),
            "--button" => f.button = next().and_then(|s| s.parse().ok()).unwrap_or(1),
            "--cell" => f.cell = next(),
            "--kind" => f.kind = next(),
            "--uno" => f.uno = next(),
            "--start" => f.start = next().and_then(|s| s.parse().ok()).unwrap_or(0),
            "--end" => f.end = next().and_then(|s| s.parse().ok()).unwrap_or(0),
            "--weight" => f.weight = next().and_then(|s| s.parse().ok()).unwrap_or(150.0),
            other if !other.starts_with('-') => rest.push(other.to_string()),
            other => {
                eprintln!("unknown flag {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    let cmd = rest.first().cloned().unwrap_or_else(|| {
        eprintln!(
            "gtk-remote — drive a preloaded GTK app\n\n\
             commands: apps ping app tree find get focused press do set-text set-value focus click-at type sheet-set sheet-get sheet-goto text-style\n\
             selectors: --sock --id --name --role --placeholder --description --class --css --index\n\
             sheet: --cell A1 --kind string|value|formula --text ... [--uno UNO-URL]\n\
             text-style: --start N --end N --weight 150 [--uno UNO-URL]\n\
             sheet-goto --cell A1 selects that cell and scrolls it into view"
        );
        std::process::exit(2);
    });
    (cmd, f)
}

fn main() {
    let (cmd, f) = parse();
    let result = match cmd.as_str() {
        "apps" => {
            let mut rows = Vec::new();
            for sock in list_socks() {
                match request(&sock, &serde_json::json!({"id": 1, "cmd": "app"})) {
                    Ok(mut info) => {
                        if let Some(obj) = info.as_object_mut() {
                            obj.insert("sock".into(), serde_json::Value::String(sock.display().to_string()));
                        }
                        rows.push(info);
                    }
                    Err(e) => rows.push(serde_json::json!({"ok": false, "sock": sock.display().to_string(), "error": e})),
                }
            }
            emit(&serde_json::json!({"ok": true, "apps": rows}), f.pretty);
            Ok(())
        }
        "ping" => cmd_raw(&f, "ping", serde_json::json!({})),
        "app" => cmd_raw(&f, "app", serde_json::json!({})),
        "tree" => cmd_raw(&f, "tree", serde_json::json!({"depth": f.depth, "visible_only": f.visible_only})),
        "find" => cmd_raw(&f, "find", serde_json::json!({"limit": f.limit})),
        "get" => cmd_raw(&f, "get", serde_json::json!({"depth": f.depth})),
        "focused" => cmd_raw(&f, "focused", serde_json::json!({})),
        "press" => cmd_raw(&f, "do_action", serde_json::json!({"action": "click"})),
        "do" => {
            let action = f.action.clone().unwrap_or_else(|| {
                eprintln!("do requires --action");
                std::process::exit(2);
            });
            cmd_raw(&f, "do_action", serde_json::json!({"action": action}))
        }
        "set-text" => {
            let text = f.text.clone().unwrap_or_default();
            cmd_raw(&f, "set_text", serde_json::json!({"text": text}))
        }
        "set-value" => {
            let value = f.value.clone().or_else(|| f.text.clone()).unwrap_or_default();
            cmd_raw(&f, "set_value", serde_json::json!({"value": value}))
        }
        "focus" => cmd_raw(&f, "grab_focus", serde_json::json!({})),
        "click-at" => cmd_raw(&f, "click_at", serde_json::json!({"x": f.x, "y": f.y, "button": f.button})),
        "type" => {
            let text = f.text.clone().unwrap_or_default();
            cmd_raw(&f, "type_text", serde_json::json!({"text": text}))
        }
        "sheet-set" => sheet_cmd(&f, "set"),
        "sheet-get" => sheet_cmd(&f, "get"),
        "sheet-goto" => sheet_cmd(&f, "goto"),
        "text-style" => text_style_cmd(&f),
        other => {
            eprintln!("unknown command {other}");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
