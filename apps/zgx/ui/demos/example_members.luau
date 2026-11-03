-- Settings: members. Who's in the organization, who's been asked, and what
-- each can do, with seats counted and the ways out asked twice.

local ds = require("@ds")
local kit = require("@demos/example_kit")

local query = ui.state("ex_mem_query", "")
local shown = ui.state("ex_mem_state", "ready")
local removing = ui.state("ex_mem_removing", false)
local inviting = ui.state("ex_mem_inviting", false)
local invitees = ui.state("ex_mem_invitees", {} :: { string })

type Member = { id: string, name: string, email: string, role: string, you: boolean?, seen: string }

local MEMBERS: { Member } = {
	{ id = "mara", name = "Mara Okafor", email = "mara@halcyon.dev", role = "owner", you = true, seen = "Active now" },
	{ id = "theo", name = "Theo Lindqvist", email = "theo@halcyon.dev", role = "admin", seen = "Active 2 hours ago" },
	{ id = "priya", name = "Priya Raman", email = "priya@halcyon.dev", role = "member", seen = "Active yesterday" },
	{ id = "jonas", name = "Jonas Weber", email = "jonas@halcyon.dev", role = "member", seen = "Active 3 days ago" },
	{ id = "ines", name = "Ines Duarte", email = "ines@halcyon.dev", role = "viewer", seen = "Active last week" },
}
local INVITES = {
	{ email = "sam@halcyon.dev", when = "Invited 2 days ago · Member" },
	{ email = "lena@northwind.io", when = "Invite expired · Viewer", expired = true },
}
local ROLES = {
	{ value = "admin", label = "Admin" },
	{ value = "member", label = "Member" },
	{ value = "viewer", label = "Viewer" },
}

-- The members the search finds.
local found = ui.computed(function(): { Member }
	local q = string.lower(query:get())
	local out: { Member } = {}
	for _, m in MEMBERS do
		if q == "" or string.find(string.lower(m.name .. " " .. m.email), q, 1, true) then
			table.insert(out, m)
		end
	end
	return out
end)

local function row(m: Member): Node
	local control: Node
	if m.role == "owner" then
		control = ds.Badge { "Owner", size = "md" }
	else
		control = ui.row {
			gap = "$space.1",
			items = "center",
			ds.Select { value = ui.state(`ex_mem_role_{m.id}`, m.role), name = `{m.name}'s role`, size = "sm", w = 108, options = ROLES },
			ds.IconButton { icon = "more-horizontal", label = `More for {m.name}`, size = "md", tone = "quiet", on_click = function() removing:set(true) end },
		}
	end
	return ds.Settings.Row {
		label = m.name,
		description = `{m.email} · {m.seen}`,
		lead = ds.Avatar { seed = m.id, size = 32 },
		badge = if m.you then ds.Badge { "You" } else nil,
		control = control,
	}
end

-- A pending invitation: who, and the ways to send it again or take it back.
local function invitation(i: any): Node
	return ds.Settings.Row {
		label = i.email,
		description = i.when,
		lead = ds.Avatar { seed = i.email, size = 32 },
		badge = if i.expired then ds.Badge { "Expired", tone = "warning" } else nil,
		control = ui.row {
			gap = "$space.1",
			ds.Button { "Resend", tone = "ghost", size = "sm" },
			ds.Button { "Revoke", tone = "ghost", size = "sm" },
		},
	}
end

return {
	id = "ex_members",
	title = "Members",
	about = "Settings → Members: everyone in the organization, found by name, each with a role to change and a way out; the invitations still open, with the seats they hold. And what shows while they load, when they fail, and when a search finds no one.",
	demo = function()
		local function body(key: string): Node
			if key == "loading" then
				return ds.Settings.Group { ds.SkeletonRow { trail = 108 }, ds.SkeletonRow { title = 110, line = 200, trail = 108 }, ds.SkeletonRow { title = 130, line = 220, trail = 108 }, ds.SkeletonRow { title = 100, line = 180, trail = 108 } }
			elseif key == "failed" then
				return ds.Callout { tone = "danger", title = "Couldn't load members", body = "Something went wrong on our side. Your team's access hasn't changed.", action = { label = "Try again", on_click = function() shown:set("ready") end } }
			end
			if key == "nobody" then
				return ds.EmptyState { icon = "search", title = "No one by that name", body = "Check the spelling, or invite them.", size = "sm", action = { label = "Invite someone", on_click = function() inviting:set(true) end } }
			end
			-- Those the search finds, each row its member's as the search
			-- narrows and widens.
			return ds.Settings.Each(found, row, {
				key = function(m: any): string
					return m.id
				end,
			})
		end
		return kit.stage {
			states = {
				{ value = "ready", label = "Ready" },
				{ value = "loading", label = "Loading" },
				{ value = "failed", label = "Failed" },
			},
			state = shown,
			w = 900,
			h = 640,
			overlays = {
				ds.Dialog {
					open = removing,
					title = "Remove Priya Raman?",
					body = "They lose access to Halcyon Labs at once. Chats they started stay with the team.",
					tone = "danger",
					confirm = { label = "Remove" },
				},
				ds.Dialog {
					open = inviting,
					title = "Invite to Halcyon Labs",
					body = "3 seats left. Each invitation holds one for 7 days.",
					size = "md",
					confirm = { label = "Send invites" },
					ds.TokenField { value = invitees, name = "Email addresses", placeholder = "name@company.com", w_full = true, autofocus = true },
				},
			},
			kit.settings {
				page = "members",
				ds.Settings.Page {
					title = "Members",
					description = "Everyone in Halcyon Labs, and what they can do here.",
					-- The search beside the one action, over the list it filters.
					trail = ui.row {
						gap = "$space.2",
						items = "center",
						ds.SearchField { value = query, placeholder = "Find someone", size = "sm", w = 180 },
						ds.Button { "Invite", size = "sm", on_click = function() inviting:set(true) end },
					},
					ds.Settings.Section {
						ui.presence(function(): string
							local s = shown:get()
							if s ~= "ready" then
								return s
							end
							return if #found:get() > 0 then "found" else "nobody"
						end, body, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
					},
					-- Invitations hold seats, so the seats stand at the head of them.
					ds.Settings.Section {
						title = "Invitations",
						ds.Settings.Group {
							ds.Settings.Row {
								label = "7 of 10 seats used",
								description = "Pending invitations hold a seat until they expire.",
								control = ui.row { gap = "$space.3", items = "center", ds.ProgressBar { value = function() return 0.7 end, w = 120, label = "Seats used" }, ds.Button { "Add seats", tone = "secondary", size = "sm" } },
							},
							invitation(INVITES[1]),
							invitation(INVITES[2]),
						},
					},
				},
			},
		}
	end,
}
