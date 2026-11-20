--!strict
-- The right panel's home: its page while no tab is in front (it was "Open a
-- tab"). For a session it says, as the shipping app's session card does
-- (crates/ui shell/session_info.rs), where the session runs: its device
-- (away, when it can't be reached), its project and its branch, with what
-- the branch adds over its base (`+N −M`, opening the branch's changes in
-- the Git tab); then the project's run actions (shell/actions_ui.rs: a row
-- runs one in a terminal tab of its own, its gear edits it, a zeron.json
-- action not yet added imports, Add action adds one). Then what can open
-- here, each with the one key that opens it: Browser B, File viewer F,
-- Terminal T, Side chat S, Fork chat K (plugins' rows after them,
-- `hook:panel.empty`; C and H are the Git plugin's). Last, the session's
-- side chats and forks, and its subagents: each a part that folds (folded
-- or not kept per session), a list that builds only the rows in view, a
-- row opening its lane's tab. A part with nothing in it isn't shown: New
-- side chat and Fork chat are above it, and subagents come from the agent.
--
--   Home.Page { key, session, open_kind, open_plugin_tab, open_action,
--               showing }
--   Home.key(e)   a key nothing else took (the panel's, the window's): the
--                 home's letter, while it shows and no text field has the
--                 keyboard. Whether it took it.
--
-- A letter the home has no row for is passed on, for plugins' rows:
-- `panel_key`, provided on the page, a state of `{ key, n }` (`n` moves
-- with each press).

local ds = require("@ds")
local locale = require("@ds/locale")
local PanelHome = require("@ds/panel_home")
local Icon = require("@ds/icon")
local Tile = require("@ds/tile")
local Tooltip = require("@ds/tooltip")
local sessions = require("@account/sessions")
local route = require("@app/screens/shell/route")
local boundary = require("@app/screens/shell/boundary")
local store = require("@app/screens/chat/store")
local Dock = require("@app/screens/chat/dock")
local api = require("@app/screens/panel/home_api")
local Previews = require("@app/screens/panel/previews")
-- Which finished lanes you've seen (their tabs in front).
local lanes_seen = require("@app/screens/panel/lanes_seen")

local M = {}

-- The home showing now (one panel shows at a time): its letters, whether
-- it shows, and where a letter it has no row for goes.
type Live = { letters: { [string]: () -> () }, showing: () -> boolean, pass: (letter: string) -> () }
local live: Live? = nil

-- Each press of a letter the home has no row for (kept outside the views:
-- a reload's rows see the press they saw last as seen).
local passed = ui.state("panel_home_key", { key = "", n = 0 } :: { key: string, n: number })

function M.key(e: any): boolean
	local now = live
	if not now or e.editing or e.cmd or e.ctrl or e.alt or e.meta or e["repeat"] then
		return false
	end
	local k = e.key
	if type(k) ~= "string" or #k ~= 1 or not string.match(k, "%a") then
		return false
	end
	if not now.showing() then
		return false
	end
	local letter = string.upper(k)
	local run = now.letters[letter]
	if run then
		run()
		return true
	end
	now.pass(letter)
	return false
end

-- The sidebar's view options, as the shell's sidebar publishes them
-- (screens/shell/sidebar): the side chats' and subagents' rows follow them.
local sidebar_view = ui.shared("sidebar.view", {} :: { [string]: any })
-- Folded or not, per session and part (open unless folded).
local folds = ui.persist("panel_home_folds", {} :: { [string]: boolean })
-- Side chats and subagents at work above the rest (their parts' menus'
-- "Working first"), kept across launches as the sidebar's view options are.
local working_first = ui.persist("panel_home_working_first", false)
-- Shown whole ("Show less") or not, per session and part (its first few
-- unless shown whole), as the sidebar's groups keep theirs.
local whole = ui.state("panel_home_whole", {} :: { [string]: boolean })

-- Each project's actions, by device and project: { state = "loading" |
-- "ready" | "saving" | "failed" | "unsupported", snapshot?, message? }
-- (`message` the engine's words; ours are drawn from the state).
local actions_by = ui.state("panel_home_actions", {} :: { [string]: any })

local ACTION_ICONS = { "play", "test", "lint", "configure", "build", "debug" }
local function action_icon(icon: string?): Icon.Name
	for _, n in ACTION_ICONS do
		if n == icon then
			return `action-{n}` :: any
		end
	end
	return "action-play"
end

local function trim(s: string?): string
	return string.match(s or "", "^%s*(.-)%s*$") or ""
end

-- The session's row in the sidebar's list, its device and its project.
local function chat_of(id: string?): sessions.Chat?
	if not id then
		return nil
	end
	for _, c in sessions.now:get().chats do
		if c.id == id then
			return c
		end
	end
	return nil
end
local function device_of(id: string?): sessions.Device?
	for _, d in sessions.devices:get() do
		if d.id == id then
			return d
		end
	end
	return nil
end
local function project_of(id: string?): sessions.Project?
	if not id then
		return nil
	end
	for _, p in sessions.projects:get() do
		if p.id == id then
			return p
		end
	end
	return nil
end

-- A device's glyph: a Cloud machine's cloud, this Mac's laptop, a devbox's
-- server, another computer's screen.
local function device_icon(d: sessions.Device?): Icon.Name
	if not d then
		return "monitor"
	elseif d.platform == "cloud" then
		return "cloud"
	elseif d.here then
		return "laptop"
	elseif d.platform == "linux" then
		return "remote-server"
	elseif d.platform == "ios" or d.platform == "android" then
		return "smartphone"
	end
	return "monitor"
end

export type Props = {
	-- The panel's key ("chat:<id>", a project's, a page's) and the session
	-- it's for (none on a project's or a page's panel).
	key: string,
	session: string?,
	open_kind: (kind: string) -> (),
	open_plugin_tab: (session: string, tab: { [string]: any }) -> (),
	-- An action run in a terminal tab of its own (its name, and what runs it).
	open_action: (title: string, run: { [string]: any }) -> (),
	-- (`at`: where its composer starts instead of the session's place.)
	new_side_chat: ((session: string, at: { project: string?, device: string? }?) -> ())?,
	fork: ((session: string) -> ())?,
	side: ((session: string) -> { any })?,
	subagents: (() -> { any })?,
	open_side: ((session: string, child: any) -> ())?,
	open_subagent: ((session: string, sub: any) -> ())?,
	-- A web address in a browser tab of the session's panel (a port's).
	open_url: ((session: string, url: string) -> ())?,
	showing: () -> boolean,
	-- How far its top stands under the panel's (its tab strip): its block
	-- lines up with a new chat's composer beside it, which stands by the
	-- panel's whole height.
	above: number?,
}

-- ---------------------------------------------------------------------------
-- Where it runs.

local function info(sid: string): Node
	local function chat(): sessions.Chat?
		return chat_of(sid)
	end
	local function device(): sessions.Device?
		local c = chat()
		return if c then device_of(c.device) else nil
	end
	local function project(): sessions.Project?
		local c = chat()
		return if c then project_of(c.project) else nil
	end
	local function branch(): string
		local c = chat()
		return trim(if c then c.branch else nil)
	end
	local function away(): boolean
		local d = device()
		return d ~= nil and not d.here and d.online == false
	end

	-- The branch as plugins read it in its row's slot (`panel_branch`, and
	-- `panel_details` under the panel's details): the session, by its id
	-- alone, while it has a branch. The SDK finds its device, branch and
	-- folder in the plugin's records, as far as its grants reach
	-- ("sessions.read", "files.read"): a place gives ids, never records.
	-- Git puts what the branch adds over its base there. Core says nothing
	-- of git past the branch's name.
	local branch_info = ui.state(`panel_home_branch.{sid}`, nil :: any)
	ui.effect(function()
		local c = chat()
		local now = if c and branch() ~= "" then { session = sid } else nil
		local was = branch_info:peek()
		if ui.json.encode(was) ~= ui.json.encode(now) then
			branch_info:set(now)
		end
	end)

	return ui.column {
		id = "panel-home-info",
		w_full = true,
		gap = "$zeron.list.gap",
		PanelHome.Row {
			id = "panel-home-device",
			lead = ui.presence(function(): string
				return device_icon(device())
			end, function(name: string): Node
				return Icon { name = name :: Icon.Name, size = 16 }
			end),
			label = function(): string
				local d = device()
				return if d then d.name else locale.s("panel.home.unknown_device")
			end,
			trail = ui.show(away, ui.div {
				role = "img",
				label = locale.t("panel.home.away"),
				tooltip = Tooltip { locale.t("panel.home.away") },
				color = "$warning",
				Icon { name = "wifi-off", size = 14 },
			}),
		},
		ui.presence(function(): string
			local p = project()
			return if p then `p:{p.id}` else "none"
		end, function(which: string): Node
			local p = project()
			if which == "none" or not p then
				return PanelHome.Row { id = "panel-home-project", icon = "folder", label = locale.t("panel.home.no_project"), tone = "muted" }
			end
			local id = p.id
			return PanelHome.Row {
				id = "panel-home-project",
				lead = Tile {
					label = function(): string?
						local now = project_of(id)
						return if now then now.name else nil
					end,
					hue = function(): string?
						local l = route.look(id)
						return if l then l.hue else route.hue(id)
					end,
					src = function(): string?
						local l = route.look(id)
						return if l then l.src else nil
					end,
					emoji = function(): string?
						local l = route.look(id)
						return if l then l.emoji else nil
					end,
					size = 16,
					project = id,
				},
				label = function(): string
					local now = project_of(id)
					return if now then now.name else ""
				end,
				tooltip = function(): string
					local now = project_of(id)
					return if now then now.path else ""
				end,
			}
		end, { w_full = true }),
		ui.show(function(): boolean
			return branch() ~= ""
		end, ui.column {
			w_full = true,
			PanelHome.Row {
				id = "panel-home-branch",
				icon = "git-branch",
				label = branch,
				tooltip = branch,
				-- Plugins' part of the row (the Git plugin's +N −M), after its
				-- name: a slot no plugin can offer, under Zeron's own name, given
				-- the branch.
				trail = ui.row {
					id = "panel-home-branch-slot",
					shrink = 0,
					items = "center",
					provide = { panel_branch = branch_info },
					ui.slot("zeron.panel.branch", { direction = "row", items = "center" }),
				},
			},
			-- Plugins' rows of their own under the branch (GitHub's pull
			-- request), given the branch: no room of its own while empty
			-- (a row added brings its own gap over it), so nothing moves.
			ui.column {
				id = "panel-home-details",
				w_full = true,
				provide = { panel_details = branch_info },
				ui.slot("zeron.panel.details", { w_full = true }),
			},
		}),
	}
