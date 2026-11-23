-- What a secondary click on something on the Map offers (each of its
-- views): the chat's own menu, as the sidebar has it (screens/shell/
-- chat_menu, never its Delete here), after the Map's few (Open, Stop while
-- it works, What's up?, Pin to Progress); a crew thread's Release from crew
-- in place of Make orchestrator; an orchestrator's own Pause or Resume,
-- Settings and Archive in place of the chat's Archive; a machine's Rename.
-- (Where the sidebar hasn't loaded, a fixture's, the same items made here.)
--
--   map_menu.entries(id)   the entries for `id` (a chat's or an
--                          orchestrator's id, a machine's as "dev:<id>")
--   map_menu.Dialogs()     what they open (a rename), mounted once

local ds = require("@ds")
local locale = require("@ds/locale")
local data = require("@ds/orch_data")
local sessions = require("@account/sessions")
local orchestration = require("@account/orchestration")
local route = require("@app/screens/shell/route")
local orch_actions = require("@app/screens/orchestrator/actions")
local progress = require("@app/screens/control/progress")
local ChatMenu = require("@app/screens/shell/chat_menu")
local Ref = require("@ds/orch_ref")
local Shared = require("@app/screens/chat/shared")

local M = {}

-- What's being renamed: { kind = "chat" | "device", id, text }.
M.renaming = ui.state("map_menu_renaming", nil :: any)
local rename_open = ui.state("map_menu_rename_open", false)

local function said(key: string): (any) -> ()
	return function(_e: any)
		ds.toast.show { title = locale.s(key), tone = "danger" }
	end
end

local function copy(text: string?, what: string)
	if text then
		ui.clipboard.write(text)
		ds.toast.show { title = locale.s("shell.sidebar.copied", { what = what }), tone = "success" }
	end
end

local function chats(): { [string]: any }
	return data.chats(sessions.now:peek())
end

