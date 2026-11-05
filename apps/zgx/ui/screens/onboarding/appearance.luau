-- Make it yours, the first step after sign-in (signed in or not), so the
-- rest of setup is seen in the look chosen: the colour scheme, the
-- window's material and the accent (ds.AppearancePicker, the gallery's
-- Make it yours). Each is the app's own setting and takes effect as it's
-- picked: the scheme on the theme and the window's chrome, the material on
-- the frame (ds.material), the accent on the frame's tokens (ds.look). All
-- of it is in Settings → Appearance too.
--
-- Signed in, the way back is Sign out: back from here is Sign in, which
-- an account has passed, so the bottom left leaves the account instead.

local ds = require("@ds")
local Onboarding = ds.Onboarding
local auth = require("@account/auth")
local state = require("./state")
local actions = require("./actions")

export type Props = { step: number?, count: number?, on_back: () -> (), on_continue: () -> () }

return function(props: Props): Node
	return Onboarding.Step {
		step = props.step,
		count = props.count,
		back = if state.path:peek() == "signed_in"
			then Onboarding.Quiet {
				label = ui.t("onboarding.appearance.sign_out") :: any,
				on_click = function()
					auth.sign_out(actions.signed_out)
				end,
			}
			else Onboarding.Back { on_click = props.on_back },
		continue = { label = ui.t("common.continue") :: any, on_click = props.on_continue },
		Onboarding.Form {
			wide = true,
			title = ui.t("onboarding.appearance.title") :: any,
			body = ui.t("onboarding.appearance.body") :: any,
			ds.AppearancePicker {
				scheme = state.appearance :: any,
				frosted = ds.material.frosted,
				accent = ds.look.accent,
			},
		},
	}
end
