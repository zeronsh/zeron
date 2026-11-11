-- Settings → Projects: the folders your sessions work in, under the device
-- each is on (this one first, then your other computers, then Cloud's),
-- each with its tile, its sessions and its folder, found by name or path.
-- Each row's ⋯ renames it in place (every device then shows the name),
-- changes its icon and color in the window's one Edit project (its tile
-- does too, at a click), shows this device's folder in Finder, and removes
-- it: Zeron forgets the
-- project and deletes its sessions, as the sidebar's Remove and the
-- shipping app's `delete_space` do (`deleteSpace`); the folder and its
-- files stay where they are, which the confirmation says plainly. Sessions
-- running in it are stopped first, wherever they run, so none carries on
-- unseen with no row of its own.
--
-- The groups and their rows are keyed (`ui.each`, `Settings.Each`), so a
-- project arriving, renamed or removed changes its row alone. Before the
-- engine answers, skeleton rows hold their place; with no engine, a callout
-- says so; none at all, the way to add one.

local ds = require("@ds")
local Menu = require("@ds/menu")
local locale = require("@ds/locale")
local toast = require("@ds/toast")
local sessions = require("@account/sessions")
local projects = require("@account/projects")
local search = require("@app/screens/settings/search")
local rename = require("@app/screens/settings/rename_row")
local look = require("@app/screens/shell/project_look")
local files = require("@app/screens/files/api")
local cloud_projects = require("@app/screens/shell/cloud_projects")

local M = {}

M.ID = "projects"
M.TITLE = "Projects"

search.index(M.ID, ui.t("settings.projects.title"), ui.t("settings.projects.terms"), {
	{ key = "projects", title = ui.t("settings.projects.yours.label"), terms = ui.t("settings.projects.yours.terms") },
	{ key = "projects", title = ui.t("settings.projects.rename_one.label"), terms = ui.t("settings.projects.rename_one.terms") },
	{ key = "projects", title = ui.t("settings.projects.remove_one.label"), terms = ui.t("settings.projects.remove_one.terms") },
})

local query = ui.state("settings_projects_query", "")
-- Renaming: which project (its id; "" none) and the name being typed.
local renaming = ui.state("settings_project_renaming", "")
local new_name = ui.state("settings_project_name", "")
-- Names saved and not yet back from the engine, by project.
local pending = ui.state("settings_project_pending_names", {} :: { [string]: string })
-- Removing: which one is asked about, and whether that's on its way.
local removing = ui.state("settings_project_removing", "")
-- (What it said as it was asked about: the dialog keeps it while it leaves,
-- the project gone by then.)
local removing_name = ui.state("settings_project_removing_name", "")
local said_last = ""
local removing_open = ui.state("settings_project_removing_open", false)
local removing_busy = ui.state("settings_project_removing_busy", false)

-- A session's live states that hold a run open on its device.
local RUNNING: { [string]: boolean } = { working = true, waiting = true, input = true }

local function trimmed(s: string): string
	return string.match(s, "^%s*(.-)%s*$") or ""
end

local function project(id: string): sessions.Project?
	for _, p in sessions.projects:get() do
		if p.id == id then
			return p
		end
	end
	return nil
end

local function name_of(p: sessions.Project): string
	return pending:get()[p.id] or p.name
end

local function set_pending(id: string, name: string?)
	pending:set_key(id, name)
end

-- What the engine sends back settles a pending name.
ui.effect(function()
	for id, name in pending:get() do
		local p = project(id)
		if not p or p.name == name then
			set_pending(id, nil)
		end
	end
end)

-- Its sessions, archived ones too (a removal deletes them all), and those
-- running now.
local function sessions_in(id: string): ({ sessions.Chat }, { sessions.Chat })
	local all: { sessions.Chat } = {}
	local running: { sessions.Chat } = {}
	local now = sessions.now:get()
	for _, list in { now.chats, now.archived or {} } do
		for _, c in list do
			if c.project == id then
				table.insert(all, c)
				if c.status and RUNNING[c.status] then
					table.insert(running, c)
				end
			end
		end
	end
	return all, running
end

-- How many sessions each project has, archived ones too: counted once a
-- change, for every row (the chats change every few seconds while they run).
local counts = ui.computed(function(): { [string]: number }
	local out: { [string]: number } = {}
	local now = sessions.now:get()
	for _, list in { now.chats, now.archived or {} } do
		for _, c in list do
			if c.project then
				out[c.project] = (out[c.project] or 0) + 1
			end
		end
	end
	return out
end)

