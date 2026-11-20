-- Settings → Archived: the sessions put away, hidden from the sidebar until
-- restored (Zeron's: crates/ui/src/settings/archived.rs). Searched from one
-- field and laid out by when each last moved (today, yesterday, this week,
-- earlier), each group a card of settings rows (as Plugins' are, washed
-- edge to edge under the pointer): its project's tile, its title, where it
-- ran, its harness and how long ago. Under the pointer a row's corner
-- restores it or deletes it; its menu has both and Open. Delete all, at the title's end, asks once more, naming how many
-- (and the Cloud machines that go with them), and deletes the ones archived
-- as it was asked: one restored meanwhile stays.

local ds = require("@ds")
local SessionRow = require("@ds/session_row")
local Tile = require("@ds/tile")
local Icon = require("@ds/icon")
local ContextMenu = require("@ds/context_menu")
local locale = require("@ds/locale")
local sessions = require("@account/sessions")
local route = require("@app/screens/shell/route")

local M = {}

-- How many show before "Show more", and how many more it shows.
M.SHOWN = 50
local query = ui.state("settings_archived_query", "")
local shown = ui.state("settings_archived_shown", M.SHOWN)
-- The session "Delete session?" asks about.
local deleting = ui.state("settings_archived_deleting", nil :: string?)
local delete_open = ui.state("settings_archived_delete_open", false)
-- Delete all: the sessions it asks about (as they were when it was asked),
-- and how many it's still deleting.
local delete_all_open = ui.state("settings_archived_delete_all_open", false)
local delete_all_ids = ui.state("settings_archived_delete_all_ids", {} :: { string })
local busy = ui.state("settings_archived_busy", 0)


local function all(): { sessions.Chat }
	return sessions.now:get().archived or {}
end
local function device_of(id: string?): sessions.Device?
	for _, d in sessions.devices:get() or {} do
		if d.id == id then
			return d
		end
	end
	return nil
end
local function project_of(id: string?): sessions.Project?
	for _, p in sessions.projects:get() or {} do
		if p.id == id then
			return p
		end
	end
	return nil
end
-- A Cloud session's: its machine is its own (deleted with it), never named.
function M.on_cloud(c: sessions.Chat): boolean
	local d = device_of(c.device)
	return string.sub(c.device or "", 1, 6) == "cloud-" or (d ~= nil and (d.cloud == true or d.platform == "cloud"))
end
-- Where it ran: its project, and the computer (not a Cloud machine, nor
-- one no longer known).
function M.where(c: sessions.Chat): string
	local parts: { string } = {}
	local p = project_of(c.project)
	if p then
		table.insert(parts, p.name)
	end
	local d = device_of(c.device)
	if d and not M.on_cloud(c) then
		table.insert(parts, d.name)
	end
	return table.concat(parts, " · ")
end

local function day_of(at: number): number
	local d = os.date("*t", math.floor(at)) :: any
	d.hour, d.min, d.sec = 0, 0, 0
	return os.time(d)
end
-- Its group by when it last moved: today, yesterday, this week (the six
-- days before), earlier.
M.GROUPS = { "today", "yesterday", "week", "earlier" }
function M.group_of(at: number, now: number): string
	local today = day_of(now)
	local start = day_of(at)
	if start >= today then
		return "today"
	elseif start >= day_of(today - 43200) then
		return "yesterday"
	elseif start >= day_of(today - 6 * 86400 + 43200) then
		return "week"
	end
	return "earlier"
end

local matching = ui.computed(function(): { sessions.Chat }
	local q = string.lower(string.match(query:get(), "^%s*(.-)%s*$") or "")
	if q == "" then
		return all()
	end
	local out = {}
	for _, c in all() do
		if string.find(string.lower(`{c.title} {M.where(c)}`), q, 1, true) then
			table.insert(out, c)
		end
	end
	return out
end)
type Group = { key: string, chats: { sessions.Chat } }
local groups = ui.computed(function(): { Group }
	local now = os.time()
	local by: { [string]: Group } = {}
	for i, c in matching:get() do
		if i > shown:get() then
			break
		end
		local key = M.group_of(c.updated, now)
		local g = by[key]
		if not g then
			g = { key = key, chats = {} }
			by[key] = g
		end
		table.insert(g.chats, c)
	end
	local out: { Group } = {}
	for _, key in M.GROUPS do
		if by[key] then
			table.insert(out, by[key])
		end
	end
	return out
end)

local function title_of(c: sessions.Chat): string
	return if c.title ~= "" then (string.gsub(c.title, "%s+", " ")) else locale.s("settings.archived.untitled")
end
local function ask_delete(id: string)
	deleting:set(id)
	delete_open:set(true)
end

-- A session's row, as a settings list's rows are (Plugins', Templates'):
-- its project's tile, its title and where it ran, washed edge to edge under
-- the pointer (Settings.CatalogRow). At its end its harness's mark and how
-- long ago; under the pointer, Restore and Delete in their place.
local Row = ui.component(function(props: { now: State<any> }): Node
	local now = props.now
	local over = ui.use_state(false)
	local function c(): sessions.Chat
		return now:get() :: sessions.Chat
	end
	local function cur(): SessionRow.Session
		local x = c()
		return { id = x.id, title = title_of(x), time = x.updated, agent = x.agent :: any, harness = x.harness, device = x.device }
	end
	-- (Its project's tile; one no longer known, a quiet tile of its mark.)
	local lead = ui.presence(function(): string
		return c().project or ""
	end, function(pid: string): Node
		local p = project_of(pid)
		if not p then
			return ui.row {
				w = 28,
				h = 28,
				shrink = 0,
				items = "center",
				justify = "center",
				rounded = "$radius.lg",
				bg = "$zeron.secondary.bg",
				SessionRow.mark(cur, function(): boolean
					return true
				end),
			}
		end
		local l = route.look(p.id) or { hue = route.hue(p.id) }
		return Tile { label = p.name, hue = l.hue, src = l.src, emoji = l.emoji, size = 28, project = p.id }
	end)
	local function lit(): boolean
		return over:get()
	end
	local corner = ui.row {
		relative = true,
		h = 24,
		items = "center",
		justify = "end",
		ui.row {
			gap = "$space.2",
			items = "center",
			opacity = function(): number
				return if lit() then 0 else 1
			end,
			pointer_events = "none",
			SessionRow.mark(cur, function(): boolean
				return false
			end),
			SessionRow.standing(cur),
		},
		ui.show(
			lit,
			ui.row {
				class = "archived-actions",
				absolute = true,
				right = 0,
				top = 0,
				gap = "$space.1",
				items = "center",
				SessionRow.corner_button("settings.archived.restore", "archive-up-minimalistic", function()
					sessions.unarchive(now:peek().id)
				end),
				SessionRow.corner_button("common.delete", "trash-bin-minimalistic", function()
					ask_delete(now:peek().id)
				end),
			}
		),
	}
	return ContextMenu {
		label = locale.t("ds.sidebar.session_actions"),
		w_full = true,
		items = function(): { any }
			local id = now:peek().id
			return {
				{ label = locale.s("common.open"), icon = "arrow-up-right", on_select = function()
					route.open_session(id)
				end },
				{ label = locale.s("settings.archived.restore"), icon = "archive-up-minimalistic", on_select = function()
					sessions.unarchive(id)
				end },
				{ separator = true },
				{ label = locale.s("settings.archived.delete_one"), icon = "trash-bin-minimalistic", danger = true, on_select = function()
					ask_delete(id)
				end },
			}
		end,
		ds.Settings.CatalogRow {
			over = over,
			row = ui.div {
				class = "archived-row",
				role = "group",
				label = function(): string
					return title_of(c())
				end,
				w_full = true,
				ds.Settings.Row {
					lead = lead,
					label = function(): string
						return title_of(c())
					end,
					description = function(): string
						return M.where(c())
					end,
					truncate = true,
					control = corner,
				},
			},
		},
	}
end)

-- A group: its name over a card of its rows, a hairline between them (each
-- its session's, kept as others come and go; each as tall as `Settings.Row`
-- with a 28 tile makes it).
local function group(g: Group, _i: number, now: State<any>): Node
	local chats = ui.computed(function(): { sessions.Chat }
		return (now:get() :: Group).chats
	end)
	-- A row's height: all alike (read once, not for each of thousands; a 28
	-- tile leads it).
	local row_h = ui.computed(function(): number
		return ds.Settings.truncated_row_h(28)
	end)
	-- (A group's key is its name: it stays the same group.)
	return ds.Settings.Section {
		title = ui.t(`settings.archived.group.{g.key}`),
		-- (Thousands can be archived: only the rows the page shows are built.)
		ds.Settings.EachList(chats, function(_c: sessions.Chat, _j: State<number>, n: State<any>): Node
			return Row { now = n }
		end, {
			key = function(x: sessions.Chat): string
				return x.id
			end,
			height = function(_x: sessions.Chat): number
				return row_h:get()
			end,
			text = function(x: sessions.Chat): string
				return `{title_of(x)}\t{M.where(x)}`
			end,
		}),
	}
end

-- Delete all's line: how many, and the Cloud machines that go with them.
function M.delete_all_body(count: number, on_cloud: number): string
	local what = locale.s("settings.archived.sessions", { n = count })
	if on_cloud == 1 and count == 1 then
		what ..= locale.s("settings.archived.its_machine")
	elseif on_cloud > 0 then
		what ..= locale.s("settings.archived.machines", { n = on_cloud })
	end
	return locale.s("settings.archived.delete_all_body", { what = what })
end

local function delete_all()
	-- (The ones still archived: one restored since it was asked stays.)
	local still: { [string]: boolean } = {}
	for _, c in all() do
		still[c.id] = true
	end
	local ids: { string } = {}
	for _, id in delete_all_ids:peek() do
		if still[id] then
			table.insert(ids, id)
		end
	end
	if #ids == 0 then
		return
	end
	local left, failed = #ids, 0
	busy:set(left)
	local function settled(ok: boolean)
		left -= 1
		if not ok then
			failed += 1
		end
		busy:set(left)
		if left == 0 and failed > 0 then
			ds.toast.show { title = locale.s("settings.archived.couldnt_delete", { n = failed }), tone = "danger" }
		end
	end
	for _, id in ids do
		local call = sessions.delete(id)
		if call and call.after then
			call:after(function()
				settled(true)
			end, function()
				settled(false)
			end)
		else
			settled(true)
		end
	end
end

local function delete_all_button(): Node
	return ui.show(
		function(): boolean
			return #all() > 0
		end,
		-- (Its ink, not its ghost's box, on the text edge: at the title's end
		-- the cards' end edge, gone under it when narrow the words' edge. Out
		-- by the ghost's own padding either side.)
		ui.div {
			mx = ui.raw(-((ui.theme.get("zeron.space.snug") :: number?) or 10), "the ghost's own padding"),
			ds.Button {
				id = "archived-delete-all",
				tone = "ghost",
				size = "sm",
				-- (Deleting: its spinner, as every working button's.)
				busy = function(): boolean
					return busy:get() > 0
				end,
				on_click = function()
					local ids = {}
					for _, c in all() do
						table.insert(ids, c.id)
					end
					delete_all_ids:set(ids)
					delete_all_open:set(true)
				end,
				ui.row {
					gap = "$space.1",
					items = "center",
					color = "$danger",
					Icon { name = "trash-bin-minimalistic", size = 14 },
					ui.text { ui.t("settings.archived.delete_all") },
				},
			},
		}
	)
end

local function body(says: () -> string): Node
	return ui.text { says, w_full = true, wrap = true, text_size = "$font.size.md", line_height = 20, color = "$text.secondary" }
end

function M.view(): Node
	return ds.Settings.Page {
		title = ui.t("settings.archived.title"),
		description = function(): string
			local n = #all()
			return if n == 0 then locale.s("settings.archived.description_none") else locale.s("settings.archived.description", { n = n })
		end,
		trail = delete_all_button(),
		-- (At the top right, on the title's line, as Plugins' is.)
		trail_top = true,
		ui.show(
			function(): boolean
				return #all() == 0
			end,
			ui.column {
				id = "archived-empty",
				w_full = true,
				py = "$space.8",
				ds.EmptyState {
					icon = "archive-minimalistic",
					size = "sm",
					title = ui.t("settings.archived.empty_title"),
					body = ui.t("settings.archived.empty_body"),
				},
			}
		),
		ui.show(
			function(): boolean
				return #all() > 0
			end,
			ds.SearchField {
				id = "archived-search",
				value = query,
				name = locale.s("settings.archived.search"),
				placeholder = locale.s("settings.archived.search"),
				w_full = true,
			}
		),
		ui.show(
			function(): boolean
				return #all() > 0 and #matching:get() == 0
			end,
			ui.row {
				id = "archived-none-match",
				w_full = true,
				px = ds.Settings.inset(),
				ds.Hint {
					function(): string
						return locale.s("settings.archived.none_match", { q = string.match(query:get(), "^%s*(.-)%s*$") or "" })
					end,
				},
			}
		),
		ui.each(groups, group, {
			key = function(g: Group): string
				return g.key
			end,
			direction = "column",
			gap = "$space.6",
			w_full = true,
		}),
		ui.show(
			function(): boolean
				return #matching:get() > shown:get()
			end,
			ui.row {
				w_full = true,
				justify = "center",
				ds.Button {
					id = "archived-more",
					tone = "ghost",
					size = "sm",
					on_click = function()
						shown:set(shown:peek() + M.SHOWN)
					end,
					ui.t("settings.archived.show_more"),
				},
			}
		),
		ds.Dialog {
			open = delete_open,
			title = ui.t("shell.sidebar.session.delete_title") :: any,
			tone = "danger",
			size = "sm",
			confirm = {
				label = ui.t("common.delete") :: any,
				on_click = function()
					local id = deleting:peek()
					deleting:set(nil)
					if id then
						sessions.delete(id)
					end
				end,
			},
			body(function(): string
				local id = deleting:get()
				for _, c in all() do
					if c.id == id then
						return locale.s("shell.sidebar.session.will_delete", { title = title_of(c) })
					end
				end
				return ""
			end),
		},
		ds.Dialog {
			open = delete_all_open,
			title = ui.t("settings.archived.delete_all_title") :: any,
			tone = "danger",
			size = "sm",
			confirm = {
				label = ui.t("settings.archived.delete_all") :: any,
				on_click = delete_all,
			},
			body(function(): string
				local ids = delete_all_ids:get()
				local mine: { [string]: boolean } = {}
				for _, id in ids do
					mine[id] = true
				end
				local on_cloud = 0
				for _, c in all() do
					if mine[c.id] and M.on_cloud(c) then
						on_cloud += 1
					end
				end
				return M.delete_all_body(#ids, on_cloud)
			end),
		},
	}
end

return M
