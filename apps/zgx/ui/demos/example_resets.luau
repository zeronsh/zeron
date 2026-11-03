-- Settings: Codex's banked resets. Codex lets an account bank resets of
-- its usage limits; Zeron shows them where they matter, on the Codex
-- account's own row: how many are banked in its detail line, and, once a
-- window is at its limit, "Use a reset" right there. Using one is a
-- dialog: which (the soonest to expire first), what it does, one button.
-- The row then says the limits started over. Whatever else Codex says
-- lands on the same row: the reset under way, kept (no window needed it),
-- or why it couldn't be used, with another go. With none banked there's no
-- row. Only Codex accounts have any of this. The pieces are in
-- providers_parts.luau; the card and the dialog are ds.ProviderCard's.

local ds = require("@ds")
local kit = require("@demos/example_kit")
local parts = require("@demos/providers_parts")

local shown = ui.state("ex_rs_state", "limit")

return {
	id = "ex_resets",
	title = "Codex resets",
	about = "Settings → Providers, the Codex card: the resets an account has banked, on the row after it; at a limit, a way to use one there; the dialog that spends it (the soonest to expire first); the row while it's used, once the limits have started over, when no window needed it, and when it couldn't be used; and none banked.",
	demo = function()
		ui.effect(function()
			local s = shown:get()
			parts.codex_reset:set(if s == "limit" or s == "choosing" then "banked" else s)
			parts.reset_open:set(s == "choosing")
		end)
		local codex = parts.INSTALLED[2]
		return kit.stage {
			states = {
				{ value = "limit", label = "At the limit" },
				{ value = "choosing", label = "Choosing" },
				{ value = "using", label = "Using" },
				{ value = "used", label = "Used" },
				{ value = "kept", label = "Kept" },
				{ value = "failed", label = "Failed" },
				{ value = "none", label = "None banked" },
			},
			state = shown,
			w = 900,
			h = 640,
			overlays = { parts.resets_dialog() },
			kit.settings {
				page = "providers",
				ds.Settings.Page {
					title = "Providers",
					description = "The coding agents on a device, and the accounts they sign in with.",
					trail = ds.Select { value = parts.device, name = "Device", size = "sm", w = 200, options = parts.DEVICES :: any },
					-- (The card follows its resets in place.)
					parts.card(codex, "idle"),
				},
			},
		}
	end,
}
