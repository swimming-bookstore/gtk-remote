//! GTK 3 backend. Loaded only into processes that mapped libgtk-3.
//! GTK 3 and ATK are resolved with dlsym so this crate does not need GTK headers.
//!
//! Exports: void gtk_remote_backend_arm(void);

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_void};
use std::sync::{Condvar, Mutex};

use serde_json::Value;

const ATK_STATE_CHECKED: i32 = 4;
const ATK_STATE_EDITABLE: i32 = 6;
const ATK_STATE_FOCUSABLE: i32 = 10;
const ATK_STATE_FOCUSED: i32 = 11;
const ATK_STATE_SENSITIVE: i32 = 22;
const ATK_STATE_VISIBLE: i32 = 28;

struct Gtk {
    lib: libloading::os::unix::Library,
}

fn gtk() -> Option<&'static Gtk> {
    static G: std::sync::OnceLock<Option<Gtk>> = std::sync::OnceLock::new();
    G.get_or_init(|| {
        let flags = libc::RTLD_LAZY | libc::RTLD_GLOBAL;
        let lib = unsafe { libloading::os::unix::Library::open(Some("libgtk-3.so.0"), flags) }.ok()?;
        Some(Gtk { lib })
    })
    .as_ref()
}

fn atk() -> Option<&'static libloading::os::unix::Library> {
    static A: std::sync::OnceLock<Option<libloading::os::unix::Library>> = std::sync::OnceLock::new();
    A.get_or_init(|| {
        unsafe { libloading::os::unix::Library::open(Some("libatk-1.0.so.0"), libc::RTLD_LAZY | libc::RTLD_GLOBAL) }.ok()
    })
    .as_ref()
}

fn gsym<T>(name: &str) -> Option<T> {
    let c = CString::new(name).ok()?;
    let p = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c.as_ptr()) };
    if p.is_null() {
        None
    } else {
        Some(unsafe { std::mem::transmute_copy(&p) })
    }
}

fn gtk_sym<T: Copy>(name: &[u8]) -> Option<T> {
    let lib = gtk()?;
    let s: libloading::os::unix::Symbol<T> = unsafe { lib.lib.get(name) }.ok()?;
    Some(*s)
}

fn atk_sym<T: Copy>(name: &[u8]) -> Option<T> {
    let lib = atk()?;
    let s: libloading::os::unix::Symbol<T> = unsafe { lib.get(name) }.ok()?;
    Some(*s)
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
    if hay.is_empty() {
        return false;
    }
    hay.to_ascii_lowercase().contains(&needle.to_ascii_lowercase())
}

fn g_type_of(get_type: Option<unsafe extern "C" fn() -> u64>) -> u64 {
    match get_type {
        Some(f) => unsafe { f() },
        None => 0,
    }
}

fn is_gobject(obj: *mut c_void) -> bool {
    if obj.is_null() {
        return false;
    }
    let check: unsafe extern "C" fn(*mut c_void, u64) -> i32 = match gsym("g_type_check_instance_is_a") {
        Some(f) => f,
        None => return false,
    };
    let ty: u64 = match gsym::<unsafe extern "C" fn() -> u64>("g_object_get_type") {
        Some(f) => unsafe { f() },
        None => return false,
    };
    unsafe { check(obj, ty) != 0 }
}

fn is_a(obj: *mut c_void, get_type: Option<unsafe extern "C" fn() -> u64>) -> bool {
    if !is_gobject(obj) {
        return false;
    }
    let t = g_type_of(get_type);
    if t == 0 {
        return false;
    }
    let check: unsafe extern "C" fn(*mut c_void, u64) -> i32 = match gsym("g_type_check_instance_is_a") {
        Some(f) => f,
        None => return false,
    };
    unsafe { check(obj, t) != 0 }
}

fn type_name(obj: *mut c_void) -> String {
    if !is_gobject(obj) {
        return "?".into();
    }
    let name_of: unsafe extern "C" fn(u64) -> *const c_char = match gsym("g_type_name") {
        Some(f) => f,
        None => return "?".into(),
    };
    let class = unsafe { *(obj as *mut *mut u64) };
    if class.is_null() {
        return "?".into();
    }
    unsafe { cstr(name_of(*class)) }.to_string()
}

fn g_free(p: *mut c_void) {
    if p.is_null() {
        return;
    }
    if let Some(f) = gsym::<unsafe extern "C" fn(*mut c_void)>("g_free") {
        unsafe { f(p) };
    }
}

fn g_object_unref(p: *mut c_void) {
    if p.is_null() {
        return;
    }
    if let Some(f) = gsym::<unsafe extern "C" fn(*mut c_void)>("g_object_unref") {
        unsafe { f(p) };
    }
}

struct Ids {
    by_obj: HashMap<usize, u32>,
    by_id: HashMap<u32, usize>,
    next: u32,
}

fn ids() -> &'static Mutex<Ids> {
    static IDS: std::sync::OnceLock<Mutex<Ids>> = std::sync::OnceLock::new();
    IDS.get_or_init(|| {
        Mutex::new(Ids {
            by_obj: HashMap::new(),
            by_id: HashMap::new(),
            next: 1,
        })
    })
}

unsafe extern "C" fn on_gone(data: *mut c_void, old: *mut c_void) {
    let id = data as usize as u32;
    if let Ok(mut t) = ids().lock() {
        t.by_id.remove(&id);
        t.by_obj.remove(&(old as usize));
    }
}

