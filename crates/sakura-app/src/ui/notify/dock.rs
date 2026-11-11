//! The badge on the Dock icon. The system's stands behind the trait; tests record what was set and
//! never reach the Dock of whoever runs them.

use std::rc::Rc;

/// What can show a short label on the app's icon.
pub trait Badge {
    /// Shows `label`, or takes the badge away.
    fn set(&self, label: Option<&str>);
}

/// No badge at all: what the tests get, and any machine that is not a Mac.
#[cfg(any(test, not(target_os = "macos")))]
#[derive(Debug, Clone, Copy, Default)]
pub struct NoBadge;

#[cfg(any(test, not(target_os = "macos")))]
impl Badge for NoBadge {
    fn set(&self, _: Option<&str>) {}
}

/// The system's badge: the Dock on a Mac, nothing elsewhere and nothing in the tests.
pub fn system() -> Rc<dyn Badge> {
    #[cfg(all(target_os = "macos", not(test)))]
    return Rc::new(mac::DockTile);
    #[cfg(not(all(target_os = "macos", not(test))))]
    Rc::new(NoBadge)
}

#[cfg(all(target_os = "macos", not(test)))]
mod mac {
    use super::Badge;
    use std::ffi::{c_char, c_void, CString};

    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    /// `NSApp.dockTile`.
    pub struct DockTile;

    impl Badge for DockTile {
        fn set(&self, label: Option<&str>) {
            // A label with a zero byte is no label: the badge is taken away.
            let label = label.and_then(|label| CString::new(label).ok());
            set_badge_label(label.as_deref());
        }
    }

    /// `[[NSApp dockTile] setBadgeLabel:]`, with nil for no label. Does nothing when AppKit has
    /// no application or no Dock tile.
    fn set_badge_label(label: Option<&std::ffi::CStr>) {
        type Get = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
        type FromUtf8 =
            unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_char) -> *mut c_void;
        type Set = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void);
        // SAFETY: the classes and the selectors are looked up by their documented names and
        // every object is checked for null; `objc_msgSend` is called through the exact C
        // signature of each method (`sharedApplication` and `dockTile` return an object,
        // `stringWithUTF8String:` takes a C string that lives until the call returns and
        // returns an autoreleased NSString, `setBadgeLabel:` takes an NSString or nil and
        // returns nothing), on the main thread, where the views run.
        unsafe {
            let send = objc_msgSend as unsafe extern "C" fn();
            let get: Get = std::mem::transmute(send);
            let application = objc_getClass(c"NSApplication".as_ptr());
            if application.is_null() {
                return;
            }
            let app = get(application, sel_registerName(c"sharedApplication".as_ptr()));
            if app.is_null() {
                return;
            }
            let tile = get(app, sel_registerName(c"dockTile".as_ptr()));
            if tile.is_null() {
                return;
            }
            let text = match label {
                Some(label) => {
                    let from_utf8: FromUtf8 = std::mem::transmute(send);
                    let string = objc_getClass(c"NSString".as_ptr());
                    if string.is_null() {
                        return;
                    }
                    from_utf8(
                        string,
                        sel_registerName(c"stringWithUTF8String:".as_ptr()),
                        label.as_ptr(),
                    )
                }
                None => std::ptr::null_mut(),
            };
            let set: Set = std::mem::transmute(send);
            set(tile, sel_registerName(c"setBadgeLabel:".as_ptr()), text);
        }
    }
}
