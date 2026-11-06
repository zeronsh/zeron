//! The `imgui.windows` permission: the one gate every native ImGui host
//! action passes through.
//!
//! A manifest asks for `imgui.windows`; [`grants`](super::super::grants)
//! turns it into [`CAPABILITY`], and that capability is the only one the
//! ImGui host actions are exposed behind, so a VM not granted it cannot
//! reach them at all. Each call checks the same thing again against the
//! plugin's listing ([`allowed`]): what a VM was granted when it loaded can
//! be older than what the engine says now, so a plugin turned off, or one
//! whose grant was taken away, is refused even while its script still runs.
//!
//! `windows` (zgx's declarative windows) is deliberately not accepted here:
//! the two permissions are mapped to different capabilities and neither
//! stands in for the other. Nothing in this module opens a window; the
//! window host and the frame pump call it.

use zgx_script::{Capability, ScriptHost};

use super::super::{Item, grants};

/// The permission a manifest asks for (`"permissions": ["imgui.windows"]`).
pub const PERMISSION: &str = "imgui.windows";

/// The capability that permission grants the plugin's VM.
pub const CAPABILITY: &str = grants::IMGUI_WINDOWS;

/// The capability itself, as `ScriptHost::expose_action` takes it.
pub fn capability() -> Capability {
    Capability::Named(CAPABILITY.to_string())
}

/// Exposes `name`, one of a plugin's ImGui host actions, to the VMs granted
/// `imgui.windows` alone. A plugin asking for anything else, `windows`
/// included, cannot bind, invoke or call it.
pub fn expose_action(host: &ScriptHost, name: impl Into<String>) {
    host.expose_action(name, Some(capability()));
}

/// Whether `item` may use the ImGui host now: it is on, its manifest asked
/// for `imgui.windows`, the VM grant that comes from the same list is held,
/// and the engine (when its listing says) still allows it. `asking`
/// completes "…wasn't allowed to …" in the refusal.
pub fn allowed(item: Option<&Item>, asking: &str) -> Result<(), String> {
    let Some(item) = item else {
        return Err(not_on(asking));
    };
    // The listing is the authority on whether it runs at all.
    if item.status() != "on" {
        return Err(not_on(asking));
    }
    // The VM's capability and the listing's permission come from one
    // manifest, so they agree; checked here anyway so a listing that says
    // one thing and a grant that says another fails closed.
    let granted = grants::capabilities(
        item.id(),
        &item.permissions(),
        &item.change(),
        &replace_of(item),
    );
    if !granted.contains(&capability()) {
        return Err(not_allowed(asking));
    }
    if !item.may(PERMISSION) {
        return Err(not_allowed(asking));
    }
    // A grant taken back leaves the plugin on: what the engine allows is
    // then narrower than what it asked for, and narrower than the VM's
    // grant until the next reload.
    if let Some(now) = allowed_permissions(item)
        && !now.iter().any(|p| p == PERMISSION)
    {
        return Err(not_allowed(asking));
    }
    Ok(())
}

/// The gate `zeron.imgui.open_window` passes before anything is created.
pub fn open_window(item: Option<&Item>) -> Result<(), String> {
    allowed(item, "open native application windows")
}

/// What a plugin's own listing says the engine allowed, when it says
/// (`Item::permissions` is what it asked). Absent in a dev plugin's made-up
/// listing, which then falls back to its asked permissions alone.
fn allowed_permissions(item: &Item) -> Option<Vec<String>> {
    let list = item.json.get("allowed")?.as_array()?;
    Some(
        list.iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
    )
}

/// Its `ui.replace` modules, as `grants::capabilities` reads them.
fn replace_of(item: &Item) -> Vec<String> {
    item.json["ui"]["replace"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect()
}

/// A refusal for a plugin that isn't running (off, waiting, changed, or
/// broken): keeping it off is what keeps every window from opening.
fn not_on(asking: &str) -> String {
    format!(
        "This plugin isn't on, so it can't {asking}. Turn it on and allow \"{PERMISSION}\" in Zeron first."
    )
}

/// A refusal for a running plugin whose grant is missing or was withdrawn.
fn not_allowed(asking: &str) -> String {
    format!("This plugin wasn't allowed to {asking}. Allow \"{PERMISSION}\" for it in Zeron.")
}
