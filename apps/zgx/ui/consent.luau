-- Allowing a plugin, in a window of its own (src/plugins/consent.rs opens
-- it over the main window). Nothing else runs here: no plugin, no
-- template, none of the main window's screens. That's what makes its
-- Allow yours alone: the main window's scripts, which plugins and your
-- templates may stand in for, can ask for this window but never answer it.

ui.theme.strict("warn")
local ds = require("@ds")
local material = require("@ds/material")
ds.setup()

local consent = require("@screens/plugins/consent")

-- The look chosen in setup, as the main window has it.
local onboarding = require("@screens/onboarding/state")
local actions = require("@screens/onboarding/actions")
actions.apply_scheme(onboarding.appearance:peek())

return ui.column {
	w_full = true,
	h_full = true,
	-- The window's glass, or (opaque) its colour: the main window's.
	bg = function(): string
		return if material.frosted:get() then "$zeron.window" else "$background"
	end,
	consent.View(),
}
