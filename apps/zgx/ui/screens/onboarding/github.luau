-- GitHub, for Cloud agents, only when signed in: they work in your
-- repositories on machines of their own, and GitHub is how they clone and
-- push. Optional, and said so: Skip for now stands beside the way on until
-- it's connected, and it can be done later in Settings → Cloud. The
-- gallery's Connect GitHub is the design (ds.GitHubBlock, Settings →
-- Cloud's block).
--
-- The device flow is the engine's (account/github): Connect asks GitHub for
-- a code, the block shows it (a click copies it; Open GitHub copies it and
-- opens GitHub's page), and the step moves to connected once it's approved
-- there. An account Cloud isn't offered to says so in the block's own row,
-- and Skip for now is the way on.

local ds = require("@ds")
local github = require("@account/github")
local state = require("./state")
local Onboarding = ds.Onboarding

export type Props = { step: number?, count: number?, on_back: () -> (), on_continue: () -> () }

local function now(): github.Now
	return github.now:get() :: github.Now
end

-- The block's phase: checking where the account stands reads as contacting
-- GitHub; an account without Cloud as the row's failure, with another go.
local function phase(): any
	local p = now().phase
	if p == "checking" then
		return "starting"
	elseif p == "unavailable" then
		return "failed"
	end
	return p
end

return function(props: Props): Node
	-- (Once a visit: a hot reload doesn't ask again.)
	state.once_a_visit("github", github.load)
	local copied = ui.use_state(false)
	local function connected(): boolean
		return now().phase == "connected"
	end
	local function copy()
		local code = now().code
		if code then
			ui.clipboard.write(code)
		end
	end
	return Onboarding.Step {
		step = props.step,
		count = props.count,
		back = Onboarding.Back { on_click = props.on_back },
		aside = ui.show(function()
			return not connected()
		end, Onboarding.Quiet { label = ui.t("onboarding.skip_for_now") :: any, on_click = props.on_continue }),
		continue = {
			label = ui.t("common.continue") :: any,
			disabled = function()
				return not connected()
			end,
			on_click = props.on_continue,
		},
		Onboarding.Form {
			wide = true,
			title = ui.t("onboarding.github.title") :: any,
			body = ui.t("onboarding.github.body") :: any,
			ds.GitHubBlock {
				state = phase,
				copied = copied,
				code = function()
					return now().code
				end,
				account = function()
					return now().account
				end,
				repos = function()
					return now().repos
				end,
				failed = function()
					return now().failure
				end,
				on_connect = function()
					-- Another go: where the account stands, if that's what
					-- failed; otherwise a new code.
					if now().phase == "unavailable" then
						github.load()
					else
						github.connect()
					end
				end,
				on_cancel = github.cancel,
				on_disconnect = github.disconnect,
				on_copy = copy,
				on_open = function()
					copy()
					github.open()
				end,
				on_choose = github.choose,
			},
		},
	}
end
