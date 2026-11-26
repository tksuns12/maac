//! macOS native floating `NSWindow` + `NSView` host for CLAP (`clap.gui` Cocoa)
//! and Audio Unit v2 (`kAudioUnitProperty_CocoaUI` / `AUCocoaUIBase` +
//! `CoreAudioKit` `AUGenericView` fallback).
//!
//! Uses direct Objective-C runtime FFI (`objc_msgSend`) so `studio/` compiles
//! offline with zero extra crate dependencies.
//!
//! # Threading invariant
//! Every constructor, method, and `Drop` of `NativePluginWindow` MUST execute
//! on the macOS AppKit main thread (`iced`'s `update()` / `Tick` thread).

#![cfg(target_os = "macos")]

use std::ffi::{c_char, c_void, CStr, CString};
use std::mem::size_of;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

type Id = *mut c_void;
type Class = *mut c_void;
type Sel = *mut c_void;
type OSStatus = i32;
type CFTypeRef = *const c_void;

const K_AUDIO_UNIT_SCOPE_GLOBAL: u32 = 0;
const K_AUDIO_UNIT_PROPERTY_COCOA_UI: u32 = 31;

// NSWindowStyleMask bits
const NS_WINDOW_STYLE_MASK_TITLED: usize = 1 << 0;
const NS_WINDOW_STYLE_MASK_CLOSABLE: usize = 1 << 1;
const NS_WINDOW_STYLE_MASK_MINIATURIZABLE: usize = 1 << 2;
const NS_WINDOW_STYLE_MASK_RESIZABLE: usize = 1 << 3;

// NSBackingStoreBuffered = 2
const NS_BACKING_STORE_BUFFERED: usize = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct NSPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct NSSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct NSRect {
    origin: NSPoint,
    size: NSSize,
}

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Class;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    #[cfg(target_arch = "x86_64")]
    fn objc_msgSend_stret();
}

#[link(name = "Foundation", kind = "framework")]
extern "C" {}

#[link(name = "AppKit", kind = "framework")]
extern "C" {}

#[link(name = "CoreAudioKit", kind = "framework")]
extern "C" {
    // Force linking CoreAudioKit so `objc_getClass("AUGenericView")` resolves.
    fn objc_opt_class();
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: CFTypeRef);
}

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn AudioUnitGetPropertyInfo(
        in_unit: *mut c_void,
        in_id: u32,
        in_scope: u32,
        in_element: u32,
        out_data_size: *mut u32,
        out_writable: *mut u8,
    ) -> OSStatus;
    fn AudioUnitGetProperty(
        in_unit: *mut c_void,
        in_id: u32,
        in_scope: u32,
        in_element: u32,
        out_data: *mut c_void,
        io_data_size: *mut u32,
    ) -> OSStatus;
}

unsafe fn cls(name: &CStr) -> Class {
    objc_getClass(name.as_ptr())
}

unsafe fn sel(name: &CStr) -> Sel {
    sel_registerName(name.as_ptr())
}

unsafe fn msg0_id(obj: Id, s: Sel) -> Id {
    let f: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(objc_msgSend as *const ());
    f(obj, s)
}

unsafe fn msg0_void(obj: Id, s: Sel) {
    let f: unsafe extern "C" fn(Id, Sel) = std::mem::transmute(objc_msgSend as *const ());
    f(obj, s)
}

unsafe fn msg0_bool(obj: Id, s: Sel) -> bool {
    let f: unsafe extern "C" fn(Id, Sel) -> i8 = std::mem::transmute(objc_msgSend as *const ());
    f(obj, s) != 0
}

unsafe fn msg1_id_id(obj: Id, s: Sel, a1: Id) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, Id) -> Id = std::mem::transmute(objc_msgSend as *const ());
    f(obj, s, a1)
}

unsafe fn msg1_void_id(obj: Id, s: Sel, a1: Id) {
    let f: unsafe extern "C" fn(Id, Sel, Id) = std::mem::transmute(objc_msgSend as *const ());
    f(obj, s, a1)
}

unsafe fn msg1_void_bool(obj: Id, s: Sel, a1: bool) {
    let f: unsafe extern "C" fn(Id, Sel, i8) = std::mem::transmute(objc_msgSend as *const ());
    f(obj, s, i8::from(a1))
}

unsafe fn msg1_void_size(obj: Id, s: Sel, size: NSSize) {
    let f: unsafe extern "C" fn(Id, Sel, NSSize) = std::mem::transmute(objc_msgSend as *const ());
    f(obj, s, size)
}

