# Ask: one guard before every host call from the app's VM

**Priority: high (security).** A template in preview can still reach the network.

**Seen on:** zgx `f81d5e99` (Zeron's pin). Lane: zgx security follow-up (0.1.10).

**Today.** While a template runs as a preview, Zeron refuses its dangerous host actions (src/plugins/preview.rs `BLOCKED`). It does that with a one-line guard at the top of each action, about 55 sites across a dozen files. Two gaps follow from that:

- `ui.fetch` and `ui.socket` (zgx_run's fetcher and connector) aren't Zeron's actions, so a preview can still read the app's data and send it anywhere.
- An action added later without its guard is open in preview.

**Ask:** `ScriptHost::guard(|kind, name| -> Result<(), String>)`, consulted before any action, async_action, function, fetch or socket the app's VM makes (`kind` saying which; `name` the action's, or the request's URL). An `Err` refuses the call with that message.

**Zeron, the day it lands:** one guard calling `preview::refuse` (fetch and socket refused in preview), drop the per-site guards, and test that a preview's `ui.fetch` is refused.
