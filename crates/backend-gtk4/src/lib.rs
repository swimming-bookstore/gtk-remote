//! GTK 4 backend. Loaded only after the stub has confirmed this process mapped
//! libgtk-4. Never preloaded into a GTK 3 app.
//!
//! Exports: void gtk_remote_backend_arm(void);

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int, c_void};
use std::sync::{Condvar, Mutex};

use serde_json::Value;

const RTLD_DEFAULT: *mut c_void = libc::RTLD_DEFAULT;

fn sym<T>(name: &str) -> Option<T> {
    let c = CString::new(name).ok()?;
    let p = unsafe { libc::dlsym(RTLD_DEFAULT, c.as_ptr()) };
    if p.is_null() {
        None
    } else {
        Some(unsafe { std::mem::transmute_copy(&p) })
    }
}

fn call0<T>(name: &str) -> Option<T> {
    let f: unsafe extern "C" fn() -> T = sym(name)?;
    Some(unsafe { f() })
}

unsafe fn cstr<'a>(p: *const c_char) -> &'a str {
    if p.is_null() {
        ""
    } else {
        CStr::from_ptr(p).to_str().unwrap_or("")
    }
}

fn escape(s: &str) -> String {
    let mut o = String::new();
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

fn substr_ci(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    hay.to_ascii_lowercase().contains(&needle.to_ascii_lowercase())
}

fn role_name(role: i32) -> &'static str {
    match role {
        0 => "alert",
        1 => "alert-dialog",
        2 => "banner",
        3 => "button",
        4 => "caption",
        5 => "cell",
        6 => "checkbox",
        7 => "column-header",
        8 => "combo-box",
        9 => "command",
        10 => "composite",
        11 => "dialog",
        12 => "document",
        13 => "feed",
        14 => "form",
        15 => "generic",
        16 => "grid",
        17 => "grid-cell",
        18 => "group",
        19 => "heading",
        20 => "img",
        21 => "input",
        22 => "label",
        23 => "landmark",
        24 => "legend",
        25 => "link",
        26 => "list",
        27 => "list-box",
        28 => "list-item",
        29 => "log",
        30 => "main",
        31 => "marquee",
        32 => "math",
        33 => "meter",
        34 => "menu",
        35 => "menu-bar",
        36 => "menu-item",
        37 => "menu-item-checkbox",
        38 => "menu-item-radio",
        39 => "navigation",
        40 => "none",
        41 => "note",
        42 => "option",
        43 => "presentation",
        44 => "progress-bar",
        45 => "radio",
        46 => "radio-group",
        47 => "range",
        48 => "region",
        49 => "row",
        50 => "row-group",
        51 => "row-header",
        52 => "scrollbar",
        53 => "search",
        54 => "search-box",
        55 => "section",
        56 => "section-head",
        57 => "select",
        58 => "separator",
        59 => "slider",
        60 => "spin-button",
        61 => "status",
        62 => "structure",
        63 => "switch",
        64 => "tab",
        65 => "table",
        66 => "tab-list",
        67 => "tab-panel",
        68 => "text-box",
        69 => "time",
        70 => "timer",
        71 => "toolbar",
        72 => "tooltip",
        73 => "tree",
        74 => "tree-grid",
        75 => "tree-item",
        76 => "widget",
        77 => "window",
        78 => "toggle-button",
        79 => "application",
        80 => "paragraph",
        _ => "widget",
    }
}

fn g_type(name: &str) -> u64 {
    call0(name).unwrap_or(0)
}

fn is_a(obj: *mut c_void, type_fn: &str) -> bool {
    if obj.is_null() {
        return false;
    }
    let t = g_type(type_fn);
    if t == 0 {
        return false;
    }
    let f: unsafe extern "C" fn(*mut c_void, u64) -> i32 =
        match sym("g_type_check_instance_is_a") {
            Some(f) => f,
            None => return false,
        };
    unsafe { f(obj, t) != 0 }
}

fn type_name(obj: *mut c_void) -> String {
    if obj.is_null() {
        return "widget".into();
    }
    let name_of: unsafe extern "C" fn(u64) -> *const c_char = match sym("g_type_name") {
        Some(f) => f,
        None => return "widget".into(),
    };
    let class = unsafe { *(obj as *mut *mut u64) };
    if class.is_null() {
        return "widget".into();
    }
    let gtype = unsafe { *class };
    let n = unsafe { cstr(name_of(gtype)) };
    if n.is_empty() { "widget".into() } else { n.to_string() }
}

struct Ids {
    by_widget: HashMap<usize, u32>,
    by_id: HashMap<u32, usize>,
    next: u32,
}

fn ids() -> &'static Mutex<Ids> {
    static IDS: std::sync::OnceLock<Mutex<Ids>> = std::sync::OnceLock::new();
    IDS.get_or_init(|| {
        Mutex::new(Ids {
            by_widget: HashMap::new(),
            by_id: HashMap::new(),
            next: 1,
        })
    })
}

unsafe extern "C" fn on_widget_gone(data: *mut c_void, old: *mut c_void) {
    let id = data as usize as u32;
    if let Ok(mut t) = ids().lock() {
        t.by_id.remove(&id);
        t.by_widget.remove(&(old as usize));
    }
}

