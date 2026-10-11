//! A fade-through when navigation changes the conversation's horizontal frame.
use std::time::Instant;

pub(super) const DURATION: f32 = 0.320;
/// Between the fade-out ending at 0.18 and the fade-in starting at 0.26.
pub(super) const GEOMETRY_SWITCH: f32 = 0.22;

fn ease(value: f32, start: f32, end: f32) -> f32 {
    let t = ((value - start) / (end - start)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[derive(Default)]
pub(super) struct PanelHandoff {
    previous: Option<(bool, (f32, f32))>,
    started: Option<Instant>,
    from_opacity: f32,
    navigation_changed: bool,
    pub route_change: bool,
    pub progress: Option<f32>,
}

impl PanelHandoff {
    pub fn navigate(&mut self) {
        self.navigation_changed = true;
    }

    pub fn opacity(&self) -> f32 {
        self.progress.map_or(1.0, |p| {
            self.from_opacity * (1.0 - ease(p, 0.0, 0.18)) + ease(p, 0.26, 1.0)
        })
    }

    pub fn sample(
        &mut self,
        docked: bool,
        column: (f32, f32),
        enabled: bool,
        now: Instant,
        duration: f32,
    ) -> bool {
        if !enabled {
            *self = Self::default();
            return false;
        }
        let navigation_changed = std::mem::take(&mut self.navigation_changed);
        if self.previous.is_some_and(|(old_docked, old_column)| {
            (old_docked != docked || navigation_changed)
                && ((old_column.0 - column.0).abs() > 0.5
                    || (old_column.1 - column.1).abs() > 0.5
                    || self.started.is_some())
        }) {
            self.route_change = self
                .previous
                .is_some_and(|(old_docked, _)| old_docked != docked);
            self.from_opacity = self.opacity();
            self.started = Some(now);
        }
        self.previous = Some((docked, column));
        self.progress = self.started.and_then(|start| {
            let p = now.saturating_duration_since(start).as_secs_f32() / duration;
            (p < 1.0).then_some(p)
        });
        if self.progress.is_none() {
            self.started = None;
        }
        self.progress.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn geometry_switch_is_hidden_in_both_directions() {
        for (docked, from, to) in [(true, 0.0, 480.0), (false, 480.0, 0.0)] {
            let mut handoff = PanelHandoff::default();
            let now = Instant::now();
            handoff.sample(!docked, (0.0, from), true, now, 0.320);
            assert!(handoff.sample(docked, (0.0, to), true, now, 0.320));
            assert_eq!(handoff.opacity(), 1.0);
            for millis in [60, 70, 80] {
                handoff.sample(
                    docked,
                    (0.0, to),
                    true,
                    now + Duration::from_millis(millis),
                    0.320,
                );
                assert_eq!(handoff.opacity(), 0.0);
            }
            assert!(!handoff.sample(
                docked,
                (0.0, to),
                true,
                now + Duration::from_millis(321),
                0.320
            ));
            assert_eq!(handoff.opacity(), 1.0);
        }
    }

    #[test]
    fn reversal_preserves_opacity_and_reduced_motion_cancels() {
        let mut handoff = PanelHandoff::default();
        let now = Instant::now();
        handoff.sample(false, (0.0, 0.0), true, now, 0.320);
        handoff.sample(true, (0.0, 480.0), true, now, 0.320);
        let later = now + Duration::from_millis(180);
        handoff.sample(true, (0.0, 480.0), true, later, 0.320);
        let alpha = handoff.opacity();
        handoff.sample(false, (0.0, 0.0), true, later, 0.320);
        assert_eq!(handoff.opacity(), alpha);
        assert!(!handoff.sample(false, (0.0, 0.0), false, later, 0.320));
        assert_eq!(handoff.opacity(), 1.0);
    }

    #[test]
    fn ordinary_resizing_and_same_column_navigation_do_not_fade() {
        let mut handoff = PanelHandoff::default();
        let now = Instant::now();
        for (docked, width) in [(true, 0.0), (true, 480.0), (true, 0.0), (false, 0.0)] {
            assert!(!handoff.sample(docked, (0.0, width), true, now, 0.320));
        }
    }
}
