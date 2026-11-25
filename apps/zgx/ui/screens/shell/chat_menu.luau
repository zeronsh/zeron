-- A chat's menu, wherever a chat is right-clicked: the sidebar's session
-- rows (Zeron's sidebar chat menu, as the gallery's), the panel's side chat
-- rows and tabs, the Control room's map. Rename, pin, copy (its folder,
-- links, ids), move into one of your sections, sleep or wake, make it an
-- orchestrator and put it on the board (when it has no card there not
-- done) while Orchestrators is on, archive (kept: Settings → Archived lists it), delete (for
-- good, asked once more); then plugins' items, under their names
-- (screens/plugins/menus: the app's registry of them, read as the menu
-- opens). A side chat's without Pin and Move to.
--
--   ChatMenu.entries(id)                     the whole menu (nil: no such chat)
--   ChatMenu.entries(id, { delete = false }) without Delete
--   ChatMenu.entries_of(ids)                 of several picked out together
--                                            (the sidebar's ⇧-click), without
--                                            what is one chat's own: rename,
--                                            copy, Orchestrators' items,
--                                            plugins' items
--
-- What it does lives with the sidebar, which hands it over as it loads
-- (`ChatMenu.install`): its rename and delete dialogs, its sections, its
-- custom sleep, which chats are side chats and which orchestrators. Until
-- then (a fixture without the sidebar) there is no menu.

local ds = require("@ds")
local locale = require("@ds/locale")
local sessions = require("@account/sessions")
local activity = require("@app/screens/shell/activity")
local boundary = require("@app/screens/shell/boundary")
local plugins = require("@account/plugins")

local M = {}

-- What the sidebar hands over.
export type Host = {
	chat: (id: string) -> any,
	places: () -> any,
	-- Puts `ids` where they go: "pinned", a section's id, or nil (back to
	-- their projects).
	put: (ids: { string }, where: string?) -> (),
	plain: (places: any) -> any,
	set_places: (places: any) -> (),
	-- Whether every one of `ids` is in `list`.
	all_in: (ids: { string }, list: { string }) -> boolean,
	is_side: (id: string) -> boolean,
	is_orchestrator: (id: string) -> boolean,
	rename: (id: string) -> (),
	archive: (id: string) -> (),
	-- Asks once more before deleting `ids` (its dialog), told what goes
	-- with them.
	ask_delete: (ids: { string }, subagents: number, sides: number) -> (),
	-- Opens the custom sleep time, for `ids`, at `at`.
	sleep_custom: (ids: { string }, at: number) -> (),
}
local host: Host? = nil

function M.install(h: Host)
	host = h
end

-- (Copied, said so, as Zeron's: "Path copied".)
local function copy(text: string?, what: string)
	if text then
		ui.clipboard.write(text)
		ds.toast.show { title = locale.s("shell.sidebar.copied", { what = what }), tone = "success" }
	end
end
-- A string as a URL's part (as production's `encode_component`).
local function encoded(v: string): string
	return (string.gsub(v, "[^%w%-_%.~]", function(c: string): string
		return string.format("%%%02X", string.byte(c))
	end))
end

export type Options = { delete: boolean? }

-- Move to's sections, for `ids`.
local function sections_menu(h: Host, ids: { string }): { any }
	local into: { any } = { { heading = locale.s("shell.sidebar.menu.sections") } }
	for _, s in h.places().sections do
		local here = h.all_in(ids, s.sessions)
		table.insert(into, {
			label = s.name,
			icon = "folder",
			checked = function(): boolean
				return here
			end,
			-- (Chosen where they are: back to their project.)
			on_select = function()
				h.put(ids, if here then nil else s.id)
			end,
		})
	end
	table.insert(into, { separator = true })
	table.insert(into, {
		label = locale.s("shell.sidebar.menu.new_section"),
		icon = "plus",
		on_select = function()
			local sid = `section-{os.time()}-{math.random(1, 1e6)}`
			local now = h.plain(h.places())
			table.insert(now.sections, { id = sid, name = locale.s("shell.sidebar.menu.new_section"), sessions = {} })
			h.set_places(now)
			h.put(ids, sid)
		end,
	})
	return into