fn obj_id(obj: *mut c_void) -> u32 {
    if obj.is_null() {
        return 0;
    }
    let mut t = ids().lock().unwrap();
    if let Some(id) = t.by_obj.get(&(obj as usize)).copied() {
        return id;
    }
    let id = t.next;
    t.next += 1;
    t.by_obj.insert(obj as usize, id);
    t.by_id.insert(id, obj as usize);
    drop(t);
    if let Some(weak) = gsym::<unsafe extern "C" fn(*mut c_void, unsafe extern "C" fn(*mut c_void, *mut c_void), *mut c_void)>("g_object_weak_ref") {
        unsafe { weak(obj, on_gone, id as usize as *mut c_void) };
    }
    id
}

fn obj_from_id(id: u32) -> *mut c_void {
    if id == 0 {
        return std::ptr::null_mut();
    }
    ids().lock().unwrap().by_id.get(&id).copied().unwrap_or(0) as *mut c_void
}

fn atk_name(obj: *mut c_void) -> String {
    if !is_gobject(obj) {
        return String::new();
    }
    let f: unsafe extern "C" fn(*mut c_void) -> *const c_char = match atk_sym(b"atk_object_get_name") {
        Some(f) => f,
        None => return String::new(),
    };
    unsafe { cstr(f(obj)) }.to_string()
}

fn atk_desc(obj: *mut c_void) -> String {
    if !is_gobject(obj) {
        return String::new();
    }
    let f: unsafe extern "C" fn(*mut c_void) -> *const c_char = match atk_sym(b"atk_object_get_description") {
        Some(f) => f,
        None => return String::new(),
    };
    unsafe { cstr(f(obj)) }.to_string()
}

fn role_slug(obj: *mut c_void) -> String {
    if !is_gobject(obj) {
        return "widget".into();
    }
    let get: unsafe extern "C" fn(*mut c_void) -> i32 = match atk_sym(b"atk_object_get_role") {
        Some(f) => f,
        None => return "widget".into(),
    };
    let name: unsafe extern "C" fn(i32) -> *const c_char = match atk_sym(b"atk_role_get_name") {
        Some(f) => f,
        None => return "widget".into(),
    };
    let role = unsafe { get(obj) };
    let n = unsafe { cstr(name(role)) };
    if n.is_empty() {
        return "widget".into();
    }
    n.chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c == ' ' { '-' } else { c }
        })
        .collect()
}

fn state_has(obj: *mut c_void, state: i32) -> bool {
    let r: unsafe extern "C" fn(*mut c_void) -> *mut c_void = match atk_sym(b"atk_object_ref_state_set") {
        Some(f) => f,
        None => return false,
    };
    let c: unsafe extern "C" fn(*mut c_void, i32) -> i32 = match atk_sym(b"atk_state_set_contains_state") {
        Some(f) => f,
        None => return false,
    };
    let set = unsafe { r(obj) };
    if set.is_null() {
        return false;
    }
    let has = unsafe { c(set, state) } != 0;
    g_object_unref(set);
    has
}

fn atk_text_type() -> Option<unsafe extern "C" fn() -> u64> {
    atk_sym(b"atk_text_get_type")
}
fn atk_editable_type() -> Option<unsafe extern "C" fn() -> u64> {
    atk_sym(b"atk_editable_text_get_type")
}
fn atk_action_type() -> Option<unsafe extern "C" fn() -> u64> {
    atk_sym(b"atk_action_get_type")
}
fn atk_value_type() -> Option<unsafe extern "C" fn() -> u64> {
    atk_sym(b"atk_value_get_type")
}
fn gtk_entry_type() -> Option<unsafe extern "C" fn() -> u64> {
    gtk_sym(b"gtk_entry_get_type")
}
fn gtk_text_view_type() -> Option<unsafe extern "C" fn() -> u64> {
    gtk_sym(b"gtk_text_view_get_type")
}
fn gtk_window_type() -> Option<unsafe extern "C" fn() -> u64> {
    gtk_sym(b"gtk_window_get_type")
}

fn object_text(obj: *mut c_void) -> Option<String> {
    if !is_a(obj, atk_text_type()) {
        return None;
    }
    let get: unsafe extern "C" fn(*mut c_void, i32, i32) -> *mut c_char = atk_sym(b"atk_text_get_text")?;
    let count: Option<unsafe extern "C" fn(*mut c_void) -> i32> = atk_sym(b"atk_text_get_character_count");
    let mut n = count.map(|f| unsafe { f(obj) }).unwrap_or(4096);
    if n < 0 {
        n = 0;
    }
    if n > 8192 {
        n = 8192;
    }
    let p = unsafe { get(obj, 0, n) };
    if p.is_null() {
        return Some(String::new());
    }
    let s = unsafe { cstr(p) }.to_string();
    g_free(p as *mut c_void);
    Some(s)
}

fn object_value(obj: *mut c_void) -> Option<String> {
    if is_a(obj, atk_value_type()) {
        if let Some(f) = atk_sym::<unsafe extern "C" fn(*mut c_void, *mut c_double, *mut *mut c_char)>(
            b"atk_value_get_value_and_text",
        ) {
            let mut v = 0.0;
            let mut text: *mut c_char = std::ptr::null_mut();
            unsafe { f(obj, &mut v, &mut text) };
            if !text.is_null() {
                let s = unsafe { cstr(text) }.to_string();
                g_free(text as *mut c_void);
                if !s.is_empty() {
                    return Some(s);
                }
            }
            return Some(format!("{v}"));
        }
    }
    if state_has(obj, ATK_STATE_CHECKED) {
        return Some("true".into());
    }
    object_text(obj)
}