end

-- ---------------------------------------------------------------------------
-- The project's run actions.

local function actions(props: Props, sid: string): Node
	-- Whose: the session's project, when it's on the session's device.
	local function context(): { space: string, device: string, slot: string }?
		local c = chat_of(sid)
		if not c then
			return nil
		end
		local p = project_of(c.project)
		if not p or p.device ~= c.device then
			return nil
		end
		return { space = p.id, device = c.device, slot = `{c.device}|{p.id}` }
	end
	local ctx = ui.computed(function(): string?
		local c = context()
		return if c then c.slot else nil
	end)
	local function now(): any
		local slot = ctx:get()
		return if slot then actions_by:get()[slot] else nil
	end
	local function load(c: { space: string, device: string, slot: string })
		local was = actions_by:peek()[c.slot]
		actions_by:set_key(c.slot, { state = "loading", snapshot = if was then was.snapshot else nil })
		api.actions(c.space, c.device):after(function(snapshot: any)
			actions_by:set_key(c.slot, { state = "ready", snapshot = snapshot })
		end, function(err: any)
			local m = if type(err) == "table" and err.message then tostring(err.message) else tostring(err)
			local before = actions_by:peek()[c.slot]
			actions_by:set_key(c.slot, {
				state = if m == "unsupported" then "unsupported" else "failed",
				message = m,
				snapshot = if before then before.snapshot else nil,
			})
		end)
	end
	-- Loaded once for a project (kept, as the shipping app's cache);
	-- Retry loads it again.
	ui.effect(function()
		local slot = ctx:get()
		if not slot or actions_by:peek()[slot] then
			return
		end
		local c = context()
		if c then
			load(c)
		end
	end)

	-- The editor: adding (or importing) one, or editing one.
	local editing = ui.use_state(false)
	local deleting = ui.use_state(false)
	local edit_id = ui.use_state(nil :: string?)
	local name = ui.use_state("")
	local command = ui.use_state("")
	local icon = ui.use_state("play")
	local setup = ui.use_state(false)
	local problem = ui.use_state(nil :: string?)
	local saving = ui.use_state(false)
	local function edit(a: any?)
		edit_id:set(if a then a.id else nil)
		name:set(if a then a.name or "" else "")
		command:set(if a then a.command or "" else "")
		icon:set(if a and a.icon then a.icon else "play")
		setup:set(a ~= nil and a.setup == true)
		problem:set(nil)
		saving:set(false)
		editing:set(true)
	end
	local function save()
		local c = context()
		if not c or saving:peek() then
			return
		end
		local n, cmd = trim(name:peek()), trim(command:peek())
		local why: string? = nil
		if n == "" then
			why = "name"
		elseif utf8.len(n) and (utf8.len(n) :: number) > 80 then
			why = "name_long"
		elseif cmd == "" then
			why = "command"
		elseif #cmd > 16 * 1024 then
			why = "command_long"
		end
		if why then
			problem:set(locale.s(`panel.home.editor.err_{why}`))
			return
		end
		saving:set(true)
		problem:set(nil)
		api.save_action(c.space, c.device, { id = edit_id:peek(), name = n, command = cmd, icon = icon:peek(), setup = setup:peek() }):after(function(snapshot: any)
			actions_by:set_key(c.slot, { state = "ready", snapshot = snapshot })
			saving:set(false)
			editing:set(false)
		end, function(err: any)
			saving:set(false)
			problem:set(if type(err) == "table" and err.message then tostring(err.message) else tostring(err))
		end)
	end
	local function delete()
		local c = context()
		local id = edit_id:peek()
		if not c or not id then
			return
		end
		saving:set(true)
		api.delete_action(c.space, c.device, id):after(function(snapshot: any)
			actions_by:set_key(c.slot, { state = "ready", snapshot = snapshot })
			saving:set(false)
			editing:set(false)
		end, function(err: any)
			saving:set(false)
			problem:set(if type(err) == "table" and err.message then tostring(err.message) else tostring(err))
		end)
	end
	local function run(a: any)
		local c = context()
		local s = now()
		if not c or not s or s.state ~= "ready" then
			return
		end
		props.open_action(a.name, { space = c.space, device = c.device, chat = sid, action = a.id })
	end

	-- A row each: the snapshot's actions (keyed by id), then its imports.
	local list = ui.computed(function(): { any }
		local s = now()
		local snap = if s then s.snapshot else nil
		return if snap then snap.actions or {} else {}
	end)
	local imports = ui.computed(function(): { any }
		local s = now()
		local snap = if s then s.snapshot else nil
		return if snap then snap.importable or {} else {}
	end)
	local function can_run(): boolean
		local s = now()
		return s ~= nil and s.state == "ready"
	end
	local function action_row(_a: any, _i: State<number>, a: State<any>): Node
		local gear_ring = PanelHome.inner(6)
		-- Its gear, as the shipping app's: there under the pointer, or with
		-- the keyboard on it.
		local hovered = ui.use_state(false)
		local gear_focused = ui.use_state(false)
		return ui.row {
			on_hover = function(inside: boolean)
				hovered:set(inside)
			end,
			class = "panel-home-action",
			w_full = true,
			h = PanelHome.ROW_H,
			role = "button",
			label = function(): string
				return locale.s("panel.home.run_named", { name = a:get().name })
			end,
			focusable = true,
			cursor = function(): string
				return if can_run() then "pointer" else "default"
			end,
			opacity = function(): number
				return if can_run() then 1 else 0.5
			end,
			px = "$space.2",
			pr = "$space.1",
			gap = "$space.3",
			items = "center",
			rounded = PanelHome.ROW_RADIUS,
			color = "$text.primary",
			hover = { bg = "$hover" },
			focus = require("@ds/control").ring,
			on_click = function()
				run(a:peek())
			end,
			on_key = function(e: any): boolean?
				if (e.key == "Enter" or e.key == " ") and not (e.cmd or e.ctrl or e.alt) then
					run(a:peek())
					return true
				end
				return nil
			end,
			ui.row {
				w = PanelHome.ICON,
				h = PanelHome.ICON,
				shrink = 0,
				items = "center",
				justify = "center",
				color = "$text.secondary",
				ui.presence(function(): string
					return action_icon(a:get().icon)
				end, function(n: string): Node
					return Icon { name = n :: Icon.Name, size = 16 }
				end),
			},
			ui.text {
				function(): string
					local now_a = a:get()
					return if now_a.setup then locale.s("panel.home.setup_suffix", { name = now_a.name }) else now_a.name
				end,
				literal = true,
				grow = 1,
				min_w = 0,
				truncate = true,
				text_size = "$font.size.md",
			},
			ds.IconButton {
				class = "panel-home-edit-action",
				icon = "settings-minimalistic",
				label = function(): string
					return locale.s("panel.home.edit_named", { name = a:get().name })
				end,
				tooltip = locale.s("panel.home.edit_action"),
				tooltip_side = "left",
				size = "md",
				tone = "quiet",
				rounded = gear_ring,
				opacity = function(): number
					return if hovered:get() or gear_focused:get() then 1 else 0
				end,
				on_focus = function()
					gear_focused:set(true)
				end,
				on_blur = function()
					gear_focused:set(false)
				end,
				on_click = function()
					edit(a:peek())
				end,
			},
		}
	end
	local function import_row(_d: any, _i: State<number>, d: State<any>): Node
		return ui.row {
			class = "panel-home-import",
			w_full = true,
			h = PanelHome.ROW_H,
			role = "button",
			label = function(): string
				return locale.s("panel.home.import_named", { name = d:get().name })
			end,
			focusable = true,
			cursor = "pointer",
			px = "$space.2",
			gap = "$space.3",
			items = "center",
			rounded = PanelHome.ROW_RADIUS,
			color = "$text.primary",
			hover = { bg = "$hover" },
			focus = require("@ds/control").ring,
			on_click = function()
				edit({ name = d:peek().name, command = d:peek().command, icon = d:peek().icon, setup = d:peek().setup })
			end,
			on_key = function(e: any): boolean?
				if (e.key == "Enter" or e.key == " ") and not (e.cmd or e.ctrl or e.alt) then
					edit({ name = d:peek().name, command = d:peek().command, icon = d:peek().icon, setup = d:peek().setup })
					return true
				end
				return nil
			end,
			ui.row {
				w = PanelHome.ICON,
				h = PanelHome.ICON,
				shrink = 0,
				items = "center",
				justify = "center",
				color = "$text.tertiary",
				ui.presence(function(): string
					return action_icon(d:get().icon)
				end, function(n: string): Node
					return Icon { name = n :: Icon.Name, size = 16 }
				end),
			},
			ui.text {
				function(): string
					return d:get().name
				end,
				literal = true,
				grow = 1,
				min_w = 0,
				truncate = true,
				text_size = "$font.size.md",
				color = "$text.secondary",
			},
			ds.Badge { locale.t("panel.home.import") },
		}
	end
	local function note(text: () -> string): Node
		return ui.text {
			text,
			literal = true,
			w_full = true,
			px = "$space.2",
			py = "$space.1",
			wrap = true,
			text_size = "$font.size.sm",
			color = "$text.tertiary",
		}
	end

	-- The editor's icon choice: the shipping app's six, one chosen.
	local function icon_choice(n: string): Node
		local function on(): boolean
			return icon:get() == n
		end
		return ui.button {
			class = "panel-home-icon-choice",
			role = "radio",
			label = locale.t(`panel.home.editor.icon_{n}`),
			tooltip = locale.s(`panel.home.editor.icon_{n}`),
			checked = on,
			w = 34,
			h = 34,
			items = "center",
			justify = "center",
			rounded = "$radius.lg",
			cursor = "pointer",
			bg = function(): string
				return if on() then "$zeron.selected.flat.bg" else "#00000000"
			end,
			color = function(): string
				return if on() then "$text.primary" else "$text.secondary"
			end,
			hover = { bg = "$hover", color = "$text.primary" },
			focus = require("@ds/control").ring,
			on_click = function()
				icon:set(n)
			end,
			Icon { name = `action-{n}` :: any, size = 16 },
		}
	end
	local icons: { Node } = {}
	for _, n in ACTION_ICONS do
		table.insert(icons, icon_choice(n))
	end

	local editor = ds.Dialog {
		open = editing,
		size = "md",
		title = function(): string
			return locale.s(if edit_id:get() then "panel.home.editor.edit_title" else "panel.home.editor.add_title")
		end,
		confirm = {
			label = function(): string
				return locale.s(if saving:get() then "panel.home.editor.saving" else "panel.home.editor.save")
			end,
			keep_open = true,
			on_click = save,
		},
		ds.TextField { value = name, label = locale.t("panel.home.editor.name"), w_full = true, autofocus = true },
		ds.TextArea { value = command, label = locale.t("panel.home.editor.command"), rows = 3, max_rows = 8, w_full = true, font = "$font.mono" },
		ui.column {
			w_full = true,
			gap = "$space.2",
			ui.text { locale.t("panel.home.editor.icon"), text_size = "$font.size.sm", weight = "$font.weight.medium", color = "$text.secondary" },
			ui.row { id = "panel-home-icons", role = "radiogroup", label = locale.t("panel.home.editor.icon"), gap = "$space.1", table.unpack(icons) },
		},
		ds.Checkbox { value = setup, label = locale.t("panel.home.editor.setup") },
		ui.show(function(): boolean
			return problem:get() ~= nil
		end, ui.text {
			function(): string
				return problem:get() or ""
			end,
			id = "panel-home-editor-problem",
			literal = true,
			wrap = true,
			text_size = "$font.size.sm",
			color = "$danger",
		}),
		ui.show(function(): boolean
			return edit_id:get() ~= nil
		end, ui.row {
			w_full = true,
			ds.Button {
				locale.t("panel.home.editor.delete"),
				tone = "ghost",
				size = "sm",
				color = "$danger",
				on_click = function()
					deleting:set(true)
				end,
			},
		}),
	}
	local confirm_delete = ds.Dialog {
		open = deleting,
		tone = "danger",
		title = locale.t("panel.home.editor.delete_title") :: any,
		body = function(): string
			return locale.s("panel.home.editor.delete_body", { name = trim(name:get()) })
		end,
		confirm = { label = locale.t("panel.home.editor.delete_confirm") :: any, on_click = delete },
	}

	return ui.show(function(): boolean
		local s = now()
		return ctx:get() ~= nil and not (s and s.state == "unsupported")
	end, ui.column {
		id = "panel-home-actions",
		w_full = true,
		gap = "$zeron.list.gap",
		PanelHome.Heading { label = locale.t("panel.home.actions") },
		ui.show(function(): boolean
			local s = now()
			return (s == nil or s.state == "loading") and #list:get() == 0
		end, note(function(): string
			return locale.s("panel.home.actions_loading")
		end)),
		ui.show(function(): boolean
			local s = now()
			return s ~= nil and s.state == "failed"
		end, ui.column {
			w_full = true,
			gap = "$zeron.list.gap",
			note(function(): string
				local s = now()
				return if s and s.message then s.message else ""
			end),
			PanelHome.Row {
				id = "panel-home-actions-retry",
				icon = "refresh",
				label = locale.t("panel.home.retry"),
				tone = "muted",
				on_click = function()
					local c = context()
					if c then
						load(c)
					end
				end,
			},
		}),
		ui.each(list, action_row, {
			key = function(a: any): string
				return a.id
			end,
			w_full = true,
			gap = "$zeron.list.gap",
		}),
		ui.each(imports, import_row, {
			key = function(d: any): string
				return d.name
			end,
			w_full = true,
			gap = "$zeron.list.gap",
		}),
		ui.show(function(): boolean
			local s = now()
			return s ~= nil and s.snapshot ~= nil and s.snapshot.issue ~= nil
		end, note(function(): string
			local s = now()
			return if s and s.snapshot and s.snapshot.issue then s.snapshot.issue else ""
		end)),
		PanelHome.Row {
			id = "panel-home-add-action",
			icon = "plus",
			label = locale.t("panel.home.add_action"),
			tone = "muted",
			on_click = function()
				edit(nil)
			end,
		},
		ui.div { hook = false, editor, confirm_delete },
	})
end

-- ---------------------------------------------------------------------------
-- The session's lanes.

local function lanes(props: Props, sid: string): Node
	local function fold(part: string): () -> boolean
		return function(): boolean
			return folds:get()[`{sid}/{part}`] ~= true
		end
	end
	local function toggle(part: string): () -> ()
		return function()
			local k = `{sid}/{part}`
			folds:set_key(k, if folds:peek()[k] == true then nil else true)
		end
	end
	-- A group's fold and whether it's shown whole, per session, part and
	-- group (as the sidebar keeps its groups').
	local function group_open(part: string): (key: string) -> boolean
		return function(key: string): boolean
			return folds:get()[`{sid}/{part}/g/{key}`] ~= true
		end
	end
	local function group_toggle(part: string): (key: string) -> ()
		return function(key: string)
			local k = `{sid}/{part}/g/{key}`
			folds:set_key(k, if folds:peek()[k] == true then nil else true)
		end
	end
	local function shown_whole(part: string): (key: string) -> boolean
		return function(key: string): boolean
			return whole:get()[`{sid}/{part}/{key}`] == true
		end
	end
	local function show_whole(part: string): (key: string, on: boolean) -> ()
		return function(key: string, on: boolean)
			whole:set_key(`{sid}/{part}/{key}`, if on then true else nil)
		end
	end
	-- How the sidebar's view groups sessions (its Organize), and the group a
	-- lane would stand in there: its project's (a side chat's is its
	-- session's), its device's, or none (one list).
	local function view(): { [string]: any }
		local v = sidebar_view:get()
		return if type(v) == "table" then v else {}
	end
	-- A lane's project and device as the lists read them (their group,
	-- second line and title's tile, ds/panel_home, as the view shows them).
	local function project_data(id: string?): any
		local p = project_of(id)
		local l = if p then route.look(p.id) else nil
		return if p then { id = p.id, name = p.name, hue = if l then l.hue else route.hue(p.id), src = if l then l.src else nil, emoji = if l then l.emoji else nil } else nil
	end
	local function device_data(id: string?): any
		local d = device_of(id)
		if d then
			return { id = d.id, name = d.name, here = d.here }
		end
		return if id then { id = id, name = locale.s("ds.sidebar.unknown_device") } else nil
	end
	-- A finished lane's state shows until it's seen (screens/panel/
	-- lanes_seen: its tab in front), then its time, as the sidebar's.
	local function showing(id: string, status: string?, at: number?): string?
		if (status == "done" or status == "failed") and not lanes_seen.unseen({ id = id, status = status, at = at }) then
			return nil
		end
		return status
	end
	local side = ui.computed(function(): { any }
		local out = {}
		local parent = chat_of(sid)
		local project = if parent then parent.project else nil
		for _, c in (if props.side then props.side(sid) else {}) do
			table.insert(out, {
				id = c.id,
				title = c.title,
				status = showing(c.id, c.status, c.updated),
				agent = c.agent,
				at = c.updated,
				created = c.created,
				child = c,
				-- (A side chat is its session's project's, on its own device.)
				project = project_data(project),
				device = device_data(c.device),
			})
		end
		return out
	end)
	-- Subagents, as the view sorts sessions (by when each was spawned).
	local subs = ui.computed(function(): { any }
		local s = store.session:get()
		if not s or s.id ~= sid or not props.subagents then
			return {}
		end
		-- (A subagent runs on its session's harness: its mark, where it has
		-- none of its own. And in its session's project, on its device,
		-- always: one list, never grouped.)
		local parent = chat_of(sid)
		local harness = if parent then parent.agent else nil
		local project = project_data(if parent then parent.project else nil)
		local device = device_data(if parent then parent.device else nil)
		local out = {}
		for _, a in props.subagents() do
			table.insert(out, {
				id = a.ref,
				title = a.title,
				status = showing(a.ref, a.status, a.at),
				agent = a.agent or harness,
				at = a.at,
				created = a.at,
				sub = a,
				project = project,
				device = device,
			})
		end
		return out
	end)
	-- A side chat's menu: the sidebar's own session menu (screens/shell/
	-- sidebar `session_menu`), where the sidebar's there.
	local function side_menu(l: any): { any }?
		local sidebar: any = boundary.load("@app/screens/shell/sidebar")
		if sidebar and sidebar.session_menu then
			return sidebar.session_menu(l.id)
		end
		return nil
	end
	-- (`extra`: a part of something else than sessions, ports: its rows'
	-- glyph, always leading.)
	-- A side chat archived from its corner, as the sidebar's rows do.
	local function side_archive(l: any)
		local sidebar: any = boundary.load("@app/screens/shell/sidebar")
		if sidebar and sidebar.archive then
			sidebar.archive(l.id)
		end
	end
	-- A part of lanes' own menu (side chats', subagents'; ds/panel_home
	-- `options`): those at work first, a toggle kept for both; every
	-- finished lane of the session not yet seen marked seen, at once.
	local function unread(): { any }
		local out = {}
		for _, list in { side:get(), subs:get() } do
			for _, l in list do
				-- (Their state shows done or failed only while unseen.)
				if l.status == "done" or l.status == "failed" then
					table.insert(out, { id = l.id, at = l.at })
				end
			end
		end
		return out
	end
	local function lanes_options(): { any }
		local waiting = unread()
		return {
			{
				label = locale.s("panel.home.working_first"),
				icon = "status-working",
				checked = function(): boolean
					return working_first:get()
				end,
				keep_open = true,
				on_select = function()
					working_first:set(not working_first:peek())
				end,
			},
			{ separator = true },
			{
				label = locale.s("panel.home.mark_all_read"),
				icon = "check-circle",
				disabled = #waiting == 0,
				on_select = function()
					lanes_seen.mark_all(waiting)
				end,
			},
		}
	end
	-- A side chats' group's "+": a new side chat of the session, its
	-- composer starting in that group's project and on its device (the
	-- header's groups: a project's, a device's, or both). A device other
	-- than the project's takes that device's project of the same name, else
	-- lets the composer pick one there.
	local function new_in(groups: { any })
		local new_side = props.new_side_chat
		if not new_side then
			return
		end
		local parent = chat_of(sid)
		local project: string? = if parent then parent.project else nil
		local device: string? = nil
		for _, g in groups do
			if g.kind == "project" then
				local id = string.match(g.key, "project:([^/]+)$")
				project = if id == "none" then "none" else id
			elseif g.kind == "device" then
				local id = string.match(g.key, "device:([^/]+)$")
				if id and id ~= "none" then
					device = id
				end
			end
		end
		local p = project_of(project)
		if device and p and p.device ~= device then
			local same: string? = nil
			for _, other in sessions.now:peek().projects do
				if other.device == device and (other.name == p.name or other.path == p.path) then
					same = other.id
				end
			end
			project = same
		elseif not device and p then
			device = p.device
		end
		new_side(sid, { project = project, device = device })
	end
	local function new_label(g: any): string
		if g.kind == "device" then
			return locale.s("panel.home.new_side_on", { name = g.name })
		end
		return locale.s("panel.home.new_side_in", { name = g.name })
	end
	-- (`extra`: a part of something else than sessions, ports: its rows'
	-- glyph, always leading. `flat`: never grouped. `archive`: its rows'
	-- corner action. `tooltip`: its rows'. `lanes_part`: side chats' or
	-- subagents', with their own menu.)
	local function part(id: string, list: string, label: string, key: string, items: () -> { any }, name: (any) -> string, on_open: (any) -> (), menu: ((any) -> { any }?)?, extra: { icon: (any) -> string? }?, flat: boolean?, archive: ((any) -> ())?, tooltip: ((any) -> string)?, lanes_part: boolean?): Node
		return ui.show(
			function(): boolean
				return #items() > 0
			end,
			PanelHome.Lanes {
				id = id,
				list = list,
				label = locale.t(label),
				open = fold(key),
				on_toggle = toggle(key),
				group_open = group_open(key),
				on_group_toggle = group_toggle(key),
				expanded = shown_whole(key),
				on_expand = show_whole(key),
				-- The sidebar's view, whole: as it draws sessions.
				view = view :: any,
				flat = flat,
				on_archive = archive,
				tooltip = tooltip,
				icon = if extra then extra.icon :: any else nil,
				menu = menu,
				items = items,
				name = name,
				on_open = on_open,
				working_first = if lanes_part
					then function(): boolean
						return working_first:get()
					end
					else nil,
				options = if lanes_part then lanes_options :: any else nil,
				-- (Side chats' groups: a new one there.)
				on_new = if id == "panel-home-side" and props.new_side_chat then new_in :: any else nil,
				new_label = new_label :: any,
			}
		)
	end
	-- Its ports: the dev servers running for it (the engine's previews,
	-- watched while the home shows, screens/panel/previews), as the shipping
	-- app's session card lists them (shell/session_info.rs `render_session_ports`):
	-- the process serving each (its command line in its tooltip) and its
	-- port, opening it in a browser tab of the panel; its menu opens it
	-- there or in the default browser, or copies its address. Held as
	-- `Previews.hold` holds them: a test's server, open a moment, never
	-- shows; one closed stays a while, faded.
	ui.effect(function()
		if props.showing() then
			ui.on_cleanup(Previews.use(sid))
		end
	end)
	local held = Previews.hold(function(): string?
		return sid
	end)
	local ports = ui.computed(function(): { any }
		local out = {}
		for _, sv in held() do
			local host = string.match(sv.url or "", "^%a+://([^/]+)") or ""
			local name = if sv.name and sv.name ~= "" then sv.name else host
			local process = if type(sv.process) == "string" and sv.process ~= "" then sv.process else nil
			table.insert(out, {
				id = `port:{sv.id}:{sv.port}`,
				-- (What runs it; else what the engine makes of it, "HTTP server".)
				title = process or name,
				corner = `:{sv.port}`,
				detail = if sv.remote and sv.device and sv.device ~= "" then `{sv.device} · {host}` else host,
				url = sv.url,
				faded = sv.closed == true,
				command = if type(sv.command) == "string" and sv.command ~= "" then sv.command else nil,
				closed = sv.closed == true,
			})
		end
		return out
	end)
	local function port_tip(l: any): string
		local says = l.command or l.url or l.title
		return if l.closed then `{locale.s("panel.home.port_closed")}\n{says}` else says
	end
	local function port_menu(l: any): { any }?
		local url = l.url
		if type(url) ~= "string" or url == "" then
			return nil
		end
		return M.port_menu(url, function()
			if props.open_url then
				props.open_url(sid, url)
			end
		end)
	end
	-- Local previews that can't open now (the engine's proxy): one plain
	-- line, its why only in its tooltip; only while a port's open (one kept
	-- a while, closed, has nothing to open).
	local trouble = ui.computed(function(): string?
		for _, l in ports:get() do
			if not l.closed then
				return Previews.trouble(sid)
			end
		end
		return nil
	end)
	return ui.column {
		id = "panel-home-lanes",
		w_full = true,
		gap = "$space.3",
		ui.column {
			w_full = true,
			gap = "$zeron.list.gap",
			part("panel-home-ports", "panel-home-port-list", "panel.home.ports", "ports", function(): { any }
				return ports:get()
			end, function(l: any): string
				return locale.s("panel.browser.start.open_preview", { name = l.title })
			end, function(l: any)
				if props.open_url and l.url then
					props.open_url(sid, l.url)
				end
			end, port_menu, {
				icon = function(_l: any): string?
					return "global"
				end,
			}, true, nil, port_tip),
			ui.show(function(): boolean
				return trouble:get() ~= nil
			end, ui.row {
				id = "panel-home-ports-trouble",
				role = "status",
				w_full = true,
				h = 24,
				px = "$space.2",
				items = "center",
				tooltip = Tooltip {
					function(): string
						return trouble:get() or ""
					end,
				},
				ui.text {
					locale.t("panel.home.previews_unavailable"),
					grow = 1,
					min_w = 0,
					truncate = true,
					text_size = "$font.size.sm",
					color = "$zeron.tone.warning.ink",
				},
			}),
		},
		part("panel-home-side", "panel-home-side-list", "panel.home.side_chats", "side", function(): { any }
			return side:get()
		end, function(l: any): string
			return locale.s("panel.home.open_side", { title = l.title })
		end, function(l: any)
			if props.open_side then
				props.open_side(sid, l.child)
			end
		end, side_menu, nil, false, side_archive, nil, true),
		part("panel-home-subagents", "panel-home-subagent-list", "panel.home.subagents", "subagents", function(): { any }
			return subs:get()
		end, function(l: any): string
			return locale.s("panel.home.open_subagent", { title = l.title })
		end, function(l: any)
			if props.open_subagent then
				props.open_subagent(sid, l.sub)
			end
		end, function(l: any): { any }?
			-- A subagent's: only opening it (it has no chat of its own to
			-- rename, archive or link to).
			return M.subagent_menu(function()
				if props.open_subagent then
					props.open_subagent(sid, l.sub)
				end
			end)
		end, nil, true, nil, nil, true),
	}
