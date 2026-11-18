-- A chat's menu, wherever a chat is right-clicked: the sidebar's session
-- rows (Zeron's sidebar chat menu, as the gallery's), the panel's side chat
-- rows and tabs, the Control room's map. Rename, pin, copy (its folder,
-- links, ids), move into one of your sections, sleep or wake, make it an
-- orchestrator, put it on the board (when it has no card there not done),
-- archive (kept: Settings → Archived lists it), delete (for
-- good, asked once more); then plugins' items, under their names
-- (screens/plugins/menus: the app's registry of them, read as the menu
-- opens). A side chat's without Pin and Move to.
--
--   ChatMenu.entries(id)                     the whole menu (nil: no such chat)
--   ChatMenu.entries(id, { delete = false }) without Delete
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
local orch_actions = require("@app/screens/orchestrator/actions")
local orchestration = require("@account/orchestration")

local M = {}

-- What the sidebar hands over.
export type Host = {
	chat: (id: string) -> any,
	places: () -> any,
	put: (id: string, where: string?) -> (),
	plain: (places: any) -> any,
	set_places: (places: any) -> (),
	is_side: (id: string) -> boolean,
	is_orchestrator: (id: string) -> boolean,
	rename: (id: string) -> (),
	pin: (id: string) -> (),
	archive: (id: string) -> (),
	-- Asks once more before deleting (its dialog), told what goes with it.
	ask_delete: (id: string, subagents: number, sides: number) -> (),
	-- Opens the custom sleep time, for `id`, at `at`.
	sleep_custom: (id: string, at: number) -> (),
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

function M.entries(id: string, opts: Options?): { any }?
	local h = host
	if not h then
		return nil
	end
	local c = h.chat(id)
	if not c then
		return nil
	end
	local p = h.places()
	local pinned = table.find(p.pinned, id) ~= nil
	local into: { any } = { { heading = locale.s("shell.sidebar.menu.sections") } }
	for _, s in p.sections do
		local here = table.find(s.sessions, id) ~= nil
		table.insert(into, {
			label = s.name,
			icon = "folder",
			checked = function(): boolean
				return here
			end,
			-- (Chosen where it is: back to its project.)
			on_select = function()
				h.put(id, if here then nil else s.id)
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
			h.put(id, sid)
		end,
	})
	-- What Copy offers: its folder, its harness's link and id where it has
	-- them (a Codex thread's), and its own id.
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
			sessions.link(id):after(function(link: any)
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
	local side = h.is_side(id)
	-- The Activity view's Sleep (its times, each saying when, and one of
	-- your own), or, asleep, Wake.
	local sleep: any = false
	if activity.on:peek() and not side then
		if c.sleep_until then
			sleep = {
				label = locale.s("shell.activity.wake"),
				icon = "sun",
				on_select = function()
					activity.wake(id)
				end,
			}
		else
			local times: { any } = {}
			for _, preset in activity.presets() do
				table.insert(times, {
					label = locale.s("shell.activity.sleep.choice", { name = preset.label, when = preset.detail }),
					on_select = function()
						activity.sleep(id, preset.until_at)
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
					h.sleep_custom(id, activity.local_time({ year = d.year, month = d.month, day = d.day + 1, hour = 9, min = 0, sec = 0 }))
				end,
			})
			sleep = { label = locale.s("shell.activity.sleep.title"), icon = "moon", submenu = times }
		end
	end
	-- Waiting on background work it said it waits on: the person can say
	-- otherwise (a dev server marked by mistake). Its background agents
	-- aren't its say, so there's nothing to take back there.
	local stop_waiting: any = false
	if c.status == "waiting" then
		for _, w in c.waiting or {} do
			if w.kind == "process" then
				stop_waiting = {
					label = locale.s("shell.activity.stop_waiting"),
					icon = "close-circle",
					on_select = function()
						sessions.stop_waiting(id):after(function() end, function()
							ds.toast.show { title = locale.s("shell.activity.stop_waiting_failed"), tone = "danger" }
						end)
					end,
				}
				break
			end
		end
	end
	local entries = {
		{
			label = locale.s("common.rename"),
			icon = "pen",
			on_select = function()
				h.rename(id)
			end,
		},
		if side
			then false
			else {
				label = locale.s(if pinned then "shell.sidebar.menu.unpin" else "shell.sidebar.menu.pin"),
				icon = "pin",
				shortcut = "cmd+shift+p",
				on_select = function()
					h.pin(id)
				end,
			},
		{
			label = locale.s("common.copy"),
			icon = "copy",
			submenu = copies,
		},
		-- (Activity lists no sections: nothing to move into.)
		if side or activity.on:peek() then false else { label = locale.s("shell.sidebar.menu.move_to"), icon = "folder-with-files", submenu = into },
		sleep,
		stop_waiting,
		-- (A chat of its own, not yet one: it becomes an orchestrator, its
		-- transcript kept.)
		if side or h.is_orchestrator(id)
			then false
			else {
				label = locale.s("orch.sidebar.make_orchestrator"),
				icon = "users-group-rounded",
				on_select = function()
					orch_actions.promote(id)
				end,
			},
		-- (Not an orchestrator's own chat, nor one with a card not done.)
		if h.is_orchestrator(id) or orchestration.on_board(id)
			then false
			else {
				label = locale.s("orch.sidebar.add_to_board"),
				icon = "checklist",
				on_select = function()
					orchestration.add_to_board(id):after(function()
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
			disabled = if c.status == "working" or c.status == "waiting" then locale.s("shell.sidebar.menu.archive_working") else nil,
			on_select = function()
				h.archive(id)
			end,
		},
		if opts and opts.delete == false then false else {
			label = locale.s("shell.sidebar.menu.delete"),
			icon = "trash-bin-minimalistic",
			danger = true,
			-- (Asked once more, as Zeron's: "Delete session?")
			-- (Once it knows what goes with it, to say so.)
			on_select = function()
				local function ask(subagents: number, sides: number)
					h.ask_delete(id, subagents, sides)
				end
				sessions.deleted_with(id):after(function(with: any)
					ask(tonumber(with and with.subagents) or 0, tonumber(with and with.sides) or 0)
				end, function()
					ask(0, 0)
				end)
			end,
		},
	}
	-- (A side chat's: without what doesn't apply to it.)
	local kept: { any } = {}
	for _, e in entries :: { any } do
		if e ~= false then
			table.insert(kept, e)
		end
	end
	entries = kept :: any
	-- (The registry is the plugin system's: without it, the app's alone.)
	local menus: { extend: ((entries: { any }, kind: string, context: { [string]: any }) -> { any })? }? =
		boundary.load("@app/screens/plugins/menus")
	local extend = menus and menus.extend
	if extend then
		return extend(entries, "session", { id = id })
	end
	return entries
end

return M
