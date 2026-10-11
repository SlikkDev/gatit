//! Turns the "Record this call?" window into a macOS panel that floats over calls.
//!
//! A regular window can't appear over another app's full-screen Space, and clicking it brings
//! Gatit to the front, which takes keyboard focus away from the call. An `NSPanel` with the
//! non-activating style and the `fullScreenAuxiliary` behaviour has neither problem: it joins
//! whatever Space the call is on, and a click on Record lands without activating Gatit.
//!
//! Tauri only builds `NSWindow`s, so the window's class is swapped to an `NSPanel` subclass after
//! it is built. `NSPanel` adds no instance variables to `NSWindow`, which is what makes the swap
//! safe; tauri-nspanel does the same.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, ClassType, MainThreadOnly};
use objc2_app_kit::{NSPanel, NSWindowCollectionBehavior, NSWindowStyleMask};
use tauri::WebviewWindow;

/// `NSStatusWindowLevel`: above full-screen apps and Zoom's own full-screen window, below the
/// screen saver.
const STATUS_WINDOW_LEVEL: isize = 25;

define_class!(
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    #[name = "GatitPromptPanel"]
    struct PromptPanel;

    impl PromptPanel {
        /// Never take keyboard focus, so typing keeps going to the call.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            false
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            false
        }
    }
);

/// Makes `window` float over the current call, full screen or not, without taking focus, then
/// shows it. Build the window hidden and call this instead of `show()`.
pub fn float_over_calls(window: &WebviewWindow) {
    let window = window.clone();
    let _ = window.clone().run_on_main_thread(move || {
        let Ok(ptr) = window.ns_window() else {
            return;
        };
        // SAFETY: `ns_window` is the live NSWindow behind this Tauri window, and we are on the
        // main thread. Swapping the class to an ivar-less NSPanel subclass keeps the object
        // layout valid.
        unsafe {
            let object = &*(ptr as *const AnyObject);
            AnyObject::set_class(object, PromptPanel::class());
            let panel: Retained<NSPanel> = Retained::retain(ptr as *mut NSPanel).unwrap();
            panel.setStyleMask(panel.styleMask() | NSWindowStyleMask::NonactivatingPanel);
            panel.setFloatingPanel(true);
            panel.setBecomesKeyOnlyIfNeeded(true);
            panel.setHidesOnDeactivate(false);
            panel.setLevel(STATUS_WINDOW_LEVEL);
            panel.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary
                    | NSWindowCollectionBehavior::Stationary
                    | NSWindowCollectionBehavior::IgnoresCycle,
            );
            // Bring it up without making it key or activating Gatit.
            let _: () = msg_send![&*panel, orderFrontRegardless];
        }
    });
}