unsafe fn msg1_id_rect(obj: Id, s: Sel, rect: NSRect) -> Id {
    let f: unsafe extern "C" fn(Id, Sel, NSRect) -> Id =
        std::mem::transmute(objc_msgSend as *const ());
    f(obj, s, rect)
}

unsafe fn msg1_void_rect(obj: Id, s: Sel, rect: NSRect) {
    let f: unsafe extern "C" fn(Id, Sel, NSRect) = std::mem::transmute(objc_msgSend as *const ());
    f(obj, s, rect)
}

unsafe fn msg_view_frame(view: Id) -> NSRect {
    let s = sel(c"frame");
    #[cfg(target_arch = "aarch64")]
    {
        let f: unsafe extern "C" fn(Id, Sel) -> NSRect =
            std::mem::transmute(objc_msgSend as *const ());
        f(view, s)
    }
    #[cfg(target_arch = "x86_64")]
    {
        let mut out = NSRect::default();
        let f: unsafe extern "C" fn(*mut NSRect, Id, Sel) =
            std::mem::transmute(objc_msgSend_stret as *const ());
        f(&mut out, view, s);
        out
    }
}

unsafe fn ns_string(text: &str) -> Id {
    let cstr = CString::new(text).unwrap_or_else(|_| CString::new("Plugin").unwrap());
    let ns_str_cls = cls(c"NSString");
    let f: unsafe extern "C" fn(Class, Sel, *const c_char) -> Id =
        std::mem::transmute(objc_msgSend as *const ());
    f(ns_str_cls, sel(c"stringWithUTF8String:"), cstr.as_ptr())
}

static CASCADE_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Floating macOS `NSWindow` hosting an external plugin's native `NSView`.
pub struct NativePluginWindow {
    window: Id,
    retained_objects: Vec<Id>,
}

// SAFETY: `NativePluginWindow` is held inside `PluginInstance` behind
// `Mutex<HostRack>`, and all methods (`new`, `show`, `set_content_size`,
// `is_visible`, `close`) are invoked strictly from the macOS AppKit main thread.
unsafe impl Send for NativePluginWindow {}

impl NativePluginWindow {
    pub fn new(title: &str, width: f64, height: f64, resizable: bool) -> Result<Self, String> {
        let w = width.clamp(240.0, 2200.0);
        let h = height.clamp(160.0, 1400.0);
        let step = (CASCADE_COUNTER.fetch_add(1, Ordering::Relaxed) % 8) as f64;
        let origin_x = 180.0 + step * 34.0;
        let origin_y = 220.0 - step * 22.0;

        unsafe {
            let win_cls = cls(c"NSWindow");
            if win_cls.is_null() {
                return Err("AppKit NSWindow class unavailable".into());
            }
            let alloc = msg0_id(win_cls, sel(c"alloc"));
            if alloc.is_null() {
                return Err("NSWindow alloc failed".into());
            }
            let mut style = NS_WINDOW_STYLE_MASK_TITLED
                | NS_WINDOW_STYLE_MASK_CLOSABLE
                | NS_WINDOW_STYLE_MASK_MINIATURIZABLE;
            if resizable {
                style |= NS_WINDOW_STYLE_MASK_RESIZABLE;
            }
            let rect = NSRect {
                origin: NSPoint {
                    x: origin_x,
                    y: origin_y,
                },
                size: NSSize {
                    width: w,
                    height: h,
                },
            };
            let init_fn: unsafe extern "C" fn(Id, Sel, NSRect, usize, usize, i8) -> Id =
                std::mem::transmute(objc_msgSend as *const ());
            let window = init_fn(
                alloc,
                sel(c"initWithContentRect:styleMask:backing:defer:"),
                rect,
                style,
                NS_BACKING_STORE_BUFFERED,
                0,
            );
            if window.is_null() {
                return Err("NSWindow initWithContentRect failed".into());
            }
            // Keep our Rust handle valid even after the user clicks the red close button.
            msg1_void_bool(window, sel(c"setReleasedWhenClosed:"), false);
            let title_ns = ns_string(title);
            if !title_ns.is_null() {
                msg1_void_id(window, sel(c"setTitle:"), title_ns);
            }
            Ok(Self {
                window,
                retained_objects: Vec::new(),
            })
        }
    }

    pub fn content_view(&self) -> *mut c_void {
        if self.window.is_null() {
            return ptr::null_mut();
        }
        unsafe { msg0_id(self.window, sel(c"contentView")) }
    }

