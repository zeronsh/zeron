-- The sessions behind the main window, as the host keeps them
-- (src/sessions.rs): the chats, the projects they work in, the devices
-- they run on, each chat's state, and where sessions are put by hand.
-- Screens read `sessions.now` and ask for what changes it. Every item
-- carries an `id`, so a page keys its rows by it and they update in place.
--
--   sessions.now:get()     { state = "loading" | "ready", here?, devices,
--                            projects, chats, archived, places, places_ready }
--   sessions.create(req)   a new chat: a promise of its id
--                          ({ project?, device?, config?, branch?, cwd?, title?,
--                          parent? }: with `parent`, a side chat of it, where it is;
--                          with `cloud`, on a Cloud machine of its own: a
--                          promise of { id, cwd }, or why not in words)
--   sessions.fork(source, parent?, id?)  a side chat starting as a copy of
--                          `source` (Fork chat): a promise of its id, or the
--                          engine's words for why not
--   sessions.children(id)  a chat's side chats, newest first (tracked)
--   sessions.archive(id), sessions.unarchive(id)
--   sessions.rename(id, title)
--   sessions.rename_project(id, name)
--   sessions.set_project_icon(id, icon)  its picture, whole, everywhere: a
--                          promise ({ emoji?, image?, hue? }; nil, its
--                          default tile again)
--   sessions.rename_device(id, name)  a promise (Settings → Devices)
--   sessions.remove_device(id)  another of your devices off the account, on
--                          every device: a promise, refused "here" (this
--                          one), "online" (it's on now), "cloud", "outdated"
--                          or "offline"; its sessions and projects stay
--   sessions.rename_project_or_say(id, name)  rename_project, as a promise
--   sessions.delete(id)    gone for good: a promise
--   sessions.deleted_with(id)  what its delete takes with it: a promise of
--                          { subagents, sides } (theirs in turn; archived too)
--   sessions.link(id)      its Zeron conversation link: a promise
--   sessions.collapse(id, folded)  a section of yours folded, everywhere
--   sessions.seen(id)      its finished state seen, on every device
--   sessions.sleep(id, until?)  asleep until `until` (Unix seconds), on every
--                          device; none, awake (its host wakes it early when
--                          it asks for you)
--   sessions.stop(id)      its turn stopped, wherever it runs: a promise
--   sessions.stop_waiting(id)  what its agent said it waits on no longer
--                          holds it
--   sessions.place(places) where sessions are put by hand, as the sidebar
--                          has them now ({ pinned, sections })
--
-- A chat: { id, title, project?, device, branch?, cwd?, status?, updated,
-- created, harness?, agent?, model? }: `status` the sidebar's state
-- ("working", "waiting" on background work that will wake it, "input",
-- "failed", "done"), nil idle; `updated` and `created`
-- Unix seconds; `agent` ds.Mark's name for its harness. A project: { id,
-- name, device, path, git, github? }. A device: { id, name, platform, here }.

local M = {}

