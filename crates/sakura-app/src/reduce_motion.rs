use block2::RcBlock;
use gpui::{App, Global, Task};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol};
use std::ptr::NonNull;
use tokio::sync::watch;

struct WorkspaceMotion {
    center: Retained<NSNotificationCenter>,
    observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    _task: Task<()>,
}

impl Global for WorkspaceMotion {}

impl Drop for WorkspaceMotion {
    fn drop(&mut self) {
        unsafe { self.center.removeObserver(self.observer.as_ref()) };
    }
}

/// Keeps GPUI in sync with the Mac's Reduce Motion setting.
pub fn install(cx: &mut App) {
    let workspace = NSWorkspace::sharedWorkspace();
    follow(
        workspace.notificationCenter(),
        move || workspace.accessibilityDisplayShouldReduceMotion(),
        cx,
    );
}

fn follow(center: Retained<NSNotificationCenter>, read: impl Fn() -> bool + 'static, cx: &mut App) {
    let (notify, mut changes) = watch::channel(());
    let callback = RcBlock::new(move |_: NonNull<NSNotification>| {
        notify.send_replace(());
    });
    // AppKit may post off the main thread. Only the receiver touches GPUI.
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
            None,
            None,
            &callback,
        )
    };
    cx.set_reduce_motion(read());
    let task = cx.spawn(async move |cx| {
        while changes.changed().await.is_ok() {
            cx.update(|cx| cx.set_reduce_motion(read()));
        }
    });
    cx.set_global(WorkspaceMotion {
        center,
        observer,
        _task: task,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[gpui::test]
    fn workspace_reduce_motion_is_read_at_launch_and_after_display_options_change(
        cx: &mut gpui::TestAppContext,
    ) {
        let center = NSNotificationCenter::new();
        let is_reduced = Rc::new(Cell::new(true));
        let read = is_reduced.clone();
        cx.update(|cx| follow(center.clone(), move || read.get(), cx));
        assert!(cx.update(|cx| cx.reduce_motion()));

        is_reduced.set(false);
        unsafe {
            center.postNotificationName_object(
                NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification,
                None,
            );
        }
        cx.run_until_parked();
        assert!(!cx.update(|cx| cx.reduce_motion()));
    }
}