fn widget_id(w: *mut c_void) -> u32 {
    if w.is_null() {
        return 0;
    }
    let mut t = ids().lock().unwrap();
    if let Some(id) = t.by_widget.get(&(w as usize)).copied() {
        return id;
    }
    let id = t.next;
    t.next += 1;
    t.by_widget.insert(w as usize, id);
    t.by_id.insert(id, w as usize);
    drop(t);
    let weak: unsafe extern "C" fn(*mut c_void, unsafe extern "C" fn(*mut c_void, *mut c_void), *mut c_void) =
        match sym("g_object_weak_ref") {
            Some(f) => f,
            None => return id,
        };
    unsafe { weak(w, on_widget_gone, id as usize as *mut c_void) };
    id
}

fn widget_from_id(id: u32) -> *mut c_void {
    if id == 0 {
        return std::ptr::null_mut();
    }
    let t = ids().lock().unwrap();
    t.by_id.get(&id).copied().unwrap_or(0) as *mut c_void
}

fn prop_string(w: *mut c_void, prop: &str) -> Option<String> {
    let find: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void = sym("g_object_class_find_property")?;
    let class = unsafe { *(w as *mut *mut c_void) };
    let cprop = CString::new(prop).ok()?;
    if unsafe { find(class, cprop.as_ptr()).is_null() } {
        return None;
    }
    let get: unsafe extern "C" fn(*mut c_void, *const c_char, *mut *mut c_char, *mut c_void) = sym("g_object_get")?;
    let mut out: *mut c_char = std::ptr::null_mut();
    unsafe { get(w, cprop.as_ptr(), &mut out, std::ptr::null_mut()) };
    if out.is_null() {
        return None;
    }
    let s = unsafe { cstr(out) }.to_string();
    if let Some(freefn) = sym::<unsafe extern "C" fn(*mut c_void)>("g_free") {
        unsafe { freefn(out as *mut c_void) };
    }
    if s.is_empty() { None } else { Some(s) }
}

fn widget_name(w: *mut c_void) -> String {
    if w.is_null() {
        return String::new();
    }
    let wptr = w as usize;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| widget_name_inner(wptr as *mut c_void)))
        .unwrap_or_default()
}

