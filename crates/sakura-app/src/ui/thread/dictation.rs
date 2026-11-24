//! Dictation into the master's composer. The system's own dictation (its shortcut, or the Edit menu) writes
//! into the composer without any help: the window's view is a text input client that takes `insertText:` and
//! `setMarkedText:`, and the composer is the input that takes them (a test types through that same path).
//! This is only the microphone button: it gives the composer the keyboard and asks the system to start
//! dictating into it, the way the Edit menu's "Start Dictation" does.

/// Asks the system to start dictation in the first responder of the key window. Says whether something took
/// the request: false off the main thread, off the Mac, or when no responder answers it.
#[cfg(target_os = "macos")]
pub fn start_system_dictation() -> bool {
    use objc2::{sel, MainThreadMarker};
    use objc2_app_kit::NSApplication;

    let Some(main_thread) = MainThreadMarker::new() else {
        return false;
    };
    let app = NSApplication::sharedApplication(main_thread);
    // SAFETY: AppKit routes its Start Dictation action to the first responder when the target is nil.
    unsafe { app.sendAction_to_from(sel!(startDictation:), None, None) }
}

/// Dictation is the Mac's.
#[cfg(not(target_os = "macos"))]
pub fn start_system_dictation() -> bool {
    false
}
