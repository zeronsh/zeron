-- Zeron in zgx: the windows' entry. The design lint first (before anything
-- builds), then the design system's themes, then the window's screen: the
-- host says which (`window`): setup ("onboarding", the default), or the
-- main window ("main": the shell, screens/shell). Each screen is required
-- as `@app/screens/…`, so a trusted plugin can replace it whole.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
-- The language chosen (Settings → General), before any screen builds.
require("@app/screens/language")

if ui.shared("window", "onboarding"):peek() == "main" then
	-- Plugins: their page in Settings, and what they're shown of Zeron.
	require("@app/screens/plugins")
	local shell = require("@app/screens/shell")
	return shell()
end

local onboarding = require("@app/screens/onboarding")
local state = require("@app/screens/onboarding/state")
local actions = require("@app/screens/onboarding/actions")

-- The look chosen in an earlier run.
actions.appearance(state.appearance:peek())

return onboarding()
