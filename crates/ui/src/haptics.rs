//! Discrete native feedback for user-driven control detents.

/// A slider or stepper landed on another value.
pub(crate) fn selection_step() {
    #[cfg(all(target_os = "macos", not(test)))]
    perform(objc2_app_kit::NSHapticFeedbackPattern::LevelChange);
    #[cfg(test)]
    STEPS.with(|steps| steps.set(steps.get() + 1));
}

/// A dragged item snapped into another slot of the list it reorders.
pub(crate) fn reorder_step() {
    #[cfg(all(target_os = "macos", not(test)))]
    perform(objc2_app_kit::NSHapticFeedbackPattern::Alignment);
    #[cfg(test)]
    STEPS.with(|steps| steps.set(steps.get() + 1));
}

#[cfg(all(target_os = "macos", not(test)))]
fn perform(pattern: objc2_app_kit::NSHapticFeedbackPattern) {
    use objc2_app_kit::{
        NSHapticFeedbackManager, NSHapticFeedbackPerformanceTime, NSHapticFeedbackPerformer,
    };
    // UI input handlers run on the main thread. AppKit gracefully does
    // nothing on devices without a haptic-capable trackpad.
    if objc2::MainThreadMarker::new().is_some() {
        NSHapticFeedbackManager::defaultPerformer().performFeedbackPattern_performanceTime(
            pattern,
            NSHapticFeedbackPerformanceTime::Now,
        );
    }
}

#[cfg(test)]
thread_local! { static STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

#[cfg(test)]
pub(crate) fn step_count() -> usize {
    STEPS.with(|steps| steps.get())
}