export type Chat = {
	id: string,
	title: string,
	project: string?,
	device: string,
	branch: string?,
	cwd: string?,
	status: string?,
	updated: number,
	created: number,
	harness: string?,
	agent: string?,
	model: string?,
	-- Its harness's own id for the conversation (a Codex thread's).
	harness_session: string?,
	-- Its subagents streaming now (the sidebar's pill), and its side chats:
	-- how many, and how many are working.
	running_subagents: number?,
	side_chats: number?,
	working_side_chats: number?,
	-- What its agent waits on after its turn (status "waiting").
	waiting: { WaitingOn }?,
	-- Put to sleep: until when; or, woken and not looked at since, when it
	-- woke (Unix seconds).
	sleep_until: number?,
	woke: number?,
}
export type WaitingOn = { id: string, kind: string, label: string }
-- A side chat: a chat under another (its `parent`).
export type Child = {
	id: string,
	parent: string,
	title: string,
	device: string,
	status: string?,
	updated: number,
	created: number,
	harness: string?,
	agent: string?,
	-- Started by a Zeron chat (through Zeron's tools), as a subagent or a
	-- side chat: "subagent" | "side"; nil for one the person started.
	child_kind: string?,
}
-- `github`: its GitHub repository (`owner/name`), the one a Cloud chat of it
-- clones.
-- A project's chosen picture: an emoji, or an uploaded image's https URL,
-- on a palette hue (each optional; none of them is no icon).
export type ProjectIcon = { emoji: string?, image: string?, hue: string? }
export type Project = { id: string, name: string, device: string, path: string, git: boolean, github: string?, repository: string?, created: number?, icon: ProjectIcon? }
-- `online`: its presence heartbeat fresh (this device always), published
-- again as that changes; `seen`: while it's away, its last beat, in unix
-- seconds ("last seen …"; online, none: its beat moves every few seconds and
-- shows nowhere).
-- `cloud`: one of Cloud's (the account's, or a session's machine): never a
-- device to pick.
export type Device = { id: string, name: string, platform: string, here: boolean, online: boolean?, seen: number?, version: string?, cloud: boolean? }
export type Section = { id: string, name: string, sessions: { string }, collapsed: boolean? }
export type Places = { pinned: { string }, sections: { Section } }
export type Now = {
	state: string,
	here: string?,
	devices: { Device },
	projects: { Project },
	chats: { Chat },
	-- Archived ones, as chats are, the most recently moved first.
	archived: { Chat },
	-- Each chat's side chats, by its id, newest first.
	children: { [string]: { Child } }?,
	places: Places,
	places_ready: boolean,
	-- The engine's connection, when it's not healthy (the sidebar's line):
	-- "unsaved" (a chat's changes couldn't be saved), "offline",
	-- "reconnecting"; nil, fine.
	connection: string?,
}

M.now = ui.shared("sessions", {
	state = "loading",
	devices = {},
	projects = {},
	chats = {},
	archived = {},
	places = { pinned = {}, sections = {} },
	places_ready = false,
} :: Now)

-- Its parts, each followed on its own: a chat's message changes `chats`
-- (every few seconds while chats run), and what reads only the projects,
-- the devices or the places doesn't run again for it. Read one of these,
-- not `now:get().projects`, where the part is all that's used.
M.projects = ui.computed(function(): { Project }
	return M.now:get().projects
end)
M.devices = ui.computed(function(): { Device }
	return M.now:get().devices
end)
M.places = ui.computed(function(): Places
	return M.now:get().places
end)
M.here = ui.computed(function(): string?
	return M.now:get().here
end)
M.state = ui.computed(function(): string
	return M.now:get().state
end)
M.archived = ui.computed(function(): { Chat }
	return M.now:get().archived or {}
end)
M.connection = ui.computed(function(): string?
	return M.now:get().connection
end)

local function quietly(): (any) -> ()
	return function() end
end

function M.create(req: { [string]: any }): any
	return ui.call("sessions.create", req)
end

function M.fork(source: string, parent: string?, id: string?): any
	return ui.call("sessions.fork", { source = source, parent = parent, id = id })
end

function M.children(id: string?): { Child }
	if id == nil then
		return {}
	end
	return (M.now:get().children or {})[id] or {}
end

function M.archive(id: string)
	ui.call("sessions.archive", id):after(function() end, quietly())
end

function M.unarchive(id: string)
	ui.call("sessions.unarchive", id):after(function() end, quietly())
end

function M.rename(id: string, title: string)
	ui.call("sessions.rename", { id = id, title = title }):after(function() end, quietly())
end

function M.rename_project(id: string, name: string)
	ui.call("sessions.rename_project", { id = id, name = name }):after(function() end, quietly())
end

function M.set_project_icon(id: string, icon: ProjectIcon?): any
	local i = icon or {}
	return ui.call("sessions.set_project_icon", { id = id, emoji = i.emoji, image = i.image, hue = i.hue })
end

function M.rename_device(id: string, name: string): any
	return ui.call("sessions.rename_device", { id = id, name = name })
end

function M.remove_device(id: string): any
	return ui.call("sessions.remove_device", id)
end

-- (Settings → Projects says when it didn't go; the sidebar's editor doesn't.)
function M.rename_project_or_say(id: string, name: string): any
	return ui.call("sessions.rename_project", { id = id, name = name })
end

-- Its Zeron conversation link (`zeron://open/chat/…`, the shipping app's,
-- so either opens it): a promise; refused while it can't be known yet.
function M.link(id: string): any
	return ui.call("sessions.link", id)
end
function M.delete(id: string): any
	return ui.call("sessions.delete", id)
end

function M.deleted_with(id: string): any
	return ui.call("sessions.deleted_with", id)
end

function M.collapse(id: string, folded: boolean)
	ui.invoke("sessions.collapse", { id = id, collapsed = folded })
end

function M.seen(id: string)
	ui.invoke("sessions.seen", id)
end

function M.sleep(id: string, until_at: number?): any
	-- (`until` is Luau's word: a key spelled out.)
	return ui.call("sessions.sleep", { id = id, ["until"] = until_at })
end

-- Its turn stopped, wherever it runs (the composer's Stop): a promise.
function M.stop(id: string): any
	return ui.call("chat.interrupt", id)
end

function M.stop_waiting(id: string): any
	return ui.call("chat.stop_waiting", id)
end

function M.place(places: Places)
	ui.invoke("sessions.places", places)
end

-- Your own computers among the devices: not a Cloud session's machine (it
-- belongs to its session) and, with `computers_only`, not a phone. This
-- one first.
function M.computers(devices: { Device }?, computers_only: boolean?): { Device }
	local out: { Device } = {}
	for _, d in devices or {} do
		local phone = d.platform == "ios" or d.platform == "android"
		if not d.cloud and d.platform ~= "cloud" and not (computers_only and phone) then
			table.insert(out, if d.here then 1 else #out + 1, d)
		end
	end
	return out
end

return M
