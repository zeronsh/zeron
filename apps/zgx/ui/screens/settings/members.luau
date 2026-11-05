-- Settings → Members: the organization you work in, everyone in it and
-- what they can do, found by name, and the invitations still open: the
-- gallery's Members (demos/example_members) and its invite dialog
-- (example_invite's chips), on the edge's organization (store.luau).
-- Admins rename it, change roles, remove people, invite, resend and revoke;
-- everyone else sees who's there.
--
-- The lists are keyed by membership and invitation id (`Settings.Each`),
-- so one leaving, arriving or changing role changes that row alone. Only
-- the list's own states swap: loading, failed, found, no one by that name.

local ds = require("@ds")
local Menu = require("@ds/menu")
local api = require("@account/api")
local store = require("@app/screens/settings/store")
local search = require("@app/screens/settings/search")
local locale = require("@ds/locale")
local visits = require("@app/screens/settings/visits")

local M = {}

M.ID = "members"
M.TITLE = "Members"

search.index(M.ID, ui.t("settings.members.title"), ui.t("settings.members.terms"), {
	{
		key = "org_name",
		title = ui.t("settings.members.org_name.label"),
		section = ui.t("settings.members.org_section"),
		terms = ui.t("settings.members.org_name.terms"),
	},
	{ key = "people", title = ui.t("settings.members.people.label"), terms = ui.t("settings.members.people.terms") },
	{ key = "people", title = ui.t("settings.members.invite.label"), terms = ui.t("settings.members.invite.terms") },
	{ key = "invitations", title = ui.t("settings.members.invitations.label"), terms = ui.t("settings.members.invitations.terms") },
})

local query = ui.state("settings_members_query", "")
-- Whose removal is being asked (a membership id; "" none).
local removing = ui.state("settings_members_removing", "")
local removing_open = ui.state("settings_members_removing_open", false)
local inviting = ui.state("settings_members_inviting", false)
local invitees = ui.state("settings_members_invitees", {} :: { string })
local invite_error = ui.state("settings_members_invite_error", nil :: string?)
local sending = ui.state("settings_members_sending", false)
local org_name = ui.state("settings_org_name", "")
local org_touched = ui.state("settings_org_touched", false)

-- (In the locale shown: read where they're drawn.)
local function ROLES(): { any }
	return {
		{ value = "admin", label = ui.t("settings.members.admin"):get() },
		{ value = "member", label = ui.t("settings.members.member"):get() },
	}
end
local function role_label(role: string?): string
	return if role == "admin" then ui.t("settings.members.admin"):get() elseif role == "member" or role == nil then ui.t("settings.members.member"):get() else role
end

-- Those the search finds.
local found = ui.computed(function(): { store.Member }
	local q = string.lower(query:get())
	local out = {}
	for _, m in store.members:get() do
		local hay = string.lower(`{m.name or ""} {m.email or ""} {m.username or ""}`)
		if q == "" or string.find(hay, q, 1, true) then
			table.insert(out, m)
		end
	end
	return out
end)

local function by_id(item: any): string
	return item.id
end

local function member_name(m: store.Member): string
	return m.name or m.email or ui.t("settings.members.someone"):get()
end

-- A member's row: who, and (for an admin, about anyone else) their role
-- and the way out.
local function member_row(_m: store.Member, _i: number, m: State<store.Member>): Node
	local first = m:peek()
	local id = first.id
	local function can_change(): boolean
		return store.admin:get() and not m:get().you
	end
	local role = ui.use_state(first.role or "member")
	ui.effect(function()
		local r = m:get().role or "member"
		if r ~= role:peek() then
			role:set(r)
		end
	end)
	local menu_open = ui.use_state(false)
	local more = ds.IconButton {
		icon = "more-horizontal",
		label = ui.t("settings.members.more_for", { name = member_name(first) }),
		size = "md",
		tone = "quiet",
		on_click = function()
			menu_open:set(not menu_open:peek())
		end,
	}
	return ds.Settings.Row {
		label = function()
			return member_name(m:get())
		end,
		-- (An address: blurred while addresses are hidden.)
		description = function()
			return m:get().email or ""
		end,
		private_description = true,
		-- What they do, on a line of its own.
		detail = function(): string?
			return m:get().title
		end,
		lead = ds.Avatar {
			seed = first.username or id,
			size = 32,
			label = member_name(first),
			-- Their photo, as it changes.
			src = function(): string
				return store.photo_file(m:get().avatar)
			end,
		},
		badge = ui.show(function()
			return m:get().you == true
		end, ds.Badge { ui.t("settings.members.you") }),
		control = ui.row {
			gap = "$space.1",
			items = "center",
			ui.show(can_change, ui.row {
				gap = "$space.1",
				items = "center",
				ds.Select {
					value = role,
					name = ui.t("settings.members.role_of", { name = member_name(first) }),
					size = "sm",
					w = 108,
					options = ROLES,
					on_change = function(v: string)
						store.set_role(id, v)
					end,
				},
				ui.menu(Menu.panel {
					open = menu_open,
					anchor = more,
					placement = { side = "bottom", align = "end", offset = 4 },
					w = 200,
					Menu.Group {
						Menu.Row {
							icon = "trash-bin-minimalistic",
							-- (A string, read as the menu is made: Menu.Row names
							-- itself only by one.)
							label = locale.s("settings.members.remove_from_org"),
							danger = true,
							on_click = function()
								menu_open:set(false)
								removing:set(id)
								removing_open:set(true)
							end,
						},
					},
				}),
			}),
			ui.show(function()
				return not can_change()
			end, ds.Badge {
				function()
					return role_label(m:get().role)
				end,
				size = "md",
			}),
		},
	}
end

-- An open invitation: who, when, and the ways to send it again or take it
-- back.
local function invitation_row(_i: store.Invitation, _n: number, inv: State<store.Invitation>): Node
	local id = inv:peek().id
	return ds.Settings.Row {
		label = function()
			return inv:get().email
		end,
		private_label = true,
		description = function()
			local i = inv:get()
			local day = if i.created_at then api.since(string.sub(i.created_at, 1, 10)) else nil
			if i.state == "expired" then
				return ui.t("settings.members.member"):get()
			end
			return if day then ui.t("settings.members.invited_on", { day = day }):get() else ui.t("settings.members.invited"):get()
		end,
		lead = ds.Avatar { seed = inv:peek().email, size = 32 },
		badge = ui.show(function()
			return inv:get().state == "expired"
		end, ds.Badge { ui.t("settings.members.expired"), tone = "warning" }),
		control = ui.row {
			gap = "$space.1",
			ds.Button {
				ui.t("settings.members.resend"),
				tone = "ghost",
				size = "sm",
				on_click = function()
					store.resend(id)
				end,
			},
			ds.Button {
				ui.t("settings.members.revoke"),
				tone = "ghost",
				size = "sm",
				on_click = function()
					store.revoke(id)
				end,
			},
		},
	}
end

local function list_kind(): string
	local s = store.state("members")
	if s == "failed" then
		return "failed"
	elseif s ~= "ready" and #store.members:get() == 0 then
		return "loading"
	end
	return if #found:get() > 0 then "found" else "nobody"
end

local function list_body(key: string): Node
	if key == "loading" then
		return ds.Settings.Group {
			ds.SkeletonRow { trail = 108 },
			ds.SkeletonRow { title = 110, line = 200, trail = 108 },
			ds.SkeletonRow { title = 130, line = 220, trail = 108 },
		}
	elseif key == "failed" then
		return ds.Callout {
			tone = "danger",
			title = ui.t("settings.members.couldnt_load") :: any,
			body = ui.t("settings.members.couldnt_load_body") :: any,
			action = {
				label = ui.t("common.retry") :: any,
				on_click = function()
					visits.again("members", store.load_org)
				end,
			},
		}
	elseif key == "nobody" then
		return ds.EmptyState {
			icon = "search",
			title = ui.t("settings.members.nobody") :: any,
			body = (if store.admin:peek() then ui.t("settings.members.nobody_admin") else ui.t("settings.members.nobody_member")) :: any,
			size = "sm",
			action = if store.admin:peek()
				then {
					label = ui.t("settings.members.invite_someone") :: any,
					on_click = function()
						inviting:set(true)
					end,
				}
				else nil,
		}
	end
	return ds.Settings.Each(found, member_row, { key = by_id })
end

local function valid(s: string): boolean
	return string.match(s, "^[^@%s]+@[^@%s]+%.[^@%s]+$") ~= nil
end

local function send_invites()
	local list = invitees:peek()
	if #list == 0 or sending:peek() then
		return
	end
	for _, e in list do
		if not valid(e) then
			invite_error:set(ui.t("settings.members.bad_address"):get())
			return
		end
	end
	sending:set(true)
	invite_error:set(nil)
	store.invite(list, function(invited: api.Invited?, word: string?)
		sending:set(false)
		if not invited then
			invite_error:set(store.says(word))
			return
		end
		-- Those that didn't go stay, saying why; the rest are done.
		local left: { string } = {}
		for _, f in invited.failed do
			table.insert(left, f.email)
		end
		invitees:set(left)
		local n = #invited.sent
		if #left == 0 then
			inviting:set(false)
			require("@ds/toast").show {
				title = ui.t("settings.members.sent", { n = n }):get(),
				body = ui.t("settings.members.sent_body"):get(),
				tone = "success",
			}
		else
			local exists = 0
			for _, f in invited.failed do
				exists += if f.error == "exists" then 1 else 0
			end
			invite_error:set(
				if exists == #left
					then (if exists == 1 then ui.t("settings.members.exists_one"):get() else ui.t("settings.members.exists_many"):get())
					else ui.t("settings.members.some_failed"):get()
			)
		end
	end)
end

local function dialogs(): Node
	return ui.column {
		ds.Dialog {
			open = removing_open,
			title = (function(): string
				for _, m in store.members:get() do
					if m.id == removing:get() then
						return ui.t("settings.members.remove_name", { name = member_name(m) }):get()
					end
				end
				return ui.t("settings.members.remove_them"):get()
			end) :: any,
			body = ui.t("settings.members.remove_body"),
			tone = "danger",
			confirm = {
				label = ui.t("settings.members.remove") :: any,
				on_click = function()
					local id = removing:peek()
					if id ~= "" then
						store.remove(id)
					end
				end,
			},
		},
		ds.Dialog {
			open = inviting,
			title = (function(): string
				local o = store.org:get()
				return if o then ui.t("settings.members.invite_to", { org = o.name }):get() else ui.t("settings.members.invite_teammates"):get()
			end) :: any,
			body = ui.t("settings.members.invite_body"),
			size = "md",
			confirm = {
				label = ui.t("settings.members.send_invites") :: any,
				keep_open = true,
				on_click = send_invites,
			},
			ds.TokenField {
				value = invitees,
				name = ui.t("settings.members.addresses"),
				placeholder = "name@company.com",
				valid = valid,
				error = function()
					return invite_error:get()
				end,
				hint = ui.t("settings.members.addresses_hint"),
				w_full = true,
				autofocus = true,
			},
		},
	}
end

-- Untouched, the name field follows the organization's.
ui.effect(function()
	local o = store.org:get()
	local n = if o then o.name else ""
	if not org_touched:peek() and org_name:peek() ~= n then
		org_name:set(n)
	end
end)
ui.effect(function()
	local typed = org_name:get()
	local o = store.org:peek()
	if o and typed ~= o.name and not org_touched:peek() then
		org_touched:set(true)
	end
end)

local function rename_org()
	local n = string.match(org_name:peek(), "^%s*(.-)%s*$") or ""
	if n == "" or #n > 80 then
		return
	end
	store.rename_org(n, function(ok: boolean)
		if ok then
			org_touched:set(false)
		end
	end)
end

local function page_kind(): string
	local s = store.state("org")
	if s == "failed" then
		return "failed"
	elseif store.org:get() == nil then
		return if s == "ready" then "none" else "loading"
	end
	return "org"
end

local function page_body(key: string): Node
	if key == "loading" then
		return ds.Settings.Group { ds.SkeletonRow { trail = 108 }, ds.SkeletonRow { title = 130, line = 220, trail = 108 } }
	elseif key == "failed" then
		return ds.Callout {
			tone = "danger",
			title = ui.t("settings.members.couldnt_load_org") :: any,
			body = ui.t("settings.members.couldnt_load_org_body") :: any,
			action = {
				label = ui.t("common.retry") :: any,
				on_click = function()
					visits.again("members", store.load_org)
				end,
			},
		}
	elseif key == "none" then
		return ds.EmptyState {
			icon = "users-group-rounded",
			title = ui.t("settings.members.no_org") :: any,
			body = ui.t("settings.members.no_org_body") :: any,
			size = "sm",
		}
	end
	return ui.column {
		w_full = true,
		gap = "$space.6",
		search.anchor(
			"people",
			ds.Settings.Section {
				ui.presence(list_kind, list_body, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
			}
		),
		-- (An admin's only: the open invitations.)
		ui.show(
			function()
				return store.admin:get() and #store.invitations:get() > 0
			end,
			search.anchor(
				"invitations",
				ds.Settings.Section {
					title = ui.t("settings.members.invitations.label") :: any,
					description = ui.t("settings.members.invitations.description") :: any,
					ds.Settings.Each(store.invitations, invitation_row, { key = by_id }),
				}
			)
		),
		search.anchor(
			"org_name",
			ds.Settings.Section {
				title = ui.t("settings.members.org_section") :: any,
				ds.Settings.Group {
					ds.Settings.Row {
						label = ui.t("settings.members.name") :: any,
						description = function()
							return if store.admin:get() then ui.t("settings.members.name_admin"):get() else ui.t("settings.members.name_member"):get()
						end,
						stacked = true,
						control = ui.row {
							w_full = true,
							gap = "$space.2",
							items = "center",
							ds.TextField {
								value = org_name,
								name = ui.t("settings.members.org_name.label"),
								grow = 1,
								disabled = function(): boolean
									return not store.admin:get()
								end,
								on_commit = function()
									rename_org()
								end,
							},
							ui.show(function()
								return store.admin:get()
							end, ds.Button {
								ui.t("common.save"),
								tone = "secondary",
								disabled = function()
									local o = store.org:get()
									local n = string.match(org_name:get(), "^%s*(.-)%s*$") or ""
									return o == nil or n == "" or n == o.name
								end,
								on_click = rename_org,
							}),
						},
					},
				},
			}
		),
	}
end

function M.view(): Node
	visits.once("members", store.load_org)
	return ui.column {
		w_full = true,
		items = "center",
		ds.Settings.Page {
			title = ui.t("settings.members.title"),
			description = function()
				local o = store.org:get()
				return if o then ui.t("settings.members.everyone_in", { org = o.name }):get() else ui.t("settings.members.everyone"):get()
			end :: any,
			-- The search beside the one action, over the list it filters.
			trail = ui.row {
				gap = "$space.2",
				items = "center",
				ds.SearchField { value = query, placeholder = ui.t("settings.members.find") :: any, size = "sm", w = 180, name = ui.t("settings.members.find") :: any },
				ui.show(function()
					return store.admin:get()
				end, ds.Button {
					ui.t("settings.members.invite.label"),
					size = "sm",
					on_click = function()
						invite_error:set(nil)
						inviting:set(true)
					end,
				}),
			},
			ui.presence(page_kind, page_body, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
		},
		dialogs(),
	}
end

return M
