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
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;

    pub struct DockTile;

    impl Badge for DockTile {
        fn set(&self, label: Option<&str>) {
            let Some(main_thread) = MainThreadMarker::new() else {
                return;
            };
            let label = label
                .filter(|text| !text.as_bytes().contains(&0))
                .map(NSString::from_str);
            NSApplication::sharedApplication(main_thread)
                .dockTile()
                .setBadgeLabel(label.as_deref());
        }
    }
}