    pub fn set_content_view(&mut self, view: *mut c_void) {
        if self.window.is_null() || view.is_null() {
            return;
        }
        unsafe {
            msg1_void_id(self.window, sel(c"setContentView:"), view);
        }
    }

    pub fn retain_extra(&mut self, obj: *mut c_void) {
        if !obj.is_null() {
            unsafe {
                let _ = msg0_id(obj, sel(c"retain"));
            }
            self.retained_objects.push(obj);
        }
    }

    pub fn adopt_owned(&mut self, obj: *mut c_void) {
        if !obj.is_null() {
            self.retained_objects.push(obj);
        }
    }

    pub fn set_content_size(&self, width: f64, height: f64) {
        if self.window.is_null() {
            return;
        }
        let size = NSSize {
            width: width.clamp(240.0, 2200.0),
            height: height.clamp(160.0, 1400.0),
        };
        unsafe {
            msg1_void_size(self.window, sel(c"setContentSize:"), size);
        }
    }

    pub fn show(&self) {
        if self.window.is_null() {
            return;
        }
        unsafe {
            msg1_void_id(self.window, sel(c"makeKeyAndOrderFront:"), ptr::null_mut());
        }
    }

    pub fn is_visible(&self) -> bool {
        if self.window.is_null() {
            return false;
        }
        unsafe { msg0_bool(self.window, sel(c"isVisible")) }
    }

    pub fn close(&mut self) {
        if !self.window.is_null() {
            unsafe {
                msg1_void_id(self.window, sel(c"orderOut:"), ptr::null_mut());
                msg1_void_id(self.window, sel(c"setContentView:"), ptr::null_mut());
                msg0_void(self.window, sel(c"release"));
            }
            self.window = ptr::null_mut();
        }
        for obj in self.retained_objects.drain(..) {
            if !obj.is_null() {
                unsafe {
                    msg0_void(obj, sel(c"release"));
                }
            }
        }
    }
}

impl Drop for NativePluginWindow {
    fn drop(&mut self) {
        self.close();
    }
}

/// Try to instantiate an Audio Unit v2's custom Cocoa UI (`AUCocoaUIBase`),
/// falling back to Apple's `CoreAudioKit` `AUGenericView` for built-in units
/// (like `AUDelay`, `AUMatrixReverb`, `DLSMusicDevice`).
pub fn open_au_native_window(unit: *mut c_void, title: &str) -> Result<NativePluginWindow, String> {
    if unit.is_null() {
        return Err("null AudioUnit handle".into());
    }
    // Keep a harmless reference so the linker retains CoreAudioKit.framework.
    let _ = objc_opt_class as *const ();

    // 1. Try custom Cocoa UI via `kAudioUnitProperty_CocoaUI` (`AudioUnitCocoaViewInfo`).
    if let Some(win) = unsafe { try_open_custom_au_cocoa_view(unit, title) } {
        return Ok(win);
    }

    // 2. Fallback to `AUGenericView` (CoreAudioKit) so even built-in Apple AUs have a native window.
    unsafe { open_au_generic_view(unit, title) }
}

