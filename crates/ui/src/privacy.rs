//! Account identities share one privacy treatment, including email fallbacks
//! used in place of a display name.

use gpui::{AnyElement, App, SharedString, div, prelude::*};

use crate::{icons, settings, theme::Theme, typography::ui_rems};

#[derive(Debug, PartialEq, Eq)]
enum IdentityLabel {
    Visible(SharedString),
    HiddenEmail,
}

fn label(text: SharedString, hide_emails: bool) -> IdentityLabel {
    let has_email = text
        .split_once('@')
        .is_some_and(|(local, domain)| !local.trim().is_empty() && !domain.trim().is_empty());
    if hide_emails && has_email {
        IdentityLabel::HiddenEmail
    } else {
        IdentityLabel::Visible(text)
    }
}

pub fn emails_hidden(cx: &App) -> bool {
    settings::current(cx).blur_emails
}

pub fn set_emails_hidden(hidden: bool, cx: &mut App) {
    if settings::update(settings::SavePolicy::Immediate, cx, |settings| {
        settings.blur_emails = hidden;
    }) {
        cx.refresh_windows();
    }
}

pub fn identity(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    if let IdentityLabel::Visible(text) = label(text.into(), emails_hidden(cx)) {
        return div().min_w_0().truncate().child(text).into_any_element();
    }

    // The mask contains only placeholder text, with transparent space for the
    // blur to fade out. No address or backdrop is drawn beneath it.
    div()
        .relative()
        .min_w_0()
        .w(ui_rems(128.0))
        .max_w_full()
        .child(div().invisible().child("Email hidden"))
        .child(
            icons::icon(icons::EMAIL_HIDDEN)
                .absolute()
                .inset_0()
                .size_full()
                .text_color(Theme::of(cx).text_muted),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_mask_fades_to_transparent_before_its_edges() {
        let renderer = gpui::SvgRenderer::new(std::sync::Arc::new(icons::Assets));
        for scale in [1.0, 1.25, 2.0] {
            let image = renderer
                .render_single_frame(include_bytes!("../assets/icons/email-hidden.svg"), scale)
                .unwrap();
            let size = image.size(0);
            let width = size.width.0 as usize;
            let height = size.height.0 as usize;
            let alpha: Vec<u8> = image
                .as_bytes(0)
                .unwrap()
                .chunks_exact(4)
                .map(|pixel| pixel[3])
                .collect();
            assert!(alpha.iter().any(|alpha| *alpha > 30), "mask is invisible");
            assert!(
                alpha
                    .iter()
                    .copied()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    > 20,
                "mask lost its soft falloff"
            );
            for x in 0..width {
                assert!(
                    alpha[x] <= 1 && alpha[(height - 1) * width + x] <= 1,
                    "vertical blur is clipped"
                );
            }
            for y in 0..height {
                assert!(
                    alpha[y * width] <= 1 && alpha[y * width + width - 1] <= 1,
                    "horizontal blur is clipped"
                );
            }
        }
    }

    #[test]
    fn privacy_removes_addresses_but_keeps_names_and_github_handles() {
        for address in [
            "person@example.com",
            "Person <person@example.com>",
            "person@localhost",
        ] {
            assert_eq!(label(address.into(), true), IdentityLabel::HiddenEmail);
            assert_eq!(
                label(address.into(), false),
                IdentityLabel::Visible(address.into())
            );
        }
        for name in ["Person", "@person", "Unknown account"] {
            assert_eq!(
                label(name.into(), true),
                IdentityLabel::Visible(name.into())
            );
        }
    }
}
