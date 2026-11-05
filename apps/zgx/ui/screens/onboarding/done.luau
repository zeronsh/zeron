-- Setup's ending. Signed in: the organization is ready, and what was set up
-- in a few lines (the organization and your place in it, who was invited
-- in this run, where its chats live), and the way into it. Without an
-- account: the chats stay on this Mac, and the way into Zeron. Not a
-- numbered step, so no dots. The gallery's All set is the design.
--
-- Not wired yet: the shell doesn't exist in this app, so the way in
-- finishes setup (the window closes; the shell opens here once it exists).

local ds = require("@ds")
local people_ds = require("@ds/people")
local Onboarding = ds.Onboarding
local state = require("./state")
local actions = require("./actions")

export type Props = { on_back: () -> () }

-- A row's lead for a fact rather than a thing: the gallery's tick.
local function tick(): Node
	return ui.row {
		w = 32,
		h = 32,
		shrink = 0,
		items = "center",
		justify = "center",
		rounded = "$radius.full",
		bg = "$zeron.tone.success.wash",
		color = "$zeron.tone.success.ink",
		ds.Icon { name = "check-circle", size = 16 },
	}
end

-- "a", "a and b", "a, b and 3 more". (Read where it's drawn: it follows
-- the language.)
local function who(list: { string }): string
	if #list == 1 then
		return list[1]
	elseif #list == 2 then
		return ui.t("onboarding.done.who.two", { a = list[1], b = list[2] }):get()
	end
	return ui.t("onboarding.done.who.more", { a = list[1], b = list[2], n = #list - 2 }):get()
end

return function(props: Props): Node
	local signed_in = state.path:peek() == "signed_in"
	local org = state.org_name:peek()
	local owner = state.org_admin:peek()
	local invited = state.invited:peek()

	local rows: { Node } = {}
	if signed_in and org ~= "" then
		table.insert(rows, ds.Settings.Row {
			label = org,
			truncate = true,
			lead = ds.Tile { label = org, hue = "violet", size = 32 },
			badge = ds.Badge { if owner then ui.t("onboarding.done.owner") else ui.t("onboarding.done.member") },
		})
	end
	if signed_in and #invited > 0 then
		local people: { people_ds.Who } = {}
		for _, e in invited do
			table.insert(people, { id = e, name = e })
		end
		table.insert(rows, ds.Settings.Row {
			label = ui.t("onboarding.done.invites_sent", { n = #invited }),
			description = function()
				return ui.t("onboarding.done.invites_join", { n = #invited, who = who(invited) }):get()
			end,
			lead = ds.AvatarStack { people = people, size = 24 },
		})
	end
	if signed_in then
		table.insert(rows, ds.Settings.Row {
			label = ui.t("onboarding.done.synced.label"),
			description = ui.t("onboarding.done.synced.description"),
			lead = tick(),
		})
	else
		table.insert(rows, ds.Settings.Row {
			label = ui.t("onboarding.done.local.label"),
			description = ui.t("onboarding.done.local.description"),
			lead = tick(),
		})
	end

	local named = signed_in and org ~= ""
	return Onboarding.Step {
		back = Onboarding.Back { on_click = props.on_back },
		continue = {
			label = (if named then ui.t("onboarding.done.open_org", { org = org }) else ui.t("onboarding.done.open_zeron")) :: any,
			max_w = 260,
			on_click = actions.finish,
		},
		Onboarding.Form {
			wide = true,
			title = (if named then ui.t("onboarding.done.title_org", { org = org }) else ui.t("onboarding.done.title")) :: any,
			body = (if not named
				then ui.t("onboarding.done.body")
				elseif owner then ui.t("onboarding.done.body_owner")
				else ui.t("onboarding.done.body_member")) :: any,
			ds.Settings.Group(rows :: any),
		},
	}
end