-- A reference to `id` (a chat, an orchestrator) put in Capture's composer
-- (the Brain dump's, docked or floating), as dragging it there would.
function M.mention(id: string)
	local now = orchestration.now:peek() or {}
	local brain: string? = nil
	local ref: any = nil
	for _, o in now.orchestrators or {} do
		if o.kind == "brain_dump" and not o.archived then
			brain = o.id
		end
		if o.id == id then
			ref = Ref.orchestrator(o, id)
		end
	end
	if not ref then
		ref = Ref.thread(now, chats()[id], id, nil)
	end
	if ref and brain then
		Shared.mention_ref(ref, brain)
	end
end

local function rename(kind: string, id: string, text: string)
	M.renaming:set({ kind = kind, id = id, text = text })
	rename_open:set(true)
end

-- Into a section of the sidebar's (or out of them all).
local function move_to(id: string, section: string?)
	local p: any = sessions.places:peek() or {}
	local sections = {}
	for _, s in p.sections or {} do
		local list = {}
		for _, sid in s.sessions or {} do
			if sid ~= id then
				table.insert(list, sid)
			end
		end
		if s.id == section then
			table.insert(list, 1, id)
		end
		local copy_ = table.clone(s)
		copy_.sessions = list
		table.insert(sections, copy_)
	end
	sessions.place({ pinned = table.clone(p.pinned or {}), sections = sections })
end

-- A chat's (an orchestrator's too, `orch` its row): the sidebar's items,
-- the Map's few more (Stop while it works, What's up?, Pin to Progress).
local function chat_entries(id: string, orch: any?): { any }
	local c = chats()[id] or { id = id }
	local now = orchestration.now:peek()
	local crew = data.crew_by_chat(now)[id]
	local working = c.status == "working" or c.status == "waiting"
	local out: { any } = {
		{
			label = locale.s("common.open"),
			icon = "arrow-up-right",
			on_select = function()
				if orch then
					route.orchestrator(id)
				else
					route.open_session(id)
				end
			end,
		},
	}
	if working then
		table.insert(out, {
			label = locale.s("control.map_menu.stop"),
			icon = "stop",
			on_select = function()
				sessions.stop(id):after(function() end, said("control.map_menu.failed"))
			end,
		})
	end
	table.insert(out, {
		label = locale.s("control.map_menu.mention"),
		icon = "chat-round-line",
		on_select = function()
			M.mention(id)
		end,
	})
	table.insert(out, {
		label = locale.s("ds.orch.progress.whats_up"),
		icon = "info-square",
		on_select = function()
			progress.check(id)
		end,
	})
	local pinned = progress.pinned(id)
	table.insert(out, {
		label = locale.s(if pinned then "control.map_menu.unpin" else "ds.orch.progress.pin"),
		icon = "pin",
		on_select = function()
			progress.pin(id, not pinned)
		end,
	})
	table.insert(out, { separator = true })
	table.insert(out, {
		label = locale.s("common.rename"),
		icon = "pen",
		on_select = function()
			rename("chat", id, if orch then orch.name or "" else c.title or "")
		end,
	})
	-- Into one of the sidebar's sections, or none.
	local into: { any } = {}
	for _, s in ((sessions.places:peek() or {}) :: any).sections or {} do
		table.insert(into, {
			label = s.name or "",
			on_select = function()
				move_to(id, s.id)
			end,
		})
	end
	if #into > 0 then
		table.insert(into, { separator = true })
	end
	table.insert(into, {
		label = locale.s("control.map_menu.no_section"),
		on_select = function()
			move_to(id, nil)
		end,
	})
	table.insert(out, { label = locale.s("shell.sidebar.menu.move_to"), icon = "folder-with-files", submenu = into })
	table.insert(out, {
		label = locale.s("common.copy"),
		icon = "copy",
		submenu = {
			{
				label = locale.s("shell.sidebar.menu.zeron_link"),
				icon = "arrow-up-right",
				on_select = function()
					sessions.link(id):after(function(link: any)
						copy(if type(link) == "string" then link else nil, locale.s("shell.sidebar.menu.zeron_link"))
					end, said("shell.sidebar.menu.link_not_ready"))
				end,
			},
			{
				label = locale.s("shell.sidebar.menu.session_id"),
				icon = "document",
				on_select = function()
					copy(id, locale.s("shell.sidebar.menu.session_id"))
				end,
			},
		},
	})
	-- Sleep, or Wake (as the sidebar's Activity has them).
	if c.sleep_until then
		table.insert(out, {
			label = locale.s("shell.activity.wake"),
			icon = "sun",
			on_select = function()
				require("@app/screens/shell/activity").wake(id)
			end,
		})
	else
		local activity = require("@app/screens/shell/activity") :: any
		local times: { any } = {}
		for _, p in activity.presets() do
			table.insert(times, {
				label = locale.s("shell.activity.sleep.choice", { name = p.label, when = p.detail }),
				on_select = function()
					activity.sleep(id, p.until_at)
				end,
			})
		end
		table.insert(out, { label = locale.s("shell.activity.sleep.title"), icon = "moon", submenu = times })
	end
	table.insert(out, { separator = true })
	if orch then
		table.insert(out, {
			label = locale.s(if orch.paused then "orch.sidebar.resume" else "orch.sidebar.pause"),
			icon = if orch.paused then "action-play" else "action-pause",
			on_select = function()
				orch_actions.set_paused(id, not orch.paused)
			end,
		})
		table.insert(out, {
			label = locale.s("orch.sidebar.settings"),
			icon = "settings-minimalistic",
			on_select = function()
				orch_actions.settings(id)
			end,
		})
		table.insert(out, {
			label = locale.s("orch.sidebar.archive"),
			icon = "archive-minimalistic",
			on_select = function()
				orch_actions.archive(id)
			end,
		})
		return out
	end
	if crew then
		table.insert(out, {
			label = locale.s("control.map_menu.release"),
			icon = "close-circle",
			on_select = function()
				orchestration.op({ op = "releaseFromCrew", chatId = id }):after(function() end, said("control.map_menu.failed"))
			end,
		})
	else
		table.insert(out, {
			label = locale.s("orch.sidebar.make_orchestrator"),
			icon = "users-group-rounded",
			on_select = function()
				orch_actions.promote(id)
			end,
		})
	end
	table.insert(out, {
		label = locale.s("common.archive"),
		icon = "archive-minimalistic",
		disabled = if working then locale.s("shell.sidebar.menu.archive_working") else nil,
		on_select = function()
			sessions.archive(id)
		end,
	})
	return out
end

-- The Map's own first items for a chat: Open, Stop (while it works),
-- What's up?, Pin to Progress.
local function lead(id: string, orch: any?): { any }
	local c = chats()[id] or {}
	local out: { any } = {
		{
			label = locale.s("common.open"),
			icon = "arrow-up-right",
			on_select = function()
				if orch then
					route.orchestrator(id)
				else
					route.open_session(id)
				end
			end,
		},
	}
	if c.status == "working" or c.status == "waiting" then
		table.insert(out, {
			label = locale.s("control.map_menu.stop"),
			icon = "stop",
			on_select = function()
				sessions.stop(id):after(function() end, said("control.map_menu.failed"))
			end,
		})
	end
	table.insert(out, {
		label = locale.s("control.map_menu.mention"),
		icon = "chat-round-line",
		on_select = function()
			M.mention(id)
		end,
	})
	table.insert(out, {
		label = locale.s("ds.orch.progress.whats_up"),
		icon = "info-square",
		on_select = function()
			progress.check(id)
		end,
	})
	local pinned = progress.pinned(id)
	table.insert(out, {
		label = locale.s(if pinned then "control.map_menu.unpin" else "ds.orch.progress.pin"),
		icon = "pin",
		on_select = function()
			progress.pin(id, not pinned)
		end,
	})
	table.insert(out, { separator = true })
	return out
end

-- The shared menu, the Map's way: its lead first; a crew thread released
-- rather than made an orchestrator; an orchestrator's own last items.
local function shared(id: string, orch: any?): { any }?
	local base = ChatMenu.entries(id, { delete = false })
	if not base then
		return nil
	end
	local crew = data.crew_by_chat(orchestration.now:peek())[id] ~= nil
	local make = locale.s("orch.sidebar.make_orchestrator")
	local archive = locale.s("common.archive")
	local out = lead(id, orch)
	for _, e in base do
		if e and e.label == make and (crew or orch) then
			if crew and not orch then
				table.insert(out, {
					label = locale.s("control.map_menu.release"),
					icon = "close-circle",
					on_select = function()
						orchestration.op({ op = "releaseFromCrew", chatId = id }):after(function() end, said("control.map_menu.failed"))
					end,
				})
			end
		elseif e and e.label == archive and orch then
			-- (Its own, below.)
		elseif e then
			table.insert(out, e)
		end
	end
	if orch then
		table.insert(out, {
			label = locale.s(if orch.paused then "orch.sidebar.resume" else "orch.sidebar.pause"),
			icon = if orch.paused then "action-play" else "action-pause",
			on_select = function()
				orch_actions.set_paused(id, not orch.paused)
			end,
		})
		table.insert(out, {
			label = locale.s("orch.sidebar.settings"),
			icon = "settings-minimalistic",
			on_select = function()
				orch_actions.settings(id)
			end,
		})
		table.insert(out, {
			label = locale.s("orch.sidebar.archive"),
			icon = "archive-minimalistic",
			on_select = function()
				orch_actions.archive(id)
			end,
		})
	end
	return out
end

function M.entries(id: string): { any }?
	local device = string.match(id, "^dev:(.+)$")
	if device then
		local name = ""
		for _, d in sessions.devices:peek() or {} do
			if d.id == device then
				name = d.name or ""
			end
		end
		return {
			{
				label = locale.s("control.map_menu.rename_device"),
				icon = "pen",
				on_select = function()
					rename("device", device, name)
				end,
			},
		}
	end
	for _, o in (orchestration.now:peek() or {}).orchestrators or {} do
		if o.id == id then
			return shared(id, o) or chat_entries(id, o)
		end
	end
	return shared(id, nil) or chat_entries(id, nil)
end

-- The rename it opens: a chat's (an orchestrator's) or a machine's name.
function M.Dialogs(): Node
	local text = ui.state("map_menu_rename_text", "")
	ui.effect(function()
		local r = M.renaming:get()
		if r then
			text:set(r.text or "")
		end
	end)
	local function save()
		local r = M.renaming:peek()
		local name = string.match(text:peek(), "^%s*(.-)%s*$") or ""
		rename_open:set(false)
		if not r or name == "" then
			return
		end
		if r.kind == "device" then
			sessions.rename_device(r.id, name):after(function() end, said("control.map_menu.failed"))
		else
			local is_orch = false
			for _, o in (orchestration.now:peek() or {}).orchestrators or {} do
				if o.id == r.id then
					is_orch = true
				end
			end
			if is_orch then
				orch_actions.update(r.id, { name = name })
			else
				sessions.rename(r.id, name)
			end
		end
	end
	return ds.Dialog {
		open = rename_open,
		title = function(): string
			local r = M.renaming:get()
			return locale.s(if r and r.kind == "device" then "control.map_menu.rename_device" else "shell.sidebar.session.rename_title")
		end,
		confirm = { label = locale.t("common.rename"), on_click = save },
		ds.TextField {
			value = text,
			w_full = true,
			on_key = function(e: any): boolean?
				if e.key == "Enter" then
					save()
					return true
				end
				return nil
			end,
		},
	}
end

return M