unsafe fn try_open_custom_au_cocoa_view(
    unit: *mut c_void,
    title: &str,
) -> Option<NativePluginWindow> {
    let mut byte_size: u32 = 0;
    let mut writable: u8 = 0;
    let st = AudioUnitGetPropertyInfo(
        unit,
        K_AUDIO_UNIT_PROPERTY_COCOA_UI,
        K_AUDIO_UNIT_SCOPE_GLOBAL,
        0,
        &mut byte_size,
        &mut writable,
    );
    let ptr_size = size_of::<*const c_void>();
    if st != 0 || (byte_size as usize) < ptr_size * 2 {
        return None;
    }
    let ptr_count = (byte_size as usize) / ptr_size;
    let mut raw_ptrs = vec![ptr::null::<c_void>(); ptr_count];
    let st2 = AudioUnitGetProperty(
        unit,
        K_AUDIO_UNIT_PROPERTY_COCOA_UI,
        K_AUDIO_UNIT_SCOPE_GLOBAL,
        0,
        raw_ptrs.as_mut_ptr() as *mut c_void,
        &mut byte_size,
    );
    if st2 != 0 {
        return None;
    }

    let bundle_url = raw_ptrs[0];
    let class_name = raw_ptrs[1];
    let mut result: Option<NativePluginWindow> = None;

    if !bundle_url.is_null() && !class_name.is_null() {
        let ns_bundle_cls = cls(c"NSBundle");
        let bundle = msg1_id_id(ns_bundle_cls, sel(c"bundleWithURL:"), bundle_url as Id);
        if !bundle.is_null() {
            let _ = msg0_bool(bundle, sel(c"load"));
            let factory_cls = msg1_id_id(bundle, sel(c"classNamed:"), class_name as Id);
            if !factory_cls.is_null() {
                let factory_alloc = msg0_id(factory_cls, sel(c"alloc"));
                let factory = if !factory_alloc.is_null() {
                    msg0_id(factory_alloc, sel(c"init"))
                } else {
                    ptr::null_mut()
                };
                if !factory.is_null() {
                    let ui_fn: unsafe extern "C" fn(Id, Sel, *mut c_void, NSSize) -> Id =
                        std::mem::transmute(objc_msgSend as *const ());
                    let view = ui_fn(
                        factory,
                        sel(c"uiViewForAudioUnit:withSize:"),
                        unit,
                        NSSize {
                            width: 800.0,
                            height: 520.0,
                        },
                    );
                    if !view.is_null() {
                        let frame = msg_view_frame(view);
                        let w = if frame.size.width >= 120.0 {
                            frame.size.width
                        } else {
                            800.0
                        };
                        let h = if frame.size.height >= 80.0 {
                            frame.size.height
                        } else {
                            520.0
                        };
                        if let Ok(mut win) = NativePluginWindow::new(title, w, h, true) {
                            win.adopt_owned(factory);
                            win.retain_extra(view);
                            win.set_content_view(view);
                            win.show();
                            result = Some(win);
                        } else {
                            msg0_void(factory, sel(c"release"));
                        }
                    } else {
                        msg0_void(factory, sel(c"release"));
                    }
                }
            }
        }
    }

    // Release every CF object returned by `AudioUnitGetProperty` (bundle URL + class names).
    for p in raw_ptrs {
        if !p.is_null() {
            CFRelease(p);
        }
    }

    result
}

unsafe fn open_au_generic_view(
    unit: *mut c_void,
    title: &str,
) -> Result<NativePluginWindow, String> {
    let gv_cls = cls(c"AUGenericView");
    if gv_cls.is_null() {
        return Err("CoreAudioKit AUGenericView class unavailable".into());
    }
    let alloc = msg0_id(gv_cls, sel(c"alloc"));
    if alloc.is_null() {
        return Err("AUGenericView alloc failed".into());
    }
    let init_fn: unsafe extern "C" fn(Id, Sel, *mut c_void) -> Id =
        std::mem::transmute(objc_msgSend as *const ());
    let view = init_fn(alloc, sel(c"initWithAudioUnit:"), unit);
    if view.is_null() {
        return Err("AUGenericView initWithAudioUnit failed".into());
    }
    let frame = msg_view_frame(view);
    let raw_w = frame.size.width.clamp(460.0, 900.0);
    let raw_h = frame.size.height.max(240.0);
    let win_h = raw_h.min(680.0);

    let mut win = NativePluginWindow::new(title, raw_w, win_h, true)?;
    if raw_h > 680.0 {
        // Wrap tall generic parameter lists in a scrollable `NSScrollView`.
        let sv_cls = cls(c"NSScrollView");
        let sv_alloc = msg0_id(sv_cls, sel(c"alloc"));
        let sv_rect = NSRect {
            origin: NSPoint { x: 0.0, y: 0.0 },
            size: NSSize {
                width: raw_w,
                height: win_h,
            },
        };
        let sv = msg1_id_rect(sv_alloc, sel(c"initWithFrame:"), sv_rect);
        if !sv.is_null() {
            msg1_void_bool(sv, sel(c"setHasVerticalScroller:"), true);
            msg1_void_bool(sv, sel(c"setAutohidesScrollers:"), true);
            msg1_void_rect(
                view,
                sel(c"setFrame:"),
                NSRect {
                    origin: NSPoint { x: 0.0, y: 0.0 },
                    size: NSSize {
                        width: raw_w,
                        height: raw_h,
                    },
                },
            );
            msg1_void_id(sv, sel(c"setDocumentView:"), view);
            win.adopt_owned(view);
            win.adopt_owned(sv);
            win.set_content_view(sv);
            win.show();
            return Ok(win);
        }
    }

    win.adopt_owned(view);
    win.set_content_view(view);
    win.show();
    Ok(win)
}