fn widget_of(obj: *mut c_void) -> *mut c_void {
    let f: unsafe extern "C" fn(*mut c_void) -> *mut c_void = match gtk_sym(b"gtk_accessible_get_widget") {
        Some(f) => f,
        None => return std::ptr::null_mut(),
    };
    unsafe { f(obj) }
}

struct Selector {
    name: String,
    role: String,
    description: String,
    klass: String,
    id: u32,
    index: i32,
    targeted: bool,
}

fn sel_from(v: &Value) -> Selector {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let mut id = v.get("widget").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    if id == 0 {
        id = v.get("target").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    }
    let name = s("name");
    let role = s("role");
    let klass = s("class");
    let targeted = id != 0 || !name.is_empty() || !role.is_empty() || !klass.is_empty();
    Selector {
        name,
        role,
        description: s("description"),
        klass,
        id,
        index: v.get("index").and_then(|x| x.as_i64()).unwrap_or(0) as i32,
        targeted,
    }
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
    let textish = substr_ci(want, "textfield")
        || substr_ci(want, "text field")
        || substr_ci(want, "entry")
        || substr_ci(want, "textbox");
    if textish && (substr_ci(have, "text") || substr_ci(have, "entry")) {
        return true;
    }
    if substr_ci(want, "checkbox") && substr_ci(have, "check") {
        return true;
    }
    false
}

fn object_matches(obj: *mut c_void, s: &Selector) -> bool {
    if obj.is_null() {
        return false;
    }
    if s.id != 0 {
        return obj_id(obj) == s.id;
    }
    let role = role_slug(obj);
    if !role_match(&s.role, &role) && !role_match(&s.role, &type_name(obj)) {
        return false;
    }
    if !s.name.is_empty() {
        let mut ok = substr_ci(&atk_name(obj), &s.name);
        // Labels are identified by their text. Other widgets (a dialog whose
        // body contains the word "no") must not match on value.
        if !ok && substr_ci(&role, "label") {
            if let Some(val) = object_text(obj) {
                ok = substr_ci(&val, &s.name);
            }
        }
        if !ok {
            return false;
        }
    }
    if !s.description.is_empty() && !substr_ci(&atk_desc(obj), &s.description) {
        return false;
    }
    if !s.klass.is_empty() && !substr_ci(&type_name(obj), &s.klass) {
        return false;
    }
    true
}

fn append_actions(o: &mut String, obj: *mut c_void) {
    o.push_str("\"actions\":[");
    let mut n = 0;
    let mut add = |name: &str| {
        if n > 0 {
            o.push(',');
        }
        n += 1;
        o.push('"');
        o.push_str(&escape(name));
        o.push('"');
    };
    if state_has(obj, ATK_STATE_FOCUSABLE) {
        add("focus");
    }
    if is_a(obj, atk_action_type()) {
        if let Some(count_fn) = atk_sym::<unsafe extern "C" fn(*mut c_void) -> i32>(b"atk_action_get_n_actions") {
            let name_fn: Option<unsafe extern "C" fn(*mut c_void, i32) -> *const c_char> = atk_sym(b"atk_action_get_name");
            let count = unsafe { count_fn(obj) };
            for i in 0..count {
                let an = name_fn.map(|f| unsafe { cstr(f(obj, i)) }).unwrap_or("");
                if !an.is_empty() {
                    add(an);
                }
            }
        }
    }
    if is_a(obj, atk_editable_type()) || state_has(obj, ATK_STATE_EDITABLE) {
        add("set-text");
        add("type");
    }
    if is_a(obj, atk_value_type()) {
        add("set-value");
    }
    o.push(']');
}

fn n_children(obj: *mut c_void) -> i32 {
    if !is_gobject(obj) {
        return 0;
    }
    let f: unsafe extern "C" fn(*mut c_void) -> i32 = match atk_sym(b"atk_object_get_n_accessible_children") {
        Some(f) => f,
        None => return 0,
    };
    let n = unsafe { f(obj) };
    n.clamp(0, 512)
}

fn child_at(obj: *mut c_void, i: i32) -> *mut c_void {
    if !is_gobject(obj) {
        return std::ptr::null_mut();
    }
    let f: unsafe extern "C" fn(*mut c_void, i32) -> *mut c_void = match atk_sym(b"atk_object_ref_accessible_child") {
        Some(f) => f,
        None => return std::ptr::null_mut(),
    };
    unsafe { f(obj, i) }
}