end

-- A port row's menu, on a secondary click: open it in the panel's browser,
-- or in the default browser; copy its address.
function M.port_menu(url: string, open: () -> ()): { any }
	return {
		{ label = locale.s("common.open"), icon = "global", on_select = open },
		{
			label = locale.s("panel.home.port_open_external"),
			icon = "arrow-up-right",
			on_select = function()
				require("@app/screens/panel/browser_api").open_externally(url)
			end,
		},
		{
			label = locale.s("panel.home.port_copy"),
			icon = "copy",
			on_select = function()
				ui.clipboard.write(url)
				ds.toast.show { title = locale.s("panel.home.port_copied"), tone = "success" }
			end,
		},
	}
end

-- A subagent row's menu, on a secondary click: open it.
function M.subagent_menu(open: () -> ()): { any }
	return { { label = locale.s("common.open"), icon = "side-chat", on_select = open } }
end

-- ---------------------------------------------------------------------------
-- The page.

type Choice = { id: string, icon: Icon.Name, label: string, key: string, run: () -> () }

M.Page = ui.component(function(props: Props): Node
	local sid = props.session
	local choices: { Choice } = {
		-- (Browser first, as the shipping app's picker lists them.)
		{ id = "panel-open-browser", icon = "global", label = "panel.empty.browser", key = "B", run = function()
			props.open_kind("browser")
		end },
		{ id = "panel-open-files", icon = "folder-with-files", label = "panel.empty.files", key = "F", run = function()
			props.open_kind("files")
		end },
		{ id = "panel-open-terminal", icon = "terminal", label = "panel.empty.terminal", key = "T", run = function()
			props.open_kind("terminal")
		end },
	}
	-- (A new chat with no project: no folder for a file viewer to show.)
	if props.key == "no-project" then
		table.remove(choices, 2)
	end
	if sid and props.new_side_chat and props.fork then
		local new_side, fork = props.new_side_chat, props.fork
		table.insert(choices, { id = "panel-open-side-chat", icon = "side-chat", label = "panel.empty.side_chat", key = "S", run = function()
			new_side(sid)
		end })
		table.insert(choices, { id = "panel-open-fork", icon = "git-branch", label = "panel.empty.fork", key = "K", run = function()
			fork(sid)
		end })
	end
	local letters: { [string]: () -> () } = {}
	local rows: { Node } = {}
	for _, c in choices do
		letters[c.key] = c.run
		table.insert(rows, PanelHome.Choice { id = c.id, icon = c.icon, label = locale.t(c.label), key = c.key, on_click = c.run })
	end
	-- Its letters heard while it's here (one home at a time: the last made).
	local mine: Live = {
		letters = letters,
		showing = props.showing,
		pass = function(letter: string)
			passed:set({ key = letter, n = passed:peek().n + 1 })
		end,
	}
	live = mine
	ui.on_cleanup(function()
		if live == mine then
			live = nil
		end
	end)

	local parts: { Node } = {}
	if sid then
		table.insert(parts, info(sid))
		table.insert(parts, actions(props, sid))
	end
	table.insert(
		parts,
		ui.column {
			id = "panel-home-open",
			w_full = true,
			gap = "$zeron.list.gap",
			PanelHome.Heading { label = locale.t("panel.home.open") },
			ui.column {
				hook = "panel.empty",
				w_full = true,
				gap = "$zeron.list.gap",
				table.unpack(rows),
			},
		}
	)
	if sid then
		table.insert(parts, lanes(props, sid))
	end

	-- Where it stands: in the panel's middle, its rows no wider than a
	-- list of rows is in ds (`zeron.form.list`: a row's icon, its words and
	-- the key at its end), each row's whole width still its target; and level with a
	-- new chat's composer, by the composer's own rule (screens/chat/dock:
	-- a little over the middle of the room under the title bar, read over
	-- the panel's whole height). What it's in is at least as tall as the
	-- scroll's window (measured) and centres it, the lift as room under it,
	-- never more than the room left: taller than the panel, it starts at the
	-- top and scrolls, nothing pushed out of reach.
	local view = ui.ref()
	local block = ui.ref()
	local room = ui.use_state(0)
	-- (Its middle that far over the scroll's: the composer's offset from the
	-- page's middle, less half the strip over it.)
	local lift = (props.above or 0) / 2 - Dock.hero_offset()
	local function under(): number
		local spare = room:get() - (block.height or 0)
		return ui.raw(math.clamp(2 * lift, 0, math.max(spare, 0)), "the composer's lift, as room under the block")
	end
	return ui.scroll {
		ref = view,
		id = "panel-empty",
		role = "group",
		label = locale.t("panel.home.label"),
		w_full = true,
		grow = 1,
		min_h = 0,
		overscroll = "contain",
		-- (Its edges fade where there's more to scroll to, as the sidebar's
		-- and Settings' lists do.)
		fade = { 24, 24 },
		fade_overflow = true,
		-- (What plugins' rows on it read: the session, and the letters it
		-- passes on.)
		provide = { session = sid, panel_key = passed },
		on_resize = function(size: { w: number, h: number })
			if room:peek() ~= size.h then
				room:set(size.h)
			end
		end,
		ui.column {
			w_full = true,
			min_h = function(): number
				return room:get()
			end,
			justify = "center",
			items = "center",
			pb = under,
			ui.column {
				id = "panel-home-block",
				ref = block,
				w_full = true,
				max_w = ui.theme.get("zeron.form.list") :: number?,
				px = "$space.4",
				py = "$space.6",
				gap = "$space.5",
				table.unpack(parts),
			},
		},
	}
end)

return M
