//! Discrete native feedback for user-driven control detents.

pub(crate) fn selection_step() {
    #[cfg(all(target_os = "macos", not(test)))]
    {
        use objc2_app_kit::{
            NSHapticFeedbackManager, NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime,
            NSHapticFeedbackPerformer,
        };
        // UI input handlers run on the main thread. AppKit gracefully does
        // nothing on devices without a haptic-capable trackpad.
        if objc2::MainThreadMarker::new().is_some() {
            NSHapticFeedbackManager::defaultPerformer().performFeedbackPattern_performanceTime(
                NSHapticFeedbackPattern::LevelChange,
                NSHapticFeedbackPerformanceTime::Now,
            );
        }
    }
    #[cfg(test)]
    STEPS.with(|steps| steps.set(steps.get() + 1));
}

#[cfg(test)]
thread_local! { static STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

#[cfg(test)]
pub(crate) fn step_count() -> usize {
    STEPS.with(|steps| steps.get())
}
