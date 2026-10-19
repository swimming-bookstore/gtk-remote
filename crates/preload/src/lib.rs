//! LD_PRELOAD stub. Does not link GTK. Linking libgtk-4 into a GTK 3 process
//! aborts at startup. This library only forces the accessibility env on, hooks
//! the init entry points, and dlopens the matching backend.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};

use libloading::os::unix::{Library, Symbol};

static ARMED: AtomicBool = AtomicBool::new(false);

fn env_disabled() -> bool {
    match std::env::var("GTK_REMOTE") {
        Ok(v) => {
            let v = v.to_ascii_lowercase();
            v == "0" || v == "off" || v == "false"
        }
        Err(_) => false,
    }
}

fn force_a11y_env() {
    if env_disabled() {
        return;
    }
    let a11y = std::env::var("GTK_A11Y").unwrap_or_default();
    if a11y.is_empty() || a11y == "1" || a11y == "yes" || a11y == "true" {
        std::env::set_var("GTK_A11Y", "atspi");
    }
    let mods = std::env::var("GTK_MODULES").unwrap_or_default();
    if !mods.contains("atk-bridge") {
        let next = if mods.is_empty() {
            "gail:atk-bridge".to_string()
        } else {
            format!("{mods}:gail:atk-bridge")
        };
        std::env::set_var("GTK_MODULES", next);
    }
    unsafe {
        let _ = Library::open(Some("libatk-bridge-2.0.so.0"), libc::RTLD_LAZY | libc::RTLD_GLOBAL);
        let _ = Library::open(Some("libatk-bridge-2.0.so"), libc::RTLD_LAZY | libc::RTLD_GLOBAL);
    }
}

#[ctor::ctor]
fn on_load() {
    force_a11y_env();
}

fn self_dir() -> Option<String> {
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::dladdr(on_load as *const () as *const libc::c_void, &mut info) };
    if ok == 0 || info.dli_fname.is_null() {
        return None;
    }
    let path = unsafe { CStr::from_ptr(info.dli_fname) }.to_string_lossy();
    Some(path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_else(|| ".".into()))
}

fn symbol_mapped(name: &str) -> bool {
    let Ok(c) = CString::new(name) else { return false };
    unsafe { !libc::dlsym(libc::RTLD_DEFAULT, c.as_ptr()).is_null() }
}

fn toolkit_is_gtk4() -> Option<bool> {
    if symbol_mapped("gtk_widget_get_first_child") {
        return Some(true);
    }
    if symbol_mapped("gtk_container_get_children") {
        return Some(false);
    }
    let c = CString::new("gtk_init").ok()?;
    let sym = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c.as_ptr()) };
    if sym.is_null() {
        return None;
    }
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    if unsafe { libc::dladdr(sym, &mut info) } == 0 || info.dli_fname.is_null() {
        return None;
    }
    let name = unsafe { CStr::from_ptr(info.dli_fname) }.to_string_lossy();
    if name.contains("libgtk-4") {
        Some(true)
    } else if name.contains("libgtk-3") {
        Some(false)
    } else {
        None
    }
}

fn try_open(path: &str) -> Option<Library> {
    match unsafe { Library::open(Some(path), libc::RTLD_NOW | libc::RTLD_GLOBAL) } {
        Ok(lib) => Some(lib),
        Err(err) => {
            eprintln!("gtk-remote: {path}: {err}");
            None
        }
    }
}

fn load_backend() {
    if ARMED.swap(true, Ordering::SeqCst) || env_disabled() {
        return;
    }
    let gtk4 = toolkit_is_gtk4();
    let which = if gtk4 == Some(false) {
        "libgtk_remote_gtk3.so"
    } else {
        "libgtk_remote_gtk4.so"
    };
    let dir = self_dir();
    let mut lib = dir
        .as_ref()
        .and_then(|d| try_open(&format!("{d}/{which}")))
        .or_else(|| try_open(which));
    if lib.is_none() && gtk4.is_none() {
        let other = "libgtk_remote_gtk3.so";
        lib = dir
            .as_ref()
            .and_then(|d| try_open(&format!("{d}/{other}")))
            .or_else(|| try_open(other));
    }
    let Some(lib) = lib else {
        ARMED.store(false, Ordering::SeqCst);
        return;
    };
    type Arm = unsafe extern "C" fn();
    let arm: Symbol<Arm> = match unsafe { lib.get(b"gtk_remote_backend_arm") } {
        Ok(s) => s,
        Err(_) => {
            eprintln!("gtk-remote: backend missing gtk_remote_backend_arm");
            ARMED.store(false, Ordering::SeqCst);
            return;
        }
    };
    unsafe { arm() };
    // Keep the backend mapped for the life of the process.
    std::mem::forget(lib);
}