end
-- What Copy offers a chat: its folder, its harness's link and id where it
-- has them (a Codex thread's), and its own id.
local function copies_menu(c: any): { any }
	local copies: { any } = {
		{
			label = locale.s("shell.sidebar.menu.path"),
			icon = "folder",
			disabled = if c.cwd then nil else locale.s("shell.sidebar.menu.no_folder"),
			on_select = function()
				copy(c.cwd, locale.s("shell.sidebar.menu.path"))
			end,
		},
	}
	table.insert(copies, {
		label = locale.s("shell.sidebar.menu.zeron_link"),
		icon = "arrow-up-right",
		on_select = function()
			sessions.link(c.id):after(function(link: any)
				copy(if type(link) == "string" then link else nil, locale.s("shell.sidebar.menu.zeron_link"))
			end, function()
				ds.toast.show { title = locale.s("shell.sidebar.menu.link_not_ready"), tone = "danger" }
			end)
		end,
	})
	if c.harness == "codex" and c.harness_session then
		table.insert(copies, {
			label = locale.s("shell.sidebar.menu.codex_link"),
			icon = "arrow-up-right",
			on_select = function()
				copy(`codex://threads/{encoded(c.harness_session :: string)}`, locale.s("shell.sidebar.menu.codex_link"))
			end,
		})
	end
	if c.harness_session then
		table.insert(copies, {
			label = locale.s("shell.sidebar.menu.harness_session_id"),
			icon = "document",
			on_select = function()
				copy(c.harness_session, locale.s("shell.sidebar.menu.harness_session_id"))
			end,
		})
	end
	table.insert(copies, {
		label = locale.s("shell.sidebar.menu.session_id"),
		icon = "document",
		on_select = function()
			copy(c.id, locale.s("shell.sidebar.menu.session_id"))
		end,
	})
	return copies
end
-- The Activity view's Sleep (its times, each saying when, and one of your
-- own), or, all of them asleep, Wake.
local function sleep_entry(h: Host, chats: { any }): any
	local ids: { string } = {}
	local awake = false
	for _, c in chats do
		table.insert(ids, c.id)
		awake = awake or not c.sleep_until
	end
	if not awake then
		return {
			label = locale.s("shell.activity.wake"),
			icon = "sun",
			on_select = function()
				for _, id in ids do
					activity.wake(id)
				end
			end,
		}
	end
	local times: { any } = {}
	for _, p in activity.presets() do
		table.insert(times, {
			label = locale.s("shell.activity.sleep.choice", { name = p.label, when = p.detail }),
			on_select = function()
				for _, id in ids do
					activity.sleep(id, p.until_at)
				end
			end,
		})
	end
	table.insert(times, { separator = true })
	table.insert(times, {
		label = locale.s("shell.activity.sleep.custom"),
		on_select = function()
			-- (It opens on tomorrow at 9: a time already chosen,
			-- moved from there.)
			local d = os.date("*t") :: any
			h.sleep_custom(ids, activity.local_time({ year = d.year, month = d.month, day = d.day + 1, hour = 9, min = 0, sec = 0 }))
		end,
	})
	return { label = locale.s("shell.activity.sleep.title"), icon = "moon", submenu = times }
end
-- Waiting on background work it said it waits on: the person can say
-- otherwise (a dev server marked by mistake). Its background agents aren't
-- its say, so there's nothing to take back there. (Of several: those that
-- are; false, none.)
local function stop_waiting_entry(chats: { any }): any
	local waiting: { string } = {}
	for _, c in chats do
		if c.status == "waiting" then
			for _, w in c.waiting or {} do
				if w.kind == "process" then
					table.insert(waiting, c.id)
					break
				end
			end
		end
	end
	if #waiting == 0 then
		return false
	end
	return {
		label = locale.s("shell.activity.stop_waiting"),
		icon = "close-circle",
		on_select = function()
			for _, id in waiting do
				sessions.stop_waiting(id):after(function() end, function()
					ds.toast.show { title = locale.s("shell.activity.stop_waiting_failed"), tone = "danger" }
				end)
			end
		end,
	}
