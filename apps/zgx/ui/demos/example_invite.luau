-- Setup, step two: inviting teammates. Emails become chips as they are
-- typed; one that can't be sent to stays and says so. The way on is in the
-- footer, and the step can be skipped from there. (The app's step:
-- screens/onboarding/invite.luau.)

local ds = require("@ds")
local kit = require("@demos/example_kit")

local emails = ui.state("ex_inv_emails", { "theo@halcyon.dev", "priya@halcyon.dev" })
local shown = ui.state("ex_inv_state", "typing")
local sent = ui.state("ex_inv_sent", false)

local function valid(s: string): boolean
	return string.match(s, "^[^@%s]+@[^@%s]+%.[^@%s]+$") ~= nil
end
local function bad(list: { string }): number
	local n = 0
	for _, e in list do
		n += if valid(e) then 0 else 1
	end
	return n
end

-- Who got one.
local INVITED = {
	{ id = "theo", name = "Theo Lindqvist", email = "theo@halcyon.dev" },
	{ id = "priya", name = "Priya Raman", email = "priya@halcyon.dev" },
}

return {
	id = "ex_invite",
	title = "Invite your teammates",
	about = "Setup's step after naming the organization. Emails turning into chips as they are typed, one that can't be sent to saying so, and the step once the invites have gone.",
	demo = function()
		ui.effect(function()
			local s = shown:get()
			sent:set(s == "sent")
			if s == "typing" then
				emails:set({ "theo@halcyon.dev", "priya@halcyon.dev" })
			elseif s == "invalid" then
				emails:set({ "theo@halcyon.dev", "priya@halcyon.dev", "ines@halcyon" })
			elseif s == "empty" then
				emails:set({})
			end
		end)
		local function problem(): string?
			local n = bad(emails:get())
			if n == 0 then
				return nil
			end
			return if n == 1 then "One address doesn't look right." else `{n} addresses don't look right.`
		end
		local form = ui.column {
			w_full = true,
			gap = "$space.4",
			ds.TokenField {
				value = emails,
				name = "Email addresses",
				placeholder = "name@company.com",
				valid = valid,
				error = problem,
				hint = "Enter or a comma adds another. They join as members.",
				-- Setup's fields are the large size (the name's, the username's).
				size = "lg",
				w_full = true,
				autofocus = true,
			},
		}
		local rows: { Node } = {}
		for _, p in INVITED do
			table.insert(rows, ds.Settings.Row {
				label = p.name,
				description = p.email,
				lead = ds.Avatar { seed = p.id, size = 32, label = p.name },
				control = ds.Badge { "Invited", tone = "info" },
			})
		end
		local done = ui.column {
			w_full = true,
			gap = "$space.4",
			ds.Settings.Group(rows :: any),
		}
		return kit.stage {
			states = {
				{ value = "typing", label = "Typing" },
				{ value = "invalid", label = "Invalid" },
				{ value = "empty", label = "Empty" },
				{ value = "sent", label = "Sent" },
			},
			state = shown,
			w = 680,
			h = 500,
			kit.step {
				step = 2,
				count = 3,
				back = kit.back {},
				aside = ui.show(function()
					return not sent:get()
				end, kit.quiet { label = "Skip for now" }),
				continue = {
					label = function()
						if sent:get() then
							return "Continue"
						end
						local n = #emails:get()
						return if n == 0 then "Send invites" elseif n == 1 then "Send 1 invite" else `Send {n} invites`
					end,
					disabled = function()
						if sent:get() then
							return false
						end
						local list = emails:get()
						return #list == 0 or bad(list) > 0
					end,
					on_click = function()
						if not sent:peek() then
							shown:set("sent")
						end
					end,
				},
				kit.form {
					title = "Invite your teammates",
					body = function()
						return if sent:get()
							then "2 invites sent. They'll get an email from Zeron."
							else "Chats and agents you share stay in step for everyone."
					end,
					ui.show(function()
						return sent:get()
					end, done, form),
				},
			},
		}
	end,
}
