-- Setup, this device's third step, only when you're signed in: GitHub, for
-- Cloud agents. They work in your repositories on machines of their own,
-- and GitHub is how they clone and push. Optional, and said so: Skip for
-- now stands beside the way on until it's connected, and it can be done
-- later in Settings → Cloud.
--
-- The block is Settings → Cloud's own GitHub block (settings/cloud.rs
-- render_github and its device flow), as the real step embeds it: GitHub's
-- row, what it's doing on the row's second line, and its one action at the
-- end. Connect asks GitHub for a code (Contacting GitHub…); then a second
-- row shows the code to enter there (a click copies it) and the way to
-- GitHub, while the first waits for the approval, which moves the page on
-- by itself; a code that expires or is turned down says so and offers
-- another go. Connected, the account, and the repositories Cloud can open
-- (chosen on GitHub, not here). Continue opens once it's connected.
--
-- The block keeps the room of its tallest state (the code) in every state, so the title and
-- the footer never move.

local ds = require("@ds")
local kit = require("@demos/example_kit")
local GitHubBlock = require("@ds/github_block")

local shown = ui.state("ex_gh_state", "idle" :: GitHubBlock.Phase)
local copied = ui.state("ex_gh_copied", false)

-- The code GitHub gives, the account and its repositories (fictional).
local CODE = "8F2C-71DA"
local ACCOUNT = "mara-okafor"
local REPOS = { "halcyon-labs/atlas", "halcyon-labs/web", "mara-okafor/dotfiles" }

return {
	id = "ex_github",
	title = "Connect GitHub",
	about = "This device's third setup step, only when signed in: GitHub for Cloud agents, optional, with Skip for now until it's connected. Settings → Cloud's own GitHub block: Connect, the code to enter on GitHub while it waits for the approval, a code that expired, and the account and its repositories once connected.",
	demo = function()
		local function connected(): boolean
			return shown:get() == "connected"
		end
		-- (ds.GitHubBlock, the app's own step's; the device flow faked here.)
		local block = ds.GitHubBlock {
			state = function()
				return shown:get()
			end,
			code = CODE,
			account = ACCOUNT,
			repos = REPOS,
			copied = copied,
			on_connect = function()
				local from = shown:peek()
				shown:set(if from == "failed" then "waiting" else "starting")
				if from ~= "failed" then
					ui.after(900, function()
						if shown:peek() == "starting" then
							shown:set("waiting")
						end
					end)
				end
			end,
			on_cancel = function()
				shown:set("idle")
			end,
			on_disconnect = function()
				shown:set("idle")
			end,
		}
		return kit.stage {
			states = {
				{ value = "idle", label = "Not connected" },
				{ value = "starting", label = "Contacting" },
				{ value = "waiting", label = "Code" },
				{ value = "failed", label = "Expired" },
				{ value = "connected", label = "Connected" },
			},
			state = shown,
			w = 680,
			h = 500,
			-- GitHub joins the steps only signed in: the third of four.
			kit.step {
				step = 3,
				count = 4,
				back = kit.back {},
				aside = ui.show(function()
					return not connected()
				end, kit.quiet { label = "Skip for now" }),
				continue = {
					label = "Continue",
					disabled = function()
						return not connected()
					end,
				},
				kit.form {
					wide = true,
					title = "Connect GitHub",
					body = "Cloud agents work in your repositories on machines of their own. Connect GitHub so they can clone and push.",
					block,
				},
			},
		}
	end,
}