local function is_cloud(device: string): boolean
	return string.sub(device, 1, 6) == "cloud-" or device == "cloud"
end

-- A row: a project, or (Cloud's) one repository's projects on Cloud's
-- machines as one: `key` names that row, `members` are its projects, and
-- the rest is the one it reads as (screens/shell/cloud_projects).
type Row = sessions.Project & { key: string?, members: { sessions.Project }? }

-- Which devices are Cloud's (as the engine lists them, or by their id).
local function cloud_devices(): { [string]: boolean }
	local out: { [string]: boolean } = {}
	local all = sessions.devices:get()
	local yours: { [string]: boolean } = {}
	for _, d in sessions.computers(all) do
		yours[d.id] = true
	end
	for _, d in all do
		if (d.cloud or d.platform == "cloud") and not yours[d.id] then
			out[d.id] = true
		end
	end
	for _, p in sessions.projects:get() do
		if not yours[p.device] and is_cloud(p.device) then
			out[p.device] = true
		end
	end
	return out
end

-- Cloud's rows, by key, whatever the search.
local cloud_rows = ui.computed(function(): { [string]: Row }
	local on_cloud = cloud_devices()
	local members: { [string]: { sessions.Project } } = {}
	for _, p in sessions.projects:get() do
		if on_cloud[p.device] then
			local key = cloud_projects.key(p)
			members[key] = members[key] or {}
			table.insert(members[key], p)
		end
	end
	local out: { [string]: Row } = {}
	for key, list in members do
		local row = table.clone(cloud_projects.lead(list)) :: Row
		row.key = key
		row.members = list
		out[key] = row
	end
	return out
end)

-- A row by its key (a project's id, or a Cloud row's key): the one it reads
-- as, and the projects it stands for.
local function row_of(key: string): (sessions.Project?, { sessions.Project })
	local row = cloud_rows:get()[key]
	if row then
		return row, row.members or {}
	end
	local p = project(key)
	return p, if p then { p } else {}
end

-- The device a project is on, as its group heads it.
type Group = { id: string, name: string, here: boolean, cloud: boolean, gone: boolean, projects: { Row } }

local function by_name(a: Row, b: Row): boolean
	local x, y = string.lower(name_of(a)), string.lower(name_of(b))
	if x ~= y then
		return x < y
	end
	return a.id < b.id
end

-- The groups shown: this device's, your other computers' (as Devices lists
-- them), Cloud's (one group, a repository once), then any on a device since
-- removed; each with the projects the search finds, by name (Cloud's, by
-- repository too).
local groups = ui.computed(function(): { Group }
	local q = string.lower(trimmed(query:get()))
	local on_cloud = cloud_devices()
	local by_device: { [string]: { Row } } = {}
	for _, p in sessions.projects:get() do
		if on_cloud[p.device] then
			continue
		end
		local hay = string.lower(`{name_of(p)} {p.path}`)
		if q == "" or string.find(hay, q, 1, true) then
			by_device[p.device] = by_device[p.device] or {}
			table.insert(by_device[p.device], p :: Row)
		end
	end
	for _, list in by_device do
		table.sort(list, by_name)
	end
	local clouds: { Row } = {}
	for _, row in cloud_rows:get() do
		local hay = string.lower(`{name_of(row)} {row.github or ""} {row.path}`)
		if q == "" or string.find(hay, q, 1, true) then
			table.insert(clouds, row)
		end
	end
	table.sort(clouds, by_name)
	local out: { Group } = {}
	local placed: { [string]: boolean } = {}
	local function add(id: string, name: string, here: boolean, cloud: boolean, gone: boolean)
		if by_device[id] and not placed[id] then
			placed[id] = true
			table.insert(out, { id = id, name = name, here = here, cloud = cloud, gone = gone, projects = by_device[id] })
		end
	end
	local all = sessions.devices:get()
	for _, d in sessions.computers(all) do
		add(d.id, d.name, d.here == true, false, false)
	end
	if #clouds > 0 then
		table.insert(out, { id = "cloud", name = ui.t("settings.projects.cloud"):get(), here = false, cloud = true, gone = false, projects = clouds })
	end
	local rest: { string } = {}
	for id in by_device do
		if not placed[id] then
			table.insert(rest, id)
		end
	end
	table.sort(rest)
	for _, id in rest do
		add(id, ui.t("settings.projects.device_gone"):get(), false, false, true)
	end
	return out
end)

-- Why a change didn't go, in words.
local function why(err: any): string
	local word = if type(err) == "string" then err elseif type(err) == "table" or type(err) == "userdata" then err.message else nil
	if word == "offline" then
		return ui.t("settings.engine.offline"):get()
	elseif type(word) == "string" and word ~= "" and word ~= "failed" and #word <= 160 then
		-- (The engine's own words, a preview's refusal among them.)
		return word
	end
	return ui.t("settings.devices.try_later"):get()
end

-- A row's sessions: those of every project it stands for.
local function sessions_of(members: { sessions.Project }): ({ sessions.Chat }, { sessions.Chat })
	local all: { sessions.Chat } = {}
	local running: { sessions.Chat } = {}
	for _, m in members do
		local a, r = sessions_in(m.id)
		table.move(a, 1, #a, #all + 1, all)
		table.move(r, 1, #r, #running + 1, running)
	end
	return all, running
end

-- (A Cloud row's name: given to each of its projects.)
local function save_rename(key: string)
	local name = trimmed(new_name:peek())
	renaming:set("")
	local p, members = row_of(key)
	if not p or name == "" or name == name_of(p) then
		return
	end
	local said = false
	for _, m in members do
		if name_of(m) ~= name then
			local id = m.id
			set_pending(id, name)
			sessions.rename_project_or_say(id, name):after(function() end, function(err: any)
				set_pending(id, nil)
				if not said then
					said = true
					toast.show { title = ui.t("settings.projects.couldnt_rename"):get(), body = why(err), tone = "danger" }
				end
			end)
		end
	end
end

local function remove()
	local key = removing:peek()
	if key == "" or removing_busy:peek() then
		return
	end
	local _, members = row_of(key)
	removing_busy:set(true)
	local function done(err: any?)
		ui.batch(function()
			removing_busy:set(false)
			removing_open:set(false)
		end)
		if err ~= nil then
			toast.show { title = ui.t("settings.projects.couldnt_remove"):get(), body = why(err), tone = "danger" }
		end
	end
	-- (A Cloud row's: each of its projects; done when they all are, the
	-- first refusal said.)
	local function go()
		local left, failed = #members, nil :: any
		if left == 0 then
			done(nil)
			return
		end
		local function one(err: any?)
			if err ~= nil and failed == nil then
				failed = err
			end
			left -= 1
			if left == 0 then
				done(failed)
			end
		end
		for _, m in members do
			projects.delete(m.id):after(function()
				one(nil)
			end, function(err: any)
				one(err or "failed")
			end)
		end
	end
	-- Its running sessions stopped first, on whichever device runs them:
	-- the removal deletes their rows, and a run with none would carry on
	-- where nothing shows it. (One that can't be reached isn't running.)
	local _, running = sessions_of(members)
	local left = #running
	if left == 0 then
		go()
		return
	end
	for _, c in running do
		sessions.stop(c.id):after(function()
			left -= 1
			if left == 0 then
				go()
			end
		end, function()
			left -= 1
			if left == 0 then
				go()
			end
		end)
	end
end

-- The confirmation's words: what goes, what's stopped, and what stays.
local function removal_words(): string
	local p, members = row_of(removing:get())
	if not p then
		return said_last
	end
	local all, running = sessions_of(members)
	local parts = {
		if #all == 0 then ui.t("settings.projects.remove_none"):get() else ui.t("settings.projects.remove_sessions", { n = #all }):get(),
	}
	if #running > 0 then
		-- (Its only session, or some of several.)
		table.insert(
			parts,
			if #all == 1 then ui.t("settings.projects.remove_running_only"):get() else ui.t("settings.projects.remove_running", { n = #running }):get()
		)
	end
	local where: string? = nil
	for _, d in sessions.devices:get() do
		if d.id == p.device and not d.cloud then
			where = d.name
		end
	end
	if is_cloud(p.device) then
		table.insert(parts, ui.t("settings.projects.remove_keeps_repo"):get())
	else
		table.insert(parts, ui.t("settings.projects.remove_keeps_folder", { path = p.path, device = where or ui.t("settings.projects.its_device"):get() }):get())
	end
	said_last = table.concat(parts, " ")
	return said_last
end

local function project_row(_p: Row, _i: number, p: State<Row>): Node
	-- (A Cloud row's key; else its project's id.)
	local id = p:peek().key or p:peek().id
	local merged = p:peek().members ~= nil
	local menu_open = ui.use_state(false)
	local trigger = ui.ref()
	local function refocus()
		ui.after(0, function()
			local el = trigger:peek()
			if el then
				el:focus()
			end
		end)
	end
	local more = ds.IconButton {
		icon = "more-horizontal",
		label = ui.t("settings.projects.more_for", { name = p:peek().name }),
		size = "md",
		tone = "quiet",
		on_click = function()
			menu_open:set(not menu_open:peek())
		end,
	}
	-- Its name and icon in the window's one editor (Edit project: the
	-- sidebar's, screens/shell/project_editor), on every device. (A Cloud
	-- row's: what's saved goes to each of its projects.)
	local function edit()
		local now = p:peek()
		local others: { string }? = nil
		if now.members then
			others = {}
			for _, m in now.members do
				if m.id ~= now.id then
					table.insert(others :: { string }, m.id)
				end
			end
		end
		require("@app/screens/shell/sidebar").edit_project(now.id, nil, others)
	end
	local function face(): Node
		return ds.Tile {
			size = 28,
			project = if merged
				then function(): string?
					return p:get().id
				end
				else id,
			label = function(): string
				return name_of(p:get())
			end,
			hue = function(): string?
				return look.of(p:get()).hue
			end,
			src = function(): string?
				return look.of(p:get()).src
			end,
			emoji = function(): string?
				return look.of(p:get()).emoji
			end,
		}
	end
	-- The tile edits its icon at a click (or Space), as the editor's preview
	-- is the tile itself.
	local function tile(): Node
		return ui.div {
			shrink = 0,
			role = "button",
			label = ui.t("settings.projects.icon_of", { name = p:peek().name }),
			tooltip = ui.t("settings.projects.change_icon_tip"):get(),
			focusable = true,
			cursor = "pointer",
			rounded = "$radius.md",
			hover = { opacity = 0.85 },
			active = { opacity = 0.7 },
			focus = { outline = "$zeron.focus.width", outline_color = "$zeron.focus.ring", outline_offset = 2 },
			on_click = edit,
			face(),
		}
	end
	local menu = ui.menu(Menu.panel {
		open = menu_open,
		anchor = more,
		placement = { side = "bottom", align = "end", offset = 4 },
		w = 220,
		Menu.Group {
			Menu.Row {
				icon = "pen",
				label = locale.s("settings.projects.rename"),
				on_click = function()
					menu_open:set(false)
					new_name:set(name_of(p:peek()))
					renaming:set(id)
				end,
			},
			Menu.Row {
				icon = "palette",
				label = locale.s("settings.projects.change_icon"),
				on_click = function()
					menu_open:set(false)
					edit()
				end,
			},
			-- (A folder on this device: shown where it is.)
			ui.show(
				function(): boolean
					return p:get().device == sessions.here:get()
				end,
				Menu.Row {
					icon = "folder",
					label = locale.s("settings.projects.show"),
					on_click = function()
						menu_open:set(false)
						local now = p:peek()
						files.reveal({ space = now.id, device = now.device, root = now.path }, ""):after(function() end, function(err: any)
							toast.show { title = ui.t("settings.projects.couldnt_show"):get(), body = why(err), tone = "danger" }
						end)
					end,
				}
			),
		},
		Menu.Rule {},
		Menu.Group {
			Menu.Row {
				icon = "trash-bin-minimalistic",
				label = locale.s("settings.projects.remove"),
				danger = true,
				on_click = function()
					menu_open:set(false)
					removing:set(id)
					removing_name:set(name_of(p:peek()))
					removing_open:set(true)
				end,
			},
		},
	})
	return rename.Row {
		row = ds.Settings.Row {
			label = function(): string
				return name_of(p:get())
			end,
			-- How many sessions, then its folder (fading where it's long).
			description = function(): string
				local now = p:get()
				local n = 0
				local all = counts:get()
				for _, m in now.members or { now } do
					n += all[m.id] or 0
				end
				local count = if n == 0 then ui.t("settings.projects.no_sessions"):get() else ui.t("settings.projects.sessions", { n = n }):get()
				local where = if now.members or is_cloud(now.device) then (now.github or now.path) else now.path
				return if where and where ~= "" then `{count} · {where}` else count
			end,
			lead = tile(),
			truncate = true,
			control = ui.div { ref = trigger, menu },
		},
		lead = face,
		editing = function(): boolean
			return renaming:get() == id
		end,
		value = new_name,
		name = ui.t("settings.projects.name_of", { name = p:peek().name }),
		can_save = function(): boolean
			return trimmed(new_name:get()) ~= ""
		end,
		on_save = function()
			save_rename(id)
			refocus()
		end,
		on_cancel = function()
			renaming:set("")
			refocus()
		end,
	}
end

-- A device's projects, under its name.
local function group(_g: Group, _i: number, g: State<Group>): Node
	local list = ui.computed(function(): { Row }
		return g:get().projects
	end)
	local function name(): string
		return g:get().name
	end
	-- A Settings.Section, its "This device" right after the name as Devices'
	-- row has it (a section's own `trail` stands at the far edge).
	return ui.column {
		w_full = true,
		gap = "$space.2",
		role = "group",
		label = name,
		ui.row {
			px = ds.Settings.inset(),
			gap = "$space.2",
			items = "center",
			ds.SectionTitle { name, heading = 2 },
			ui.show(function(): boolean
				return g:get().here
			end, ds.Badge { ui.t("settings.devices.this_device"), size = "sm" }),
		},
		ds.Settings.Each(list, project_row, {
			key = function(p: Row): string
				return p.key or p.id
			end,
		}),
	}
end

local function add_project()
	require("@app/screens/shell/add_project").open()
end

local function list_kind(): string
	if #groups:get() > 0 then
		return "projects"
	end
	local s = sessions.state:get()
	if #sessions.projects:get() > 0 then
		return "nothing_found"
	end
	return if s == "offline" then "offline" elseif s == "ready" then "none" else "loading"
end

local function list_body(kind: string): Node
	if kind == "loading" then
		-- (Said, not only drawn: the bars are hidden from assistive technology.)
		return ui.column {
			w_full = true,
			label = ui.t("common.loading"),
			live = "polite",
			ds.Settings.Group {
				ds.SkeletonRow { trail = 28 },
				ds.SkeletonRow { title = 110, line = 220, trail = 28 },
				ds.SkeletonRow { title = 130, line = 200, trail = 28 },
			},
		}
	elseif kind == "offline" then
		return ds.Callout {
			tone = "danger",
			title = ui.t("settings.projects.offline_title") :: any,
			body = ui.t("settings.engine.offline") :: any,
		}
	elseif kind == "none" then
		return ds.EmptyState {
			icon = "folder-with-files",
			title = ui.t("settings.projects.empty_title") :: any,
			body = ui.t("settings.projects.empty_body") :: any,
			size = "sm",
			action = { label = ui.t("settings.projects.add") :: any, on_click = add_project },
		}
	elseif kind == "nothing_found" then
		return ds.EmptyState {
			icon = "search",
			title = ui.t("settings.projects.nothing_found") :: any,
			body = ui.t("settings.projects.nothing_found_body") :: any,
			size = "sm",
		}
	end
	return ui.each(groups, group, {
		key = function(g: Group): string
			return g.id
		end,
		w_full = true,
		gap = "$space.6",
	})
end

function M.view(): Node
	return ui.column {
		w_full = true,
		items = "center",
		ds.Settings.Page {
			title = ui.t("settings.projects.title"),
			description = ui.t("settings.projects.description"),
			-- The search beside the one action, over the list it filters.
			trail = ui.row {
				gap = "$space.2",
				items = "center",
				ds.SearchField {
					value = query,
					placeholder = ui.t("settings.projects.find") :: any,
					size = "sm",
					w = 180,
					name = ui.t("settings.projects.find") :: any,
				},
				ds.Button { ui.t("settings.projects.add"), size = "sm", on_click = add_project },
			},
			search.anchor(
				"projects",
				ui.presence(list_kind, list_body, {
					w_full = true,
					enter = { opacity = { 0, 1 }, duration = 120 },
					exit = { opacity = 0, duration = 60 },
					mode = "wait",
				})
			),
		},
		ds.Dialog {
			open = removing_open,
			title = (function(): string
				return ui.t("settings.projects.remove_title", { name = removing_name:get() }):get()
			end) :: any,
			body = removal_words :: any,
			tone = "danger",
			size = "sm",
			confirm = {
				label = ui.t("settings.projects.remove_confirm"),
				keep_open = true,
				busy = function(): boolean
					return removing_busy:get()
				end,
				on_click = remove,
			},
		},
	}
end

return M