fn next_sym<T>(name: &str) -> Option<T> {
    let c = CString::new(name).ok()?;
    let p = unsafe { libc::dlsym(libc::RTLD_NEXT, c.as_ptr()) };
    if p.is_null() {
        None
    } else {
        Some(unsafe { std::mem::transmute_copy(&p) })
    }
}

/// GTK 4's `gtk_init_check` takes no arguments. GTK 3's takes `argc` and `argv`
/// (and the public macro also passes two ABI sizes). Calling the GTK 3 symbol
/// as a no-arg function feeds garbage into `gtk_parse_args` and aborts apps
/// such as LibreOffice.
fn real_is_gtk3() -> bool {
    if toolkit_is_gtk4() == Some(false) {
        return true;
    }
    let Some(sym) = next_sym::<*const c_void>("gtk_init_check") else {
        return false;
    };
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    if unsafe { libc::dladdr(sym, &mut info) } == 0 || info.dli_fname.is_null() {
        return false;
    }
    let name = unsafe { CStr::from_ptr(info.dli_fname) }.to_string_lossy();
    name.contains("libgtk-3")
}

/// GTK 3: `void gtk_init(int *argc, char ***argv)`.
/// GTK 4: `void gtk_init(void)`.
#[no_mangle]
pub unsafe extern "C" fn gtk_init(argc: *mut c_int, argv: *mut *mut *mut c_char) {
    force_a11y_env();
    if real_is_gtk3() {
        if let Some(real) = next_sym::<unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char)>("gtk_init") {
            real(argc, argv);
        }
    } else if let Some(real) = next_sym::<unsafe extern "C" fn()>("gtk_init") {
        real();
    }
    load_backend();
}

/// GTK 3: `gboolean gtk_init_check(int *argc, char ***argv)`.
/// GTK 4: `gboolean gtk_init_check(void)`.
#[no_mangle]
pub unsafe extern "C" fn gtk_init_check(argc: *mut c_int, argv: *mut *mut *mut c_char) -> c_int {
    force_a11y_env();
    let ok = if real_is_gtk3() {
        if let Some(real) =
            next_sym::<unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char) -> c_int>("gtk_init_check")
        {
            real(argc, argv)
        } else {
            0
        }
    } else if let Some(real) = next_sym::<unsafe extern "C" fn() -> c_int>("gtk_init_check") {
        real()
    } else {
        0
    };
    if ok != 0 {
        load_backend();
    }
    ok
}

#[no_mangle]
pub unsafe extern "C" fn gtk_application_new(id: *const c_char, flags: c_int) -> *mut c_void {
    force_a11y_env();
    let app = if let Some(real) =
        next_sym::<unsafe extern "C" fn(*const c_char, c_int) -> *mut c_void>("gtk_application_new")
    {
        real(id, flags)
    } else {
        std::ptr::null_mut()
    };
    load_backend();
    app
}

/// GtkApplication never calls gtk_init itself. gedit goes through this.
#[no_mangle]
pub unsafe extern "C" fn g_application_run(
    app: *mut c_void,
    argc: c_int,
    argv: *mut *mut c_char,
) -> c_int {
    force_a11y_env();
    load_backend();
    if let Some(real) = next_sym::<unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_char) -> c_int>(
        "g_application_run",
    ) {
        real(app, argc, argv)
    } else {
        1
    }
}

#[no_mangle]
pub unsafe extern "C" fn gtk_init_with_args(
    argc: *mut c_int,
    argv: *mut *mut *mut c_char,
    parameter_string: *const c_char,
    entries: *mut c_void,
    translation_domain: *const c_char,
    error: *mut *mut c_void,
) -> c_int {
    type Fn = unsafe extern "C" fn(
        *mut c_int,
        *mut *mut *mut c_char,
        *const c_char,
        *mut c_void,
        *const c_char,
        *mut *mut c_void,
    ) -> c_int;
    force_a11y_env();
    let Some(real) = next_sym::<Fn>("gtk_init_with_args") else {
        return 0;
    };
    let ok = real(argc, argv, parameter_string, entries, translation_domain, error);
    if ok != 0 {
        load_backend();
    }
    ok
}
