//! Sparkle's standard updater, loaded from the bundle and used only on the main thread.

use super::model::{Release, UpdateState};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly, ProtocolType};
use objc2_foundation::{NSBundle, NSError, NSObject, NSObjectProtocol, NSString};
use std::cell::RefCell;

/// Sparkle's updater delegate protocol, loaded from the bundled framework.
///
/// # Safety
/// Implementations must use Sparkle 2.10.1's selector signatures and handle
/// updater callbacks on the main thread, as required by the framework.
unsafe trait UpdaterDelegate: NSObjectProtocol {}

// SAFETY: Sparkle defines SPUUpdaterDelegate with NSObject protocol conformance.
unsafe impl ProtocolType for dyn UpdaterDelegate {
    const NAME: &'static str = "SPUUpdaterDelegate";
    const __INNER: () = ();
}

define_class!(
    // SAFETY: NSObject permits subclassing, and Sparkle invokes its delegate on the main thread.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = RefCell<UpdateState>]
    struct SakuraUpdateDelegate;

    // SAFETY: The NSObject superclass provides its protocol methods.
    unsafe impl NSObjectProtocol for SakuraUpdateDelegate {}
    // SAFETY: The delegate methods match Sparkle 2.10.1's Objective-C signatures.
    unsafe impl UpdaterDelegate for SakuraUpdateDelegate {}

    impl SakuraUpdateDelegate {
        #[unsafe(method(updater:mayPerformUpdateCheck:error:))]
        fn checking(&self, _updater: &AnyObject, _kind: isize, _error: *mut *mut NSError) -> Bool {
            self.ivars().replace(UpdateState::Checking);
            Bool::YES
        }
        #[unsafe(method(updater:didFindValidUpdate:))]
        fn found(&self, _updater: &AnyObject, item: &AnyObject) {
            // SAFETY: Sparkle passes a SUAppcastItem whose displayVersionString returns a nonnull NSString.
            let version: Retained<NSString> = unsafe { msg_send![item, displayVersionString] };
            self.ivars().replace(UpdateState::Available(Release {
                version: version.to_string(), url: String::new(),
            }));
        }

        #[unsafe(method(updaterDidNotFindUpdate:error:))]
        fn up_to_date(&self, _updater: &AnyObject, _error: &NSError) {
            self.ivars().replace(UpdateState::UpToDate);
        }

        #[unsafe(method(updater:didAbortWithError:))]
        fn failed(&self, _updater: &AnyObject, error: &NSError) {
            // SUNoUpdateError from Sparkle's public SUError.h is a successful no-update result.
            const NO_UPDATE_ERROR: isize = 1001;
            let state = if error.code() == NO_UPDATE_ERROR { UpdateState::UpToDate } else { UpdateState::Failed };
            self.ivars().replace(state);
        }
    }
);

impl SakuraUpdateDelegate {
    fn new(marker: MainThreadMarker) -> Retained<Self> {
        let allocated = Self::alloc(marker).set_ivars(RefCell::new(UpdateState::Ready));
        // SAFETY: The allocated NSObject subclass has its Rust ivars set before calling init.
        unsafe { msg_send![super(allocated), init] }
    }
}

pub struct Sparkle {
    controller: Retained<AnyObject>,
    updater: Retained<AnyObject>,
    delegate: Retained<SakuraUpdateDelegate>,
    _framework: Retained<NSBundle>,
}

impl Sparkle {
    /// Starts Sparkle only when the bundle, HTTPS feed and public key are valid.
    /// Missing components leave the build unconfigured without opening an alert.
    pub fn new(automatic: bool) -> Option<Self> {
        let marker = MainThreadMarker::new()?;
        let bundle = NSBundle::mainBundle();
        let feed = bundle_text(&bundle, "SUFeedURL");
        let key = bundle_text(&bundle, "SUPublicEDKey");
        if !super::has_metadata(feed.as_deref(), key.as_deref()) {
            return None;
        }
        let path = format!("{}/Sparkle.framework", bundle.privateFrameworksPath()?);
        let framework = NSBundle::bundleWithPath(&NSString::from_str(&path))?;
        // SAFETY: This is the bundled Sparkle framework, loaded on the main thread.
        unsafe {
            framework.loadAndReturnError().ok()?;
        }
        Self::create(framework, marker, automatic)
    }

    /// The most recent delegate result, including checks started by Sparkle's scheduler.
    pub fn state(&self) -> UpdateState {
        let state = self.delegate.ivars().borrow().clone();
        if state == UpdateState::Ready && !self.can_check() {
            UpdateState::Checking
        } else {
            state
        }
    }

    /// Sparkle owns readiness, including downloads and installation sessions.
    pub fn can_check(&self) -> bool {
        // SAFETY: The retained SPUUpdater returns a BOOL from canCheckForUpdates.
        let ready: Bool = unsafe { msg_send![&*self.updater, canCheckForUpdates] };
        ready.as_bool()
    }

    /// Performs the standard, user-initiated update check when Sparkle is ready.
    pub fn check_now(&self) {
        if !self.can_check() {
            return;
        }
        self.delegate.ivars().replace(UpdateState::Checking);
        // SAFETY: The controller's checkForUpdates: accepts a nil sender.
        unsafe {
            let _: () =
                msg_send![&*self.controller, checkForUpdates: std::ptr::null::<AnyObject>()];
        }
    }

    /// Keeps checks and downloads in sync with the application's stored preference.
    pub fn set_automatic(&self, enabled: bool) {
        let enabled = Bool::new(enabled);
        // SAFETY: SPUUpdater's setters take BOOL or NSString as declared in Sparkle 2.10.1.
        unsafe {
            let _: () = msg_send![&*self.updater, setAutomaticallyChecksForUpdates: enabled];
            let _: () = msg_send![&*self.updater, setAutomaticallyDownloadsUpdates: enabled];
            let _: () = msg_send![&*self.updater, setSendsSystemProfile: Bool::NO];
            let _: () =
                msg_send![&*self.updater, setUserAgentString: &*NSString::from_str("Sakura")];
        }
    }

    fn create(
        framework: Retained<NSBundle>,
        marker: MainThreadMarker,
        automatic: bool,
    ) -> Option<Self> {
        let class = AnyClass::get(c"SPUStandardUpdaterController")?;
        let delegate = SakuraUpdateDelegate::new(marker);
        // SAFETY: Sparkle's initializer accepts a BOOL, a retained delegate and a nil user driver delegate.
        let controller: Option<Retained<AnyObject>> = unsafe {
            let allocated: Allocated<AnyObject> = msg_send![class, alloc];
            msg_send![allocated, initWithStartingUpdater: Bool::NO,
                updaterDelegate: &*delegate, userDriverDelegate: std::ptr::null::<AnyObject>()]
        };
        let controller = controller?;
        // SAFETY: The initialized controller supplies its nonnull SPUUpdater.
        let updater = unsafe { msg_send![&*controller, updater] };
        let sparkle = Self {
            controller,
            updater,
            delegate,
            _framework: framework,
        };
        sparkle.set_automatic(automatic);
        // SAFETY: SPUUpdater's startUpdater: returns BOOL and accepts a null NSError out pointer.
        let started: Bool = unsafe {
            msg_send![&*sparkle.updater, startUpdater: std::ptr::null_mut::<*mut NSError>()]
        };
        started.as_bool().then_some(sparkle)
    }
}

fn bundle_text(bundle: &NSBundle, name: &str) -> Option<String> {
    bundle
        .objectForInfoDictionaryKey(&NSString::from_str(name))?
        .downcast::<NSString>()
        .ok()
        .map(|text| text.to_string())
}
