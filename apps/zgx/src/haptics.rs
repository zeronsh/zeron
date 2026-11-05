//! A trackpad's tap under a control's detent (the shipping app's
//! `crates/ui/src/haptics.rs`): each step a slider lands on, as a person
//! moves it. AppKit does nothing on a Mac without a haptic trackpad, and
//! nothing happens elsewhere.
//!
//! ```text
//! invoke  haptic.step   (none)   one detent's tap, now
//! ```

use zgx_script::ScriptHost;

pub fn install(host: &ScriptHost) {
    host.action("haptic.step", |_| step());
}

/// One detent's tap: AppKit's level change, performed now.
pub fn step() {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{
            NSHapticFeedbackManager, NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime,
            NSHapticFeedbackPerformer,
        };
        // (Script actions run on the main thread; AppKit's performer is a
        // main-thread object.)
        if objc2::MainThreadMarker::new().is_some() {
            NSHapticFeedbackManager::defaultPerformer().performFeedbackPattern_performanceTime(
                NSHapticFeedbackPattern::LevelChange,
                NSHapticFeedbackPerformanceTime::Now,
            );
        }
    }
}
