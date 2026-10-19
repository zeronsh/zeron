//! Settings → Appearance → Home visual: the user's own Luau visual, drawn
//! below the blank new-thread composer. The script is the format agents
//! show with `show_visual`; it is checked the way the engine checks theirs
//! before it is saved, so a broken script never reaches the home view.

use crate::settings::script_card::{Commit, ScriptSpec};

/// The visual's id, which also seeds its randomness: the check and the
/// home view see the same random data.
pub const HOME_VISUAL_ID: &str = "home-visual";

pub static SPEC: ScriptSpec = ScriptSpec {
    key: "home-visual",
    title: "Visual script",
    empty_status: "A Luau visual, in the format agents use with show_visual.",
    saved: |settings| settings.new_thread_visual.clone(),
    saved_status: |_| "Showing below the new-session composer.",
    check,
    clear: |cx| crate::settings::set_new_thread_visual(None, cx),
};

fn check(script: String) -> Result<Commit, String> {
    zeron_visual::check(&script, zeron_visual::seed_for(HOME_VISUAL_ID))
        .map_err(|error| error.message)?;
    Ok(Box::new(move |cx| {
        crate::settings::set_new_thread_visual(Some(script), cx)
    }))
}