fn emit_object(o: &mut String, obj: *mut c_void, depth: i32, max_depth: i32, visible_only: bool) {
    if obj.is_null() {
        o.push_str("null");
        return;
    }
    let name = atk_name(obj);
    let desc = atk_desc(obj);
    let role = role_slug(obj);
    let value = object_value(obj);
    let klass = type_name(obj);
    o.push_str(&format!(
        "{{\"id\":{},\"role\":\"{}\",\"class\":\"{}\",\"css\":\"\",\"name\":\"{}\",\"placeholder\":\"\",\"description\":\"{}\",",
        obj_id(obj),
        escape(&role),
        escape(&klass),
        escape(&name),
        escape(&desc)
    ));
    match &value {
        Some(v) => o.push_str(&format!("\"value\":\"{}\",", escape(v))),
        None => o.push_str("\"value\":null,"),
    }
    o.push_str(&format!(
        "\"focused\":{},\"visible\":{},\"sensitive\":{},\"can_focus\":{},\"bounds\":{{\"x\":0,\"y\":0,\"width\":0,\"height\":0}},",
        state_has(obj, ATK_STATE_FOCUSED),
        state_has(obj, ATK_STATE_VISIBLE),
        state_has(obj, ATK_STATE_SENSITIVE),
        state_has(obj, ATK_STATE_FOCUSABLE)
    ));
    append_actions(o, obj);
    o.push_str(",\"children\":[");
    if depth < max_depth {
        let n = n_children(obj);
        let mut written = 0;
        for i in 0..n {
            let child = child_at(obj, i);
            if child.is_null() {
                continue;
            }
            if visible_only && !state_has(child, ATK_STATE_VISIBLE) {
                g_object_unref(child);
                continue;
            }
            if written > 0 {
                o.push(',');
            }
            written += 1;
            emit_object(o, child, depth + 1, max_depth, visible_only);
            g_object_unref(child);
        }
    }
    o.push_str("]}");
}

fn collect(obj: *mut c_void, s: &Selector, out: &mut Vec<*mut c_void>, limit: i32) {
    if obj.is_null() {
        return;
    }
    if object_matches(obj, s) {
        out.push(obj);
        if limit > 0 && out.len() as i32 >= limit {
            return;
        }
    }
    for i in 0..n_children(obj) {
        let child = child_at(obj, i);
        if child.is_null() {
            continue;
        }
        collect(child, s, out, limit);
        g_object_unref(child);
        if limit > 0 && out.len() as i32 >= limit {
            return;
        }
    }
}

fn toplevel_accessibles() -> Vec<*mut c_void> {
    let list_fn: unsafe extern "C" fn() -> *mut c_void = match gtk_sym(b"gtk_window_list_toplevels") {
        Some(f) => f,
        None => return Vec::new(),
    };
    let acc: unsafe extern "C" fn(*mut c_void) -> *mut c_void = match gtk_sym(b"gtk_widget_get_accessible") {
        Some(f) => f,
        None => return Vec::new(),
    };
    let head = unsafe { list_fn() };
    let mut list = head;
    let mut out = Vec::new();
    while !list.is_null() {
        let data = unsafe { *(list as *mut *mut c_void) };
        let next = unsafe { *((list as *mut u8).add(std::mem::size_of::<*mut c_void>()) as *mut *mut c_void) };
        if !data.is_null() {
            let a = unsafe { acc(data) };
            if !a.is_null() {
                out.push(a);
            }
        }
        list = next;
    }
    if let Some(free_list) = gsym::<unsafe extern "C" fn(*mut c_void)>("g_list_free") {
        unsafe { free_list(head) };
    }
    out
}

