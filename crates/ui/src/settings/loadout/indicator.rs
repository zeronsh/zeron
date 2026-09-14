use super::*;
use gpui::AnimationExt;

const DROP_LABEL: &str = "DROP MODEL HERE";
const EMPTY_LABEL: &str = "EMPTY";

fn vacant_indicator_label(first_empty: bool) -> &'static str {
    if first_empty { DROP_LABEL } else { EMPTY_LABEL }
}

fn vacant_indicator_is_active(first_empty: bool, drag_over: bool) -> bool {
    first_empty && drag_over
}

pub(super) fn vacant_indicator(
    index: usize,
    first_empty: bool,
    drag_over: bool,
    theme: &Theme,
    cx: &mut Context<LoadoutPage>,
) -> gpui::AnyElement {
    let mut body = div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center();

    if first_empty {
        body = body.debug_selector(|| "loadout-first-vacant".into());
        let active = vacant_indicator_is_active(first_empty, drag_over);
        let phase = active
            .then(|| crate::motion::pulse_delta(&crate::motion::GRADIENT_SPIN, cx.entity_id(), cx));
        let mut file = div()
            .size(px(20.0))
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme.text_muted.opacity(if active { 0.7 } else { 0.35 }))
            .child(icon(icons::DOCUMENT_ADD).size(px(16.0)));

        if let Some(phase) = phase {
            let wave = crate::motion::pulse_wave(phase);
            file = file
                .relative()
                .top(px(1.5 - 3.0 * wave))
                .opacity(0.65 + 0.35 * wave);
        }

        body = body.child(file).child(
            div()
                .mt(px(6.0))
                .text_size(px(10.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text_muted.opacity(if drag_over { 0.7 } else { 0.45 }))
                .child(SharedString::from(vacant_indicator_label(true))),
        );
    } else {
        body = body.child(
            div()
                .text_size(px(10.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text_muted.opacity(0.45))
                .child(SharedString::from(vacant_indicator_label(false))),
        );
    }

    let body = body.id(("loadout-vacant-indicator", index));
    if first_empty {
        body.with_animation(
            ("loadout-vacant-entrance", index),
            crate::motion::FADE_QUICK.animation(),
            |element, t| element.relative().opacity(t).top(px(-4.0 * (1.0 - t))),
        )
        .into_any_element()
    } else {
        body.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_vacant_slot_gets_the_drop_prompt() {
        assert_eq!(vacant_indicator_label(true), DROP_LABEL);
        assert_eq!(vacant_indicator_label(false), EMPTY_LABEL);
    }

    #[test]
    fn animation_only_runs_for_the_first_slot_under_drag() {
        assert!(vacant_indicator_is_active(true, true));
        assert!(!vacant_indicator_is_active(true, false));
        assert!(!vacant_indicator_is_active(false, true));
    }
}