fn widget_name_inner(w: *mut c_void) -> String {
    if let Some(s) = prop_string(w, "accessible-label") {
        return s;
    }
    if is_a(w, "gtk_label_get_type") {
        let f: unsafe extern "C" fn(*mut c_void) -> *const c_char = match sym("gtk_label_get_text") {
            Some(f) => f,
            None => return String::new(),
        };
        let t = unsafe { cstr(f(w)) };
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if is_a(w, "gtk_button_get_type") && !is_a(w, "gtk_menu_button_get_type") {
        let first: unsafe extern "C" fn(*mut c_void) -> *mut c_void = match sym("gtk_widget_get_first_child") {
            Some(f) => f,
            None => return String::new(),
        };
        let next: unsafe extern "C" fn(*mut c_void) -> *mut c_void = match sym("gtk_widget_get_next_sibling") {
            Some(f) => f,
            None => return String::new(),
        };
        let mut child = unsafe { first(w) };
        while !child.is_null() {
            if is_a(child, "gtk_label_get_type") {
                let f: unsafe extern "C" fn(*mut c_void) -> *const c_char = match sym("gtk_label_get_text") {
                    Some(f) => f,
                    None => break,
                };
                let t = unsafe { cstr(f(child)) };
                if !t.is_empty() {
                    return t.to_string();
                }
            }
            child = unsafe { next(child) };
        }
    }
    if is_a(w, "gtk_window_get_type") {
        let f: unsafe extern "C" fn(*mut c_void) -> *const c_char = match sym("gtk_window_get_title") {
            Some(f) => f,
            None => return String::new(),
        };
        let t = unsafe { cstr(f(w)) };
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if is_a(w, "gtk_check_button_get_type") {
        let first: unsafe extern "C" fn(*mut c_void) -> *mut c_void = match sym("gtk_widget_get_first_child") {
            Some(f) => f,
            None => return String::new(),
        };
        let next: unsafe extern "C" fn(*mut c_void) -> *mut c_void = match sym("gtk_widget_get_next_sibling") {
            Some(f) => f,
            None => return String::new(),
        };
        let mut child = unsafe { first(w) };
        while !child.is_null() {
            if is_a(child, "gtk_label_get_type") {
                let f: unsafe extern "C" fn(*mut c_void) -> *const c_char = match sym("gtk_label_get_text") {
                    Some(f) => f,
                    None => break,
                };
                let t = unsafe { cstr(f(child)) };
                if !t.is_empty() {
                    return t.to_string();
                }
            }
            child = unsafe { next(child) };
        }
    }
    let get_name: unsafe extern "C" fn(*mut c_void) -> *const c_char = match sym("gtk_widget_get_name") {
        Some(f) => f,
        None => return String::new(),
    };
    let css: unsafe extern "C" fn(*mut c_void) -> *const c_char = match sym("gtk_widget_get_css_name") {
        Some(f) => f,
        None => return String::new(),
    };
    let n = unsafe { cstr(get_name(w)) };
    let c = unsafe { cstr(css(w)) };
    if !n.is_empty() && n != c {
        n.to_string()
    } else {
        String::new()
    }
}

fn placeholder(w: *mut c_void) -> String {
    if !is_a(w, "gtk_entry_get_type") {
        return String::new();
    }
    let f: unsafe extern "C" fn(*mut c_void) -> *const c_char = match sym("gtk_entry_get_placeholder_text") {
        Some(f) => f,
        None => return String::new(),
    };
    unsafe { cstr(f(w)) }.to_string()
}

fn value_text(w: *mut c_void) -> Option<String> {
    if is_a(w, "gtk_editable_get_type") {
        let f: unsafe extern "C" fn(*mut c_void) -> *const c_char = sym("gtk_editable_get_text")?;
        return Some(unsafe { cstr(f(w)) }.to_string());
    }
    if is_a(w, "gtk_label_get_type") {
        let f: unsafe extern "C" fn(*mut c_void) -> *const c_char = sym("gtk_label_get_text")?;
        return Some(unsafe { cstr(f(w)) }.to_string());
    }
    if is_a(w, "gtk_check_button_get_type") {
        let f: unsafe extern "C" fn(*mut c_void) -> i32 = sym("gtk_check_button_get_active")?;
        return Some(if unsafe { f(w) } != 0 { "true" } else { "false" }.into());
    }
    if is_a(w, "gtk_toggle_button_get_type") {
        let f: unsafe extern "C" fn(*mut c_void) -> i32 = sym("gtk_toggle_button_get_active")?;
        return Some(if unsafe { f(w) } != 0 { "true" } else { "false" }.into());
    }
    if is_a(w, "gtk_range_get_type") {
        let f: unsafe extern "C" fn(*mut c_void) -> c_double = sym("gtk_range_get_value")?;
        return Some(format!("{}", unsafe { f(w) }));
    }
    if is_a(w, "gtk_text_view_get_type") {
        let get_buf: unsafe extern "C" fn(*mut c_void) -> *mut c_void = sym("gtk_text_view_get_buffer")?;
        let buf = unsafe { get_buf(w) };
        if buf.is_null() {
            return None;
        }
        let get_bounds: unsafe extern "C" fn(*mut c_void, *mut GtkTextIter, *mut GtkTextIter) =
            sym("gtk_text_buffer_get_bounds")?;
        let get_text: unsafe extern "C" fn(*mut c_void, *const GtkTextIter, *const GtkTextIter, i32) -> *mut c_char =
            sym("gtk_text_buffer_get_text")?;
        let mut a = GtkTextIter::default();
        let mut b = GtkTextIter::default();
        unsafe { get_bounds(buf, &mut a, &mut b) };
        let p = unsafe { get_text(buf, &a, &b, 0) };
        if p.is_null() {
            return Some(String::new());
        }
        let s = unsafe { cstr(p) }.to_string();
        if let Some(freefn) = sym::<unsafe extern "C" fn(*mut c_void)>("g_free") {
            unsafe { freefn(p as *mut c_void) };
        }
        return Some(s);
    }
    None
}

#[repr(C)]
#[derive(Clone, Copy)]
struct GtkTextIter {
    dummy: [u8; 80],
}

impl Default for GtkTextIter {
    fn default() -> Self {
        Self { dummy: [0; 80] }
    }
}

struct Selector {
    name: String,
    role: String,
    placeholder: String,
    description: String,
    klass: String,
    css: String,
    id: u32,
    index: i32,
}

fn sel_from(v: &Value) -> Selector {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let id = v.get("widget").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    let id = if id == 0 {
        v.get("target").and_then(|x| x.as_u64()).unwrap_or(0) as u32
    } else {
        id
    };
    Selector {
        name: s("name"),
        role: s("role"),
        placeholder: s("placeholder"),
        description: s("description"),
        klass: s("class"),
        css: s("css"),
        id,
        index: v.get("index").and_then(|x| x.as_i64()).unwrap_or(0) as i32,
    }
}

fn accessible_role(w: *mut c_void) -> i32 {
    let f: unsafe extern "C" fn(*mut c_void) -> i32 = match sym("gtk_accessible_get_accessible_role") {
        Some(f) => f,
        None => return 72,
    };
    unsafe { f(w) }
}

fn css_name(w: *mut c_void) -> String {
    let f: unsafe extern "C" fn(*mut c_void) -> *const c_char = match sym("gtk_widget_get_css_name") {
        Some(f) => f,
        None => return String::new(),
    };
    unsafe { cstr(f(w)) }.to_string()
}

fn tooltip(w: *mut c_void) -> String {
    let f: unsafe extern "C" fn(*mut c_void) -> *const c_char = match sym("gtk_widget_get_tooltip_text") {
        Some(f) => f,
        None => return String::new(),
    };
    let p = unsafe { f(w) };
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { cstr(p) }.to_string();
    if let Some(freefn) = sym::<unsafe extern "C" fn(*mut c_void)>("g_free") {
        unsafe { freefn(p as *mut c_void) };
    }
    s
}

fn role_match(want: &str, have: &str) -> bool {
    if want.is_empty() {
        return true;
    }
    if substr_ci(have, want) {
        return true;
    }
    if substr_ci(want, "push") && substr_ci(have, "button") {
        return true;
    }
    if substr_ci(want, "AXButton") && substr_ci(have, "button") {
        return true;
    }
    let textish = substr_ci(want, "textfield")
        || substr_ci(want, "text field")
        || substr_ci(want, "AXTextField")
        || substr_ci(want, "entry")
        || substr_ci(want, "textbox");
    if textish && (substr_ci(have, "text") || substr_ci(have, "entry") || substr_ci(have, "search")) {
        return true;
    }
    false
}

fn widget_matches(w: *mut c_void, s: &Selector) -> bool {
    if s.id != 0 {
        return widget_id(w) == s.id;
    }
    let role = role_name(accessible_role(w));
    let klass = type_name(w);
    let css = css_name(w);
    if !role_match(&s.role, role) && !role_match(&s.role, &klass) && !role_match(&s.role, &css) {
        return false;
    }
    if !s.name.is_empty() {
        let name = widget_name(w);
        let mut ok = substr_ci(&name, &s.name);
        if !ok && is_a(w, "gtk_label_get_type") {
            if let Some(val) = value_text(w) {
                ok = substr_ci(&val, &s.name);
            }
        }
        if !ok {
            return false;
        }
    }
    if !s.placeholder.is_empty() && !substr_ci(&placeholder(w), &s.placeholder) {
        return false;
    }
    if !s.description.is_empty() && !substr_ci(&tooltip(w), &s.description) {
        return false;
    }
    if !s.klass.is_empty() && !substr_ci(&klass, &s.klass) {
        return false;
    }
    if !s.css.is_empty() && !substr_ci(&css, &s.css) {
        return false;
    }
    true
}

fn bool_prop(w: *mut c_void, name: &str) -> bool {
    let f: unsafe extern "C" fn(*mut c_void) -> i32 = match sym(name) {
        Some(f) => f,
        None => return false,
    };
    unsafe { f(w) != 0 }
}

fn append_actions(o: &mut String, w: *mut c_void) {
    o.push_str("\"actions\":[");
    let mut n = 0;
    let mut add = |name: &str| {
        if n > 0 {
            o.push(',');
        }
        n += 1;
        o.push('"');
        o.push_str(name);
        o.push('"');
    };
    if bool_prop(w, "gtk_widget_get_can_focus") {
        add("focus");
    }
    if is_a(w, "gtk_button_get_type")
        || is_a(w, "gtk_menu_button_get_type")
        || is_a(w, "gtk_check_button_get_type")
        || is_a(w, "gtk_toggle_button_get_type")
    {
        add("click");
        add("press");
        add("activate");
    }
    if is_a(w, "gtk_menu_button_get_type") {
        add("show-menu");
    }
    if is_a(w, "gtk_toggle_button_get_type") || is_a(w, "gtk_check_button_get_type") {
        add("toggle");
    }
    if is_a(w, "gtk_editable_get_type") || is_a(w, "gtk_text_view_get_type") {
        add("set-text");
        add("type");
    }
    if is_a(w, "gtk_range_get_type") || is_a(w, "gtk_spin_button_get_type") {
        add("set-value");
    }
    o.push(']');
}

fn emit_widget(o: &mut String, w: *mut c_void, depth: i32, max_depth: i32, visible_only: bool) {
    if w.is_null() {
        o.push_str("null");
        return;
    }
    let name = widget_name(w);
    let value = value_text(w);
    let role = role_name(accessible_role(w));
    let klass = type_name(w);
    let css = css_name(w);
    let ph = placeholder(w);
    let tip = tooltip(w);
    o.push_str(&format!(
        "{{\"id\":{},\"role\":\"{}\",\"class\":\"{}\",\"css\":\"{}\",\"name\":\"{}\",\"placeholder\":\"{}\",\"description\":\"{}\",",
        widget_id(w),
        escape(role),
        escape(&klass),
        escape(&css),
        escape(&name),
        escape(&ph),
        escape(&tip)
    ));
    match &value {
        Some(v) => o.push_str(&format!("\"value\":\"{}\",", escape(v))),
        None => o.push_str("\"value\":null,"),
    }
    o.push_str(&format!(
        "\"focused\":{},\"visible\":{},\"sensitive\":{},\"can_focus\":{},",
        bool_prop(w, "gtk_widget_has_focus"),
        bool_prop(w, "gtk_widget_is_visible") || bool_prop(w, "gtk_widget_get_visible"),
        bool_prop(w, "gtk_widget_get_sensitive"),
        bool_prop(w, "gtk_widget_get_can_focus")
    ));
    let width: unsafe extern "C" fn(*mut c_void) -> i32 = match sym("gtk_widget_get_width") {
        Some(f) => f,
        None => return,
    };
    let height: unsafe extern "C" fn(*mut c_void) -> i32 = match sym("gtk_widget_get_height") {
        Some(f) => f,
        None => return,
    };
    o.push_str(&format!(
        "\"bounds\":{{\"x\":0,\"y\":0,\"width\":{},\"height\":{}}},",
        unsafe { width(w) },
        unsafe { height(w) }
    ));
    append_actions(o, w);
    o.push_str(",\"children\":[");
    if depth < max_depth {
        let first: unsafe extern "C" fn(*mut c_void) -> *mut c_void = match sym("gtk_widget_get_first_child") {
            Some(f) => f,
            None => {
                o.push_str("]}");
                return;
            }
        };
        let next: unsafe extern "C" fn(*mut c_void) -> *mut c_void = match sym("gtk_widget_get_next_sibling") {
            Some(f) => f,
            None => {
                o.push_str("]}");
                return;
            }
        };
        let mut child = unsafe { first(w) };
        let mut n = 0;
        while !child.is_null() {
            let nxt = unsafe { next(child) };
            if !(visible_only && !bool_prop(child, "gtk_widget_get_visible")) {
                if n > 0 {
                    o.push(',');
                }
                n += 1;
                emit_widget(o, child, depth + 1, max_depth, visible_only);
            }
            child = nxt;
        }
    }
    o.push_str("]}");
}

fn toplevels() -> Vec<*mut c_void> {
    let f: unsafe extern "C" fn() -> *mut c_void = match sym("gtk_window_list_toplevels") {
        Some(f) => f,
        None => return Vec::new(),
    };
    let head = unsafe { f() };
    let mut list = head;
    let mut out = Vec::new();
    while !list.is_null() {
        let data = unsafe { *(list as *mut *mut c_void) };
        let next = unsafe { *((list as *mut u8).add(std::mem::size_of::<*mut c_void>()) as *mut *mut c_void) };
        if !data.is_null() {
            out.push(data);
        }
        list = next;
    }
    if let Some(free_list) = sym::<unsafe extern "C" fn(*mut c_void)>("g_list_free") {
        unsafe { free_list(head) };
    }
    out
}

fn collect(w: *mut c_void, s: &Selector, out: &mut Vec<*mut c_void>, limit: i32, visible_only: bool) {
    if w.is_null() {
        return;
    }
    if !(visible_only && !bool_prop(w, "gtk_widget_get_visible")) && widget_matches(w, s) {
        out.push(w);
        if limit > 0 && out.len() as i32 >= limit {
            return;
        }
    }
    let first: unsafe extern "C" fn(*mut c_void) -> *mut c_void = match sym("gtk_widget_get_first_child") {
        Some(f) => f,
        None => return,
    };
    let next: unsafe extern "C" fn(*mut c_void) -> *mut c_void = match sym("gtk_widget_get_next_sibling") {
        Some(f) => f,
        None => return,
    };
    let mut child = unsafe { first(w) };
    while !child.is_null() {
        let nxt = unsafe { next(child) };
        collect(child, s, out, limit, visible_only);
        if limit > 0 && out.len() as i32 >= limit {
            return;
        }
        child = nxt;
    }
}

fn resolve_one(s: &Selector) -> Result<*mut c_void, String> {
    if s.id != 0 {
        let w = widget_from_id(s.id);
        if w.is_null() {
            return Err(format!("no widget with id {} (it may have been destroyed)", s.id));
        }
        return Ok(w);
    }
    let mut hits = Vec::new();
    for top in toplevels() {
        collect(top, s, &mut hits, 0, false);
    }
    if !s.name.is_empty() {
        let exact: Vec<*mut c_void> = hits
            .iter()
            .copied()
            .filter(|w| widget_name(*w).eq_ignore_ascii_case(&s.name))
            .collect();
        if !exact.is_empty() {
            hits = exact;
        }
    }
    if hits.is_empty() {
        return Err("no widget matched".into());
    }
    if s.index < 0 || s.index as usize >= hits.len() {
        return Err(format!("index {} out of range ({} matches)", s.index, hits.len()));
    }
    Ok(hits[s.index as usize])
}

fn eq_ci(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn do_action(w: *mut c_void, action: &str) -> Result<(), String> {
    if eq_ci(action, "click") || eq_ci(action, "press") || eq_ci(action, "AXPress") || eq_ci(action, "activate") || eq_ci(action, "toggle")
    {
        if let Some(grab) = sym::<unsafe extern "C" fn(*mut c_void)>("gtk_widget_grab_focus") {
            unsafe { grab(w) };
        }
        if is_a(w, "gtk_button_get_type") || is_a(w, "gtk_check_button_get_type") {
            let emit: unsafe extern "C" fn(*mut c_void, *const c_char, ...) -> u32 = match sym("g_signal_emit_by_name") {
                Some(f) => f,
                None => return Err("g_signal_emit_by_name missing".into()),
            };
            let clicked = CString::new("clicked").unwrap();
            unsafe { emit(w, clicked.as_ptr()) };
            return Ok(());
        }
        if let Some(act) = sym::<unsafe extern "C" fn(*mut c_void) -> i32>("gtk_widget_activate") {
            if unsafe { act(w) } != 0 {
                return Ok(());
            }
        }
    }
    if eq_ci(action, "focus") {
        if let Some(grab) = sym::<unsafe extern "C" fn(*mut c_void)>("gtk_widget_grab_focus") {
            unsafe { grab(w) };
            return Ok(());
        }
    }
    Err(format!("action '{action}' not available on {}", type_name(w)))
}

fn set_text(w: *mut c_void, text: &str) -> Result<(), String> {
    let c = CString::new(text).unwrap_or_else(|_| CString::new("").unwrap());
    if is_a(w, "gtk_editable_get_type") {
        let f: unsafe extern "C" fn(*mut c_void, *const c_char) = sym("gtk_editable_set_text")
            .ok_or("gtk_editable_set_text missing")?;
        unsafe { f(w, c.as_ptr()) };
        return Ok(());
    }
    if is_a(w, "gtk_label_get_type") {
        let f: unsafe extern "C" fn(*mut c_void, *const c_char) = sym("gtk_label_set_text")
            .ok_or("gtk_label_set_text missing")?;
        unsafe { f(w, c.as_ptr()) };
        return Ok(());
    }
    if is_a(w, "gtk_text_view_get_type") {
        let get_buf: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
            sym("gtk_text_view_get_buffer").ok_or("no buffer")?;
        let set: unsafe extern "C" fn(*mut c_void, *const c_char, i32) = sym("gtk_text_buffer_set_text")
            .ok_or("gtk_text_buffer_set_text missing")?;
        let buf = unsafe { get_buf(w) };
        unsafe { set(buf, c.as_ptr(), -1) };
        return Ok(());
    }
    Err(format!("{} is not a text widget", type_name(w)))
}

fn parse_boolish(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    !(t == "false" || t == "off" || t == "no" || t == "0" || t.is_empty())
}

fn set_value(w: *mut c_void, text: &str) -> Result<(), String> {
    if is_a(w, "gtk_editable_get_type") || is_a(w, "gtk_label_get_type") || is_a(w, "gtk_text_view_get_type") {
        return set_text(w, text);
    }
    if is_a(w, "gtk_check_button_get_type") {
        let f: unsafe extern "C" fn(*mut c_void, i32) = sym("gtk_check_button_set_active").ok_or("missing")?;
        unsafe { f(w, parse_boolish(text) as i32) };
        return Ok(());
    }
    if is_a(w, "gtk_toggle_button_get_type") {
        let f: unsafe extern "C" fn(*mut c_void, i32) = sym("gtk_toggle_button_set_active").ok_or("missing")?;
        unsafe { f(w, parse_boolish(text) as i32) };
        return Ok(());
    }
    if is_a(w, "gtk_range_get_type") {
        let f: unsafe extern "C" fn(*mut c_void, c_double) = sym("gtk_range_set_value").ok_or("missing")?;
        unsafe { f(w, text.parse().unwrap_or(0.0)) };
        return Ok(());
    }
    Err(format!("cannot set value on {}", type_name(w)))
}

fn focused_widget() -> *mut c_void {
    let mut focus = std::ptr::null_mut();
    let get: Option<unsafe extern "C" fn(*mut c_void) -> *mut c_void> = sym("gtk_window_get_focus");
    let Some(get) = get else { return focus };
    for top in toplevels() {
        if !is_a(top, "gtk_window_get_type") {
            continue;
        }
        let f = unsafe { get(top) };
        if !f.is_null() && bool_prop(f, "gtk_widget_has_focus") {
            return f;
        }
        if focus.is_null() {
            focus = f;
        }
    }
    focus
}

fn type_into(mut w: *mut c_void, text: &str) {
    if w.is_null() {
        w = focused_widget();
    }
    if w.is_null() {
        return;
    }
    let c = CString::new(text).unwrap_or_else(|_| CString::new("").unwrap());
    if is_a(w, "gtk_text_view_get_type") {
        let get_buf: unsafe extern "C" fn(*mut c_void) -> *mut c_void = match sym("gtk_text_view_get_buffer") {
            Some(f) => f,
            None => return,
        };
        let ins: unsafe extern "C" fn(*mut c_void, *const c_char, i32) = match sym("gtk_text_buffer_insert_at_cursor") {
            Some(f) => f,
            None => return,
        };
        let buf = unsafe { get_buf(w) };
        if !buf.is_null() {
            unsafe { ins(buf, c.as_ptr(), -1) };
        }
        return;
    }
    if is_a(w, "gtk_editable_get_type") {
        let get: unsafe extern "C" fn(*mut c_void) -> *const c_char = match sym("gtk_editable_get_text") {
            Some(f) => f,
            None => return,
        };
        let set: unsafe extern "C" fn(*mut c_void, *const c_char) = match sym("gtk_editable_set_text") {
            Some(f) => f,
            None => return,
        };
        let cur = unsafe { cstr(get(w)) };
        let next = format!("{cur}{text}");
        if let Ok(cs) = CString::new(next) {
            unsafe { set(w, cs.as_ptr()) };
        }
    }
}

struct Deferred {
    widget: usize,
    action: String,
    click: bool,
}

extern "C" fn run_deferred(data: *mut c_void) -> i32 {
    let d = unsafe { Box::from_raw(data as *mut Deferred) };
    let w = d.widget as *mut c_void;
    if !w.is_null() {
        if d.click {
            let _ = do_action(w, "click");
        } else {
            let _ = do_action(w, &d.action);
        }
    }
    0 // G_SOURCE_REMOVE
}

fn defer(w: *mut c_void, action: &str, click: bool) {
    let d = Box::new(Deferred {
        widget: w as usize,
        action: action.to_string(),
        click,
    });
    let idle: unsafe extern "C" fn(extern "C" fn(*mut c_void) -> i32, *mut c_void) -> u32 =
        match sym("g_idle_add") {
            Some(f) => f,
            None => return,
        };
    unsafe { idle(run_deferred, Box::into_raw(d) as *mut c_void) };
}

fn sock_path_global() -> &'static Mutex<String> {
    static P: std::sync::OnceLock<Mutex<String>> = std::sync::OnceLock::new();
    P.get_or_init(|| Mutex::new(String::new()))
}

fn handle(json: &str) -> String {
    let v: Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return "{\"id\":0,\"ok\":false,\"error\":\"bad json\"}\n".into(),
    };
    let id = v.get("id").and_then(|x| x.as_i64()).unwrap_or(0);
    let cmd = v.get("cmd").and_then(|x| x.as_str()).unwrap_or("");
    if cmd.is_empty() {
        return format!("{{\"id\":{id},\"ok\":false,\"error\":\"missing cmd\"}}\n");
    }
    let sel = sel_from(&v);
    let sock = sock_path_global().lock().unwrap().clone();
    let fail = |err: &str| format!("{{\"id\":{id},\"ok\":false,\"error\":\"{}\"}}\n", escape(err));

    match cmd {
        "ping" => format!(
            "{{\"id\":{id},\"ok\":true,\"pong\":true,\"pid\":{},\"sock\":\"{}\"}}\n",
            std::process::id(),
            escape(&sock)
        ),
        "app" => {
            let prg = std::env::var("GTK_REMOTE_APP").unwrap_or_default();
            let n = toplevels().len();
            format!(
                "{{\"id\":{id},\"ok\":true,\"pid\":{},\"prgname\":\"{}\",\"toplevels\":{n},\"toolkit\":\"gtk4\",\"sock\":\"{}\"}}\n",
                std::process::id(),
                escape(&prg),
                escape(&sock)
            )
        }
        "tree" => {
            let depth = v.get("depth").and_then(|x| x.as_i64()).unwrap_or(8) as i32;
            let vis = v.get("visible_only").and_then(|x| x.as_bool()).unwrap_or(false);
            let mut o = format!("{{\"id\":{id},\"ok\":true,\"windows\":[");
            for (i, top) in toplevels().into_iter().enumerate() {
                if i > 0 {
                    o.push(',');
                }
                emit_widget(&mut o, top, 0, depth, vis);
            }
            o.push_str("]}\n");
            o
        }
        "find" => {
            let limit = v.get("limit").and_then(|x| x.as_i64()).unwrap_or(50) as i32;
            let mut hits = Vec::new();
            for top in toplevels() {
                collect(top, &sel, &mut hits, limit, false);
            }
            let mut o = format!("{{\"id\":{id},\"ok\":true,\"matches\":[");
            for (i, w) in hits.iter().enumerate() {
                if i > 0 {
                    o.push(',');
                }
                emit_widget(&mut o, *w, 0, 0, false);
            }
            o.push_str("]}\n");
            o
        }
        "get" | "focused" => {
            let w = if cmd == "focused" {
                let f = focused_widget();
                if f.is_null() {
                    return fail("nothing focused");
                }
                f
            } else {
                match resolve_one(&sel) {
                    Ok(w) => w,
                    Err(e) => return fail(&e),
                }
            };
            let depth = v.get("depth").and_then(|x| x.as_i64()).unwrap_or(1) as i32;
            let mut o = format!("{{\"id\":{id},\"ok\":true,\"widget\":");
            emit_widget(&mut o, w, 0, depth, false);
            o.push_str("}\n");
            o
        }
        "do_action" => {
            let action = v.get("action").and_then(|x| x.as_str()).unwrap_or("activate");
            match resolve_one(&sel) {
                Ok(w) => match do_action(w, action) {
                    Ok(()) => format!("{{\"id\":{id},\"ok\":true,\"did\":\"{}\"}}\n", escape(action)),
                    Err(e) => fail(&e),
                },
                Err(e) => fail(&e),
            }
        }
        "set_text" | "set_value" => {
            let text = v
                .get("text")
                .and_then(|x| x.as_str())
                .or_else(|| v.get("value").and_then(|x| x.as_str()))
                .unwrap_or("");
            let w = match resolve_one(&sel) {
                Ok(w) => w,
                Err(e) => return fail(&e),
            };
            let r = if cmd == "set_text" { set_text(w, text) } else { set_value(w, text) };
            match r {
                Ok(()) => {
                    let mut o = format!("{{\"id\":{id},\"ok\":true,\"widget\":");
                    emit_widget(&mut o, w, 0, 0, false);
                    o.push_str("}\n");
                    o
                }
                Err(e) => fail(&e),
            }
        }
        "grab_focus" => match resolve_one(&sel) {
            Ok(w) => {
                if let Some(grab) = sym::<unsafe extern "C" fn(*mut c_void)>("gtk_widget_grab_focus") {
                    unsafe { grab(w) };
                }
                let mut o = format!("{{\"id\":{id},\"ok\":true,\"widget\":");
                emit_widget(&mut o, w, 0, 0, false);
                o.push_str("}\n");
                o
            }
            Err(e) => fail(&e),
        },
        "click_at" => match resolve_one(&sel) {
            Ok(w) => {
                defer(w, "click", true);
                let x = v.get("x").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let y = v.get("y").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let button = v.get("button").and_then(|x| x.as_i64()).unwrap_or(1);
                format!("{{\"id\":{id},\"ok\":true,\"x\":{x},\"y\":{y},\"button\":{button}}}\n")
            }
            Err(e) => fail(&e),
        },
        "type_text" => {
            let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("");
            let targeted = sel.id != 0 || !sel.name.is_empty() || !sel.role.is_empty() || !sel.placeholder.is_empty() || !sel.klass.is_empty();
            let w = if targeted {
                match resolve_one(&sel) {
                    Ok(w) => w,
                    Err(e) => return fail(&e),
                }
            } else {
                std::ptr::null_mut()
            };
            type_into(w, text);
            format!("{{\"id\":{id},\"ok\":true}}\n")
        }
        _ => fail("unknown cmd"),
    }
}

struct Job {
    line: String,
    reply: Mutex<Option<String>>,
    cv: Condvar,
}

extern "C" fn job_idle(data: *mut c_void) -> i32 {
    let job = unsafe { &*(data as *const Job) };
    let reply = handle(&job.line);
    let mut slot = job.reply.lock().unwrap();
    *slot = Some(reply);
    job.cv.notify_one();
    0
}

fn dispatch(line: &str) -> String {
    let job = Box::new(Job {
        line: line.to_string(),
        reply: Mutex::new(None),
        cv: Condvar::new(),
    });
    let ptr = Box::into_raw(job);
    let idle: unsafe extern "C" fn(extern "C" fn(*mut c_void) -> i32, *mut c_void) -> u32 =
        match sym("g_idle_add") {
            Some(f) => f,
            None => return "{\"ok\":false,\"error\":\"no main loop\"}\n".into(),
        };
    unsafe { idle(job_idle, ptr as *mut c_void) };
    let job = unsafe { &*ptr };
    let mut slot = job.reply.lock().unwrap();
    while slot.is_none() {
        slot = job.cv.wait(slot).unwrap();
    }
    let reply = slot.take().unwrap();
    drop(slot);
    unsafe { drop(Box::from_raw(ptr)) };
    reply
}

fn client_thread(fd: i32) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        let n = unsafe { libc::read(fd, tmp.as_mut_ptr() as *mut c_void, tmp.len()) };
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n as usize]);
        while let Some(i) = buf.iter().position(|b| *b == b'\n') {
            let line = String::from_utf8_lossy(&buf[..i]).to_string();
            let reply = dispatch(line.trim());
            let bytes = reply.as_bytes();
            let mut off = 0;
            while off < bytes.len() {
                let w = unsafe {
                    libc::write(fd, bytes[off..].as_ptr() as *const c_void, bytes.len() - off)
                };
                if w <= 0 {
                    break;
                }
                off += w as usize;
            }
            buf.drain(..=i);
        }
        if buf.len() > 1024 * 1024 {
            break;
        }
    }
    unsafe { libc::close(fd) };
}