fn resolve_one(s: &Selector) -> Result<*mut c_void, String> {
    if s.id != 0 {
        let o = obj_from_id(s.id);
        if o.is_null() {
            return Err(format!("no widget with id {} (it may have been destroyed)", s.id));
        }
        return Ok(o);
    }
    let mut hits = Vec::new();
    for top in toplevel_accessibles() {
        collect(top, s, &mut hits, 0);
    }
    if !s.name.is_empty() {
        let exact: Vec<*mut c_void> = hits
            .iter()
            .copied()
            .filter(|o| atk_name(*o).eq_ignore_ascii_case(&s.name))
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

fn do_named_action(obj: *mut c_void, want: &str) -> bool {
    if !is_a(obj, atk_action_type()) {
        return false;
    }
    let count_fn: unsafe extern "C" fn(*mut c_void) -> i32 = match atk_sym(b"atk_action_get_n_actions") {
        Some(f) => f,
        None => return false,
    };
    let name_fn: unsafe extern "C" fn(*mut c_void, i32) -> *const c_char = match atk_sym(b"atk_action_get_name") {
        Some(f) => f,
        None => return false,
    };
    let do_fn: unsafe extern "C" fn(*mut c_void, i32) -> i32 = match atk_sym(b"atk_action_do_action") {
        Some(f) => f,
        None => return false,
    };
    let n = unsafe { count_fn(obj) };
    for i in 0..n {
        let an = unsafe { cstr(name_fn(obj, i)) };
        let hit = eq_ci(an, want)
            || (eq_ci(want, "activate") && eq_ci(an, "click"))
            || (eq_ci(want, "press") && (eq_ci(an, "press") || eq_ci(an, "click")))
            || (eq_ci(want, "AXPress") && eq_ci(an, "click"))
            || (eq_ci(want, "toggle") && (eq_ci(an, "toggle") || eq_ci(an, "click")));
        if hit {
            return unsafe { do_fn(obj, i) } != 0;
        }
    }
    if n > 0 && (eq_ci(want, "click") || eq_ci(want, "press") || eq_ci(want, "activate") || eq_ci(want, "AXPress")) {
        return unsafe { do_fn(obj, 0) } != 0;
    }
    false
}

fn table_cell_at(table: *mut c_void, row: i32, col: i32) -> *mut c_void {
    if !is_gobject(table) {
        return std::ptr::null_mut();
    }
    let f: unsafe extern "C" fn(*mut c_void, i32, i32) -> *mut c_void = match atk_sym(b"atk_table_ref_at") {
        Some(f) => f,
        None => return std::ptr::null_mut(),
    };
    unsafe { f(table, row, col) }
}

fn cell_addr(name: &str) -> Option<(i32, i32)> {
    let name = name.trim();
    let mut col: i32 = 0;
    let mut i = 0;
    let b = name.as_bytes();
    if b.is_empty() || !b[0].is_ascii_alphabetic() {
        return None;
    }
    while i < b.len() && b[i].is_ascii_alphabetic() {
        col = col * 26 + (b[i].to_ascii_uppercase() - b'A') as i32 + 1;
        i += 1;
    }
    if col == 0 || i == 0 || i == b.len() {
        return None;
    }
    let row: i32 = name[i..].parse().ok()?;
    if row < 1 {
        return None;
    }
    Some((row - 1, col - 1))
}

fn resolve_cell(v: &Value) -> Option<*mut c_void> {
    let name = v.get("cell").and_then(|x| x.as_str()).unwrap_or("");
    let (row, col) = cell_addr(name)?;
    let mut table = std::ptr::null_mut();
    if let Some(id) = v.get("table").and_then(|x| x.as_u64()) {
        let obj = obj_from_id(id as u32);
        if role_slug(obj) == "table" {
            table = obj;
        }
    }
    if table.is_null() {
        let sel = Selector {
            name: "Sheet Sheet1".into(),
            role: "table".into(),
            description: String::new(),
            klass: String::new(),
            id: 0,
            index: 0,
            targeted: true,
        };
        table = resolve_one(&sel).unwrap_or(std::ptr::null_mut());
    }
    if table.is_null() {
        return None;
    }
    let cell = table_cell_at(table, row, col);
    if cell.is_null() || !is_gobject(cell) {
        None
    } else {
        Some(cell)
    }
}

fn set_object_text(obj: *mut c_void, text: &str) -> Result<(), String> {
    let c = CString::new(text).unwrap_or_else(|_| CString::new("").unwrap());
    if is_a(obj, atk_editable_type()) {
        if let Some(f) = atk_sym::<unsafe extern "C" fn(*mut c_void, *const c_char) -> i32>(b"atk_editable_text_set_text_contents") {
            if unsafe { f(obj, c.as_ptr()) } != 0 {
                return Ok(());
            }
        }
    }
    // LibreOffice cells advertise set-text and an "edit" action, but the
    // accessible is not AtkEditableText. Commit through the action, then the
    // value interface, which is what the formula bar reads back.
    if do_named_action(obj, "edit") {
        if let Some(f) = atk_sym::<unsafe extern "C" fn(*mut c_void, *const c_char) -> i32>(b"atk_editable_text_set_text_contents") {
            if unsafe { f(obj, c.as_ptr()) } != 0 {
                return Ok(());
            }
        }
    }
    if is_a(obj, atk_text_type()) {
        if let Some(setv) = gsym::<unsafe extern "C" fn(*mut c_void, *const GValue) -> i32>("atk_value_set_value") {
            if let Some(init) = gsym::<unsafe extern "C" fn(*mut GValue, u64) -> *mut GValue>("g_value_init") {
                if let Some(sets) = gsym::<unsafe extern "C" fn(*mut GValue, *const c_char)>("g_value_set_string") {
                    let ty = gsym::<unsafe extern "C" fn() -> u64>("g_string_get_type").map(|f| unsafe { f() }).unwrap_or(0);
                    // G_TYPE_STRING is fundamental type id 16<<2 = 64 on this ABI.
                    let ty = if ty != 0 { ty } else { 64 };
                    let mut v = GValue::default();
                    unsafe { init(&mut v, ty) };
                    unsafe { sets(&mut v, c.as_ptr()) };
                    let ok = unsafe { setv(obj, &v) } != 0;
                    if let Some(unset) = gsym::<unsafe extern "C" fn(*mut GValue)>("g_value_unset") {
                        unsafe { unset(&mut v) };
                    }
                    if ok {
                        return Ok(());
                    }
                }
            }
        }
    }
    let w = widget_of(obj);
    if !w.is_null() && is_a(w, gtk_entry_type()) {
        if let Some(f) = gtk_sym::<unsafe extern "C" fn(*mut c_void, *const c_char)>(b"gtk_entry_set_text") {
            unsafe { f(w, c.as_ptr()) };
            return Ok(());
        }
    }
    if !w.is_null() && is_a(w, gtk_text_view_type()) {
        if let Some(get_buf) = gtk_sym::<unsafe extern "C" fn(*mut c_void) -> *mut c_void>(b"gtk_text_view_get_buffer") {
            if let Some(set) = gtk_sym::<unsafe extern "C" fn(*mut c_void, *const c_char, i32)>(b"gtk_text_buffer_set_text") {
                let buf = unsafe { get_buf(w) };
                if !buf.is_null() {
                    unsafe { set(buf, c.as_ptr(), -1) };
                    return Ok(());
                }
            }
        }
    }
    Err(format!("{} is not an editable text widget", type_name(obj)))
}

fn set_object_value(obj: *mut c_void, text: &str) -> Result<(), String> {
    if is_a(obj, atk_editable_type()) {
        return set_object_text(obj, text);
    }
    let w = widget_of(obj);
    if !w.is_null() && is_a(w, gtk_entry_type()) {
        return set_object_text(obj, text);
    }
    if is_a(obj, atk_value_type()) {
        if let Some(setv) = gsym::<unsafe extern "C" fn(*mut c_void, *const GValue) -> i32>("atk_value_set_value") {
            let mut v = GValue::default();
            if let Some(init) = gsym::<unsafe extern "C" fn(*mut GValue, u64) -> *mut GValue>("g_value_init") {
                if let Some(setd) = gsym::<unsafe extern "C" fn(*mut GValue, c_double)>("g_value_set_double") {
                    let gdouble: u64 = gsym::<unsafe extern "C" fn() -> u64>("g_double_get_type").map(|f| unsafe { f() }).unwrap_or(0);
                    // G_TYPE_DOUBLE is a fundamental type, not a get_type. Use the constant 15<<2 on this ABI? Safer: g_value_init with the type from a known double.
                    let ty = if gdouble != 0 { gdouble } else { 60 };
                    unsafe { init(&mut v, ty) };
                    unsafe { setd(&mut v, text.parse().unwrap_or(0.0)) };
                    unsafe { setv(obj, &v) };
                    if let Some(unset) = gsym::<unsafe extern "C" fn(*mut GValue)>("g_value_unset") {
                        unsafe { unset(&mut v) };
                    }
                    return Ok(());
                }
            }
        }
    }
    Err(format!("cannot set value on {}", type_name(obj)))
}

#[repr(C)]
#[derive(Default)]
struct GValue {
    g_type: u64,
    data: [u64; 2],
}

fn select_text(obj: *mut c_void, start: i32, end: i32) -> Result<(), String> {
    let w = widget_of(obj);
    if !w.is_null() && is_a(w, gtk_text_view_type()) {
        if let Some(get_buf) = gtk_sym::<unsafe extern "C" fn(*mut c_void) -> *mut c_void>(b"gtk_text_view_get_buffer") {
            if let Some(select) = gtk_sym::<unsafe extern "C" fn(*mut c_void, *const GtkTextIter, *const GtkTextIter)>(
                b"gtk_text_buffer_select_range",
            ) {
                if let Some(get_iter) = gtk_sym::<unsafe extern "C" fn(*mut c_void, *mut GtkTextIter, i32)>(
                    b"gtk_text_buffer_get_iter_at_offset",
                ) {
                    let buf = unsafe { get_buf(w) };
                    if !buf.is_null() {
                        let mut a = GtkTextIter { raw: [0; 80] };
                        let mut b = GtkTextIter { raw: [0; 80] };
                        unsafe { get_iter(buf, &mut a, start) };
                        unsafe { get_iter(buf, &mut b, end) };
                        unsafe { select(buf, &a, &b) };
                        return Ok(());
                    }
                }
            }
        }
    }
    if !w.is_null() && is_a(w, gtk_entry_type()) {
        if let Some(sel) = gtk_sym::<unsafe extern "C" fn(*mut c_void, i32, i32)>(b"gtk_editable_select_region") {
            unsafe { sel(w, start, end) };
            return Ok(());
        }
    }
    if let Some(sel) = atk_sym::<unsafe extern "C" fn(*mut c_void, i32, i32, i32) -> i32>(b"atk_text_set_selection") {
        if unsafe { sel(obj, 0, start, end) } != 0 {
            return Ok(());
        }
    }
    if let Some(add) = atk_sym::<unsafe extern "C" fn(*mut c_void, i32, i32) -> i32>(b"atk_text_add_selection") {
        let _ = atk_sym::<unsafe extern "C" fn(*mut c_void, i32) -> i32>(b"atk_text_remove_selection")
            .map(|rm| unsafe { rm(obj, 0) });
        if unsafe { add(obj, start, end) } != 0 {
            return Ok(());
        }
    }
    Err(format!("cannot select text on {}", type_name(obj)))
}

#[repr(C)]
struct GtkTextIter {
    raw: [u8; 80],
}

fn type_into(obj: *mut c_void, text: &str) {
    let c = CString::new(text).unwrap_or_else(|_| CString::new("").unwrap());
    let w = widget_of(obj);
    // Insert on the buffer. atk_editable_text_insert_text re-enters the main
    // loop on GtkTextView and the idle that is waiting for it never finishes.
    if !w.is_null() && is_a(w, gtk_text_view_type()) {
        if let Some(get_buf) = gtk_sym::<unsafe extern "C" fn(*mut c_void) -> *mut c_void>(b"gtk_text_view_get_buffer") {
            if let Some(ins) = gtk_sym::<unsafe extern "C" fn(*mut c_void, *const c_char, i32)>(b"gtk_text_buffer_insert_at_cursor") {
                let buf = unsafe { get_buf(w) };
                if !buf.is_null() {
                    unsafe { ins(buf, c.as_ptr(), -1) };
                    return;
                }
            }
        }
    }
    if !w.is_null() && is_a(w, gtk_entry_type()) {
        let get: Option<unsafe extern "C" fn(*mut c_void) -> *const c_char> = gtk_sym(b"gtk_entry_get_text");
        let set: Option<unsafe extern "C" fn(*mut c_void, *const c_char)> = gtk_sym(b"gtk_entry_set_text");
        if let (Some(get), Some(set)) = (get, set) {
            let cur = unsafe { cstr(get(w)) };
            let next = format!("{cur}{text}");
            if let Ok(cs) = CString::new(next) {
                unsafe { set(w, cs.as_ptr()) };
                return;
            }
        }
    }
    if is_a(obj, atk_editable_type()) {
        if let Some(ins) = atk_sym::<unsafe extern "C" fn(*mut c_void, *const c_char, i32, *mut i32)>(b"atk_editable_text_insert_text") {
            let count: Option<unsafe extern "C" fn(*mut c_void) -> i32> = atk_sym(b"atk_text_get_character_count");
            let mut pos = count.map(|f| unsafe { f(obj) }).unwrap_or(0);
            unsafe { ins(obj, c.as_ptr(), text.len() as i32, &mut pos) };
        }
    }
}

struct Deferred {
    obj: usize,
    action: String,
    text: String,
    kind: i32,
}

extern "C" fn run_deferred(data: *mut c_void) -> i32 {
    let d = unsafe { Box::from_raw(data as *mut Deferred) };
    let obj = d.obj as *mut c_void;
    if !obj.is_null() {
        if d.kind == 0 {
            let _ = do_named_action(obj, &d.action);
        } else if d.kind == 1 {
            let _ = set_object_text(obj, &d.text);
        } else if d.kind == 2 {
            type_into(obj, &d.text);
        } else if d.kind == 3 {
            let start = d.action.parse::<i32>().unwrap_or(0);
            let end = d.text.parse::<i32>().unwrap_or(start);
            let _ = select_text(obj, start, end);
        }
    }
    0
}

fn defer(obj: *mut c_void, kind: i32, action: &str, text: &str) {
    let d = Box::new(Deferred {
        obj: obj as usize,
        action: action.to_string(),
        text: text.to_string(),
        kind,
    });
    let idle: unsafe extern "C" fn(extern "C" fn(*mut c_void) -> i32, *mut c_void) -> u32 = match gsym("g_idle_add") {
        Some(f) => f,
        None => return,
    };
    unsafe { idle(run_deferred, Box::into_raw(d) as *mut c_void) };
}

fn sock_path_global() -> &'static Mutex<String> {
    static P: std::sync::OnceLock<Mutex<String>> = std::sync::OnceLock::new();
    P.get_or_init(|| Mutex::new(String::new()))
}

fn focused_object() -> *mut c_void {
    let list_fn: unsafe extern "C" fn() -> *mut c_void = match gtk_sym(b"gtk_window_list_toplevels") {
        Some(f) => f,
        None => return std::ptr::null_mut(),
    };
    let get_focus: unsafe extern "C" fn(*mut c_void) -> *mut c_void = match gtk_sym(b"gtk_window_get_focus") {
        Some(f) => f,
        None => return std::ptr::null_mut(),
    };
    let acc: unsafe extern "C" fn(*mut c_void) -> *mut c_void = match gtk_sym(b"gtk_widget_get_accessible") {
        Some(f) => f,
        None => return std::ptr::null_mut(),
    };
    let head = unsafe { list_fn() };
    let mut list = head;
    let mut found = std::ptr::null_mut();
    while !list.is_null() {
        let data = unsafe { *(list as *mut *mut c_void) };
        let next = unsafe { *((list as *mut u8).add(std::mem::size_of::<*mut c_void>()) as *mut *mut c_void) };
        if !data.is_null() && is_a(data, gtk_window_type()) {
            let f = unsafe { get_focus(data) };
            if !f.is_null() {
                let a = unsafe { acc(f) };
                if !a.is_null() {
                    found = a;
                    if state_has(a, ATK_STATE_FOCUSED) {
                        break;
                    }
                }
            }
        }
        list = next;
    }
    if let Some(free_list) = gsym::<unsafe extern "C" fn(*mut c_void)>("g_list_free") {
        unsafe { free_list(head) };
    }
    found
}

fn handle(json: &str) -> String {
    if gtk().is_none() || atk().is_none() {
        return "{\"id\":0,\"ok\":false,\"error\":\"gtk3 symbols unavailable\"}\n".into();
    }
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
        "app" => format!(
            "{{\"id\":{id},\"ok\":true,\"pid\":{},\"prgname\":\"{}\",\"toplevels\":{},\"toolkit\":\"gtk3\",\"sock\":\"{}\"}}\n",
            std::process::id(),
            escape(&prgname()),
            toplevel_accessibles().len(),
            escape(&sock)
        ),
        "tree" => {
            let depth = v.get("depth").and_then(|x| x.as_i64()).unwrap_or(8) as i32;
            let vis = v.get("visible_only").and_then(|x| x.as_bool()).unwrap_or(false);
            let mut o = format!("{{\"id\":{id},\"ok\":true,\"windows\":[");
            for (i, top) in toplevel_accessibles().into_iter().enumerate() {
                if i > 0 {
                    o.push(',');
                }
                emit_object(&mut o, top, 0, depth, vis);
            }
            o.push_str("]}\n");
            o
        }
        "find" => {
            let limit = v.get("limit").and_then(|x| x.as_i64()).unwrap_or(50) as i32;
            let mut hits = Vec::new();
            for top in toplevel_accessibles() {
                collect(top, &sel, &mut hits, limit);
            }
            let mut o = format!("{{\"id\":{id},\"ok\":true,\"matches\":[");
            for (i, obj) in hits.iter().enumerate() {
                if i > 0 {
                    o.push(',');
                }
                emit_object(&mut o, *obj, 0, 0, false);
            }
            o.push_str("]}\n");
            o
        }
        "get" | "focused" => {
            let obj = if cmd == "focused" {
                let o = focused_object();
                if o.is_null() {
                    return fail("nothing focused");
                }
                o
            } else if let Some(c) = resolve_cell(&v) {
                c
            } else {
                match resolve_one(&sel) {
                    Ok(o) => o,
                    Err(e) => return fail(&e),
                }
            };
            let depth = v.get("depth").and_then(|x| x.as_i64()).unwrap_or(1) as i32;
            let mut o = format!("{{\"id\":{id},\"ok\":true,\"widget\":");
            emit_object(&mut o, obj, 0, depth, false);
            o.push_str("}\n");
            o
        }
        "do_action" => {
            let action = v.get("action").and_then(|x| x.as_str()).unwrap_or("click");
            let resolved = if let Some(c) = resolve_cell(&v) {
                Ok(c)
            } else {
                resolve_one(&sel)
            };
            match resolved {
                Ok(obj) => {
                    if do_named_action(obj, action) {
                        format!("{{\"id\":{id},\"ok\":true,\"did\":\"{}\"}}\n", escape(action))
                    } else {
                        fail(&format!("action '{action}' not available on {}", type_name(obj)))
                    }
                }
                Err(e) => fail(&e),
            }
        }
        "set_text" | "set_value" => {
            let text = v
                .get("text")
                .and_then(|x| x.as_str())
                .or_else(|| v.get("value").and_then(|x| x.as_str()))
                .unwrap_or("");
            let cell = resolve_cell(&v);
            let obj = if let Some(c) = cell {
                c
            } else {
                match resolve_one(&sel) {
                    Ok(o) => o,
                    Err(e) => return fail(&e),
                }
            };
            let r = if cmd == "set_text" {
                set_object_text(obj, text)
            } else {
                set_object_value(obj, text)
            };
            match r {
                Ok(()) => {
                    let mut o = format!("{{\"id\":{id},\"ok\":true,\"widget\":");
                    emit_object(&mut o, obj, 0, 0, false);
                    o.push_str("}\n");
                    o
                }
                Err(e) => fail(&e),
            }
        }
        "grab_focus" => {
            let resolved = if let Some(c) = resolve_cell(&v) {
                Ok(c)
            } else {
                resolve_one(&sel)
            };
            match resolved {
                Ok(obj) => {
                    let w = widget_of(obj);
                    if !w.is_null() {
                        if let Some(grab) = gtk_sym::<unsafe extern "C" fn(*mut c_void)>(b"gtk_widget_grab_focus") {
                            unsafe { grab(w) };
                        }
                    } else {
                        let _ = do_named_action(obj, "focus");
                    }
                    let mut o = format!("{{\"id\":{id},\"ok\":true,\"widget\":");
                    emit_object(&mut o, obj, 0, 0, false);
                    o.push_str("}\n");
                    o
                }
                Err(e) => fail(&e),
            }
        }
        "click_at" => match resolve_one(&sel) {
            Ok(obj) => {
                defer(obj, 0, "click", "");
                format!("{{\"id\":{id},\"ok\":true}}\n")
            }
            Err(e) => fail(&e),
        },
        "select_text" => {
            let start = v.get("start").and_then(|x| x.as_i64()).unwrap_or(0) as i32;
            let end = v.get("end").and_then(|x| x.as_i64()).unwrap_or(start as i64) as i32;
            let obj = match resolve_one(&sel) {
                Ok(o) => o,
                Err(e) => return fail(&e),
            };
            // Writer drops a selection made off the main loop. Same idle path as type_text.
            defer(obj, 3, &format!("{start}"), &end.to_string());
            format!("{{\"id\":{id},\"ok\":true,\"start\":{start},\"end\":{end}}}\n")
        }
        "type_text" => {
            let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("");
            let obj = if let Some(c) = resolve_cell(&v) {
                c
            } else if sel.targeted {
                match resolve_one(&sel) {
                    Ok(o) => o,
                    Err(e) => return fail(&e),
                }
            } else {
                focused_object()
            };
            if obj.is_null() {
                return fail("nothing focused");
            }
            defer(obj, 2, "", text);
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
    *job.reply.lock().unwrap() = Some(reply);
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
    let idle: unsafe extern "C" fn(extern "C" fn(*mut c_void) -> i32, *mut c_void) -> u32 = match gsym("g_idle_add") {
        Some(f) => f,
        None => return "{\"ok\":false,\"error\":\"no main loop\"}\n".into(),
    };
    unsafe { idle(job_idle, ptr as *mut c_void) };
    let job = unsafe { &*ptr };
    let mut slot = job.reply.lock().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while slot.is_none() {
        let now = std::time::Instant::now();
        if now >= deadline {
            return "{\"ok\":false,\"error\":\"main loop did not answer\"}\n".into();
        }
        let (guard, _) = job.cv.wait_timeout(slot, deadline - now).unwrap();
        slot = guard;
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
                let w = unsafe { libc::write(fd, bytes[off..].as_ptr() as *const c_void, bytes.len() - off) };
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
    if let Some(f) = gsym::<unsafe extern "C" fn() -> *const c_char>("g_get_prgname") {
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
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        eprintln!("gtk-remote: socket() failed");
        return 0;
    }
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let bytes = path.as_bytes();
    for (i, b) in bytes.iter().take(addr.sun_path.len() - 1).enumerate() {
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
        "gtk-remote: listening on {path} app={} pid={} toolkit=gtk3",
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
    let idle: unsafe extern "C" fn(extern "C" fn(*mut c_void) -> i32, *mut c_void) -> u32 = match gsym("g_idle_add") {
        Some(f) => f,
        None => {
            eprintln!("gtk-remote: g_idle_add missing");
            return;
        }
    };
    unsafe { idle(start_server, std::ptr::null_mut()) };
}
