//! Settings → Appearance → Generative background: the user's Luau
//! `ui.shader`, drawn behind the new-thread composer in place of the image.
//! Applying it checks that it runs and shows a shader, samples one frame's
//! color for "Match wallpaper colors", and makes it the background.

use crate::settings::NewThreadBackgroundSource;
use crate::settings::script_card::{Commit, ScriptSpec};

pub static SPEC: ScriptSpec = ScriptSpec {
    key: "generative-art",
    title: "Shader script",
    empty_status: "A Luau script returning a ui.shader. It moves, but takes no input.",
    saved: |settings| settings.new_thread_generative_art.clone(),
    saved_status: |settings| match settings.new_thread_background_source {
        NewThreadBackgroundSource::Generative => "Showing behind the new-session composer.",
        NewThreadBackgroundSource::Image => "Saved. Choose Generative above to show it.",
    },
    check,
    clear: |cx| crate::settings::set_new_thread_generative_art(None, cx),
};

fn check(script: String) -> Result<Commit, String> {
    let color = crate::new_thread_generative::check(&script)?;
    Ok(Box::new(move |cx| {
        crate::settings::set_new_thread_generative_art(Some((script, color)), cx)
    }))
}