fn default_sock() -> String {
    if let Ok(p) = std::env::var("GTK_REMOTE_SOCK") {
        if !p.is_empty() {
            return p;
        }
    }
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc::getuid() }));
    let dir = format!("{runtime}/gtk-remote");
    let _ = std::fs::create_dir_all(&dir);
    format!("{dir}/{}.sock", std::process::id())
}

fn prgname() -> String {
    let f: Option<unsafe extern "C" fn() -> *const c_char> = sym("g_get_prgname");
    if let Some(f) = f {
        let p = unsafe { f() };
        if !p.is_null() {
            return unsafe { cstr(p) }.to_string();
        }
    }
    std::env::args().next().unwrap_or_else(|| "?".into())
}

extern "C" fn start_server(_data: *mut c_void) -> i32 {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return 0;
    }
    let path = default_sock();
    let _ = std::fs::remove_file(&path);
    *sock_path_global().lock().unwrap() = path.clone();
    std::env::set_var("GTK_REMOTE_APP", prgname());

    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        eprintln!("gtk-remote: socket() failed");
        return 0;
    }
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let bytes = path.as_bytes();
    let n = bytes.len().min(addr.sun_path.len() - 1);
    for (i, b) in bytes.iter().take(n).enumerate() {
        addr.sun_path[i] = *b as libc::c_char;
    }
    let rc = unsafe {
        libc::bind(
            fd,
            &addr as *const libc::sockaddr_un as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
        )
    };
    if rc != 0 {
        eprintln!("gtk-remote: cannot listen on {path}");
        unsafe { libc::close(fd) };
        return 0;
    }
    unsafe {
        libc::listen(fd, 16);
        libc::chmod(CString::new(path.clone()).unwrap().as_ptr(), 0o600);
    }
    eprintln!(
        "gtk-remote: listening on {path} app={} pid={} toolkit=gtk4",
        prgname(),
        std::process::id()
    );
    std::thread::spawn(move || loop {
        let cfd = unsafe { libc::accept(fd, std::ptr::null_mut(), std::ptr::null_mut()) };
        if cfd < 0 {
            break;
        }
        std::thread::spawn(move || client_thread(cfd));
    });
    0
}

#[no_mangle]
pub extern "C" fn gtk_remote_backend_arm() {
    static ARMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if ARMED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let idle: unsafe extern "C" fn(extern "C" fn(*mut c_void) -> i32, *mut c_void) -> u32 =
        match sym("g_idle_add") {
            Some(f) => f,
            None => {
                eprintln!("gtk-remote: g_idle_add missing");
                return;
            }
        };
    unsafe { idle(start_server, std::ptr::null_mut()) };
}