end
-- Deleting asks once more (Zeron's "Delete session?"), once it knows what
-- goes with them, to say so.
local function ask_delete(h: Host, ids: { string })
	local subagents, sides, left = 0, 0, #ids
	local function heard(with: any)
		subagents += tonumber(with and with.subagents) or 0
		sides += tonumber(with and with.sides) or 0
		left -= 1
		if left == 0 then
			h.ask_delete(ids, subagents, sides)
		end
	end
	for _, id in ids do
		sessions.deleted_with(id):after(heard, function()
			heard(nil)
		end)
	end
end

function M.entries_of(wanted: { string }, opts: Options?): { any }?
	local h = host
	if not h then
		return nil
	end
	local ids: { string } = {}
	local chats: { any } = {}
	local busy = 0
	for _, id in wanted do
		local c = h.chat(id)
		if c then
			table.insert(ids, id)
			table.insert(chats, c)
			busy += if c.status == "working" or c.status == "waiting" then 1 else 0
		end
	end
	if #ids == 0 then
		return nil
	end
	local one = if #ids == 1 then chats[1] else nil
	-- (A side chat's: without what doesn't apply to it.)
	local side = one ~= nil and h.is_side(one.id)
	local pinned = h.all_in(ids, h.places().pinned)
	-- (Orchestrators' parts while it's on, read as the menu opens; one
	-- chat's alone.)
	local orch = if one then plugins.module("orchestrators") else nil
	local all: { any } = {
		one ~= nil and {
			label = locale.s("common.rename"),
			icon = "pen",
			on_select = function()
				h.rename(ids[1])
			end,
		},
		not side and {
			label = locale.s(if pinned then "shell.sidebar.menu.unpin" else "shell.sidebar.menu.pin"),
			icon = "pin",
			shortcut = if one then "cmd+shift+p" else nil,
			on_select = function()
				h.put(ids, if pinned then nil else "pinned")
			end,
		},
		one ~= nil and {
			label = locale.s("common.copy"),
			icon = "copy",
			submenu = copies_menu(one),
		},
		-- (Activity lists no sections: nothing to move into.)
		not side and not activity.on:peek() and {
			label = locale.s("shell.sidebar.menu.move_to"),
			icon = "folder-with-files",
			submenu = sections_menu(h, ids),
		},
		not side and activity.on:peek() and sleep_entry(h, chats),
		stop_waiting_entry(chats),
		-- (A chat of its own, not yet one: it becomes an orchestrator, its
		-- transcript kept. Orchestrators off: neither of these.)
		orch ~= nil and not side and not h.is_orchestrator(ids[1]) and {
			label = locale.s("orch.sidebar.make_orchestrator"),
			icon = "users-group-rounded",
			on_select = function()
				orch.actions.promote(ids[1])
			end,
		},
		-- (Not an orchestrator's own chat, nor one with a card not done.)
		orch ~= nil and not h.is_orchestrator(ids[1]) and not orch.orchestration.on_board(ids[1]) and {
			label = locale.s("orch.sidebar.add_to_board"),
			icon = "checklist",
			on_select = function()
				orch.orchestration.add_to_board(ids[1]):after(function()
					ds.toast.show { title = locale.s("orch.sidebar.added_to_board"), tone = "success" }
				end, function()
					ds.toast.show { title = locale.s("orch.sidebar.add_to_board_failed"), tone = "danger" }
				end)
			end,
		},
		{ separator = true },
		{
			-- (No key: ⌘⌫ is a text field's, deleting to the line's start.)
			label = locale.s("common.archive"),
			icon = "archive-minimalistic",
			disabled = if busy == 0
				then nil
				elseif one then locale.s("shell.sidebar.menu.archive_working")
				else locale.s("shell.sidebar.menu.archive_working_some", { n = busy }),
			on_select = function()
				for _, id in ids do
					h.archive(id)
				end
			end,
		},
		not (opts and opts.delete == false) and {
			label = locale.s("shell.sidebar.menu.delete"),
			icon = "trash-bin-minimalistic",
			danger = true,
			on_select = function()
				ask_delete(h, ids)
			end,
		},
	}
	local entries: { any } = {}
	for _, e in all do
		if e then
			table.insert(entries, e)
		end
	end
	-- (The registry is the plugin system's: without it, the app's alone. An
	-- item is told one chat.)
	local menus: { extend: ((entries: { any }, kind: string, context: { [string]: any }) -> { any })? }? =
		boundary.load("@app/screens/plugins/menus")
	local extend = menus and menus.extend
	if extend and one then
		return extend(entries, "session", { id = one.id })
	end
	return entries
end

function M.entries(id: string, opts: Options?): { any }?
	return M.entries_of({ id }, opts)
end

return M
