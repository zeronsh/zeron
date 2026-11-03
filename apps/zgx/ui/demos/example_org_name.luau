-- Setup, step one: naming the organization. The name is the whole screen:
-- one field, the address it makes on the line under it (checked a beat
-- after the typing stops: being checked, free, or taken), and the way on
-- under that. Nothing else to look at.

local ds = require("@ds")
local kit = require("@demos/example_kit")

local name = ui.state("ex_org_name", "Halcyon Labs")
local shown = ui.state("ex_org_state", "ready")
local check = ui.state("ex_org_check", "free")
local failed = ui.state("ex_org_failed", false)

-- Addresses already spoken for.
local TAKEN = { halcyon = true, acme = true }

local function slug(s: string): string
	local out = string.gsub(string.lower(s), "[^%w]+", "-")
	return (string.gsub(out, "^%-*(.-)%-*$", "%1"))
end

local attempt = 0
local function look_up(s: string)
	attempt += 1
	local mine = attempt
	if s == "" then
		check:set("empty")
		return
	end
	check:set("checking")
	ui.after(650, function()
		if mine == attempt then
			check:set(if TAKEN[s] then "taken" else "free")
		end
	end)
end

return {
	id = "ex_org_name",
	title = "Name your organization",
	about = "The first step of setup. One field for the name; the address it makes on the line under it, checked a beat after the typing stops; and the way on under that.",
	demo = function()
		ui.effect(function()
			local s = shown:get()
			failed:set(s == "failed")
			if s == "empty" then
				name:set("")
			elseif s == "taken" then
				name:set("Acme")
			elseif s == "ready" or s == "failed" then
				name:set("Halcyon Labs")
			end
		end)
		ui.effect(function()
			look_up(slug(name:get()))
		end)
		local address = ds.AddressLine {
			prefix = "zeron.sh/",
			value = function()
				return slug(name:get())
			end,
			status = function()
				return check:get()
			end,
		}
		return kit.stage {
			states = {
				{ value = "ready", label = "Ready" },
				{ value = "empty", label = "Empty" },
				{ value = "taken", label = "Taken" },
				{ value = "failed", label = "Failed" },
			},
			state = shown,
			w = 680,
			h = 500,
			-- The first step: nothing before it to go back to, so no Back.
			kit.step {
				step = 1,
				count = 3,
				continue = {
					label = function()
						return if failed:get() then "Try again" else "Continue"
					end,
					disabled = function()
						return slug(name:get()) == "" or check:get() == "taken"
					end,
					on_click = function()
						if failed:peek() then
							shown:set("ready")
						end
					end,
				},
				kit.form {
					title = "Name your organization",
					-- Gone wrong, the line under the title says so, in the
					-- danger's ink, and the button offers another go: nothing
					-- else moves.
					body = function()
						return if failed:get()
							then `Couldn't create {name:get()}. Nothing was saved.`
							else "It's what your teammates see when they join."
					end,
					tone = function()
						return if failed:get() then "$zeron.tone.danger.ink" else "$text.secondary"
					end,
					ui.column {
						w_full = true,
						gap = "$space.2",
						ds.TextField {
							value = name,
							name = "Organization name",
							placeholder = "Your company or team",
							size = "lg",
							w_full = true,
							autofocus = true,
						},
						address,
					},
				},
			},
		}
	end,
}
