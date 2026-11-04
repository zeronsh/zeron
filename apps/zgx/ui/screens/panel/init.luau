-- The session page's right panel: tabs, as Zeron's right panel has them
-- (crates/ui/src/shell.rs `render_right_tab_strip`, surface_chrome.rs),
-- each tab a file viewer (the tree and the file, screens/files) or a
-- terminal (screens/panel/terminal). Opening it shows its empty page,
-- which offers what can open here; "+" goes back to it.
--
--   Panel.Panel { project = route.project, session = route.session, left = fn?,
--                 on_last_closed = fn? }
--
-- `on_last_closed`: what closing its last tab does (the shell's
-- `view.set_files(false)` if not given: the panel closes for the session).
--
-- `left`: where the panel's left edge is in the window, if the shell says.
-- Where the title bar's window buttons and controls stand over it (the
-- panel expanded, the sidebar collapsed), its tabs start after them, as
-- the shipping app's do (crates/ui/src/shell/tabs.rs
-- `titlebar_trailing_layout`, its takeover `row_left`).
--
-- `project` and `session` are values, states or functions, followed as
-- they change (the shapes files.Pane takes). Tabs are per session and kept
-- outside the views (screens/panel/state), so switching tabs or sessions,
-- a reload, or a template swapped in finds them as they were; each tab's
-- own state (its file, its terminal) is kept by its kind the same way.
--
-- The strip: a tab each, as wide as its title, its kind's icon, its title
-- (a file's name, "Terminal 2"), at its end a dot while its file has
-- unsaved changes and × under the pointer. A click shows it, a middle
-- click or × closes it, a secondary click has Close, Close Others, Close to
-- the Left and Right; tabs drag to reorder; "+" opens the empty page.
-- ⌘W closes the active tab; ⌃Tab and ⌃⇧Tab step through them.
--
--   Panel.close_focused_tab()    (the app's ⌘W) with the keyboard in the
--                                 panel (a file tab's tree or editor, a
--                                 terminal), closes its active tab, asking
--                                 first as a click on × does: true; else false
--   Panel.open_file(session, path, project?, at?)  a file mentioned in a chat or the
--                                 composer, opened in the session's panel:
--                                 its tab if open already (never twice)
--   Panel.open_url(session, url)   a web link from a chat, in a browser tab
--                                 of the session's panel (the one already
--                                 showing it, else a new one); nil: an
--                                 empty browser tab (screens/panel/browser)
--   Panel.toggle_files()          ⌘E: the panel's Files tab in front, or closed
--   Panel.toggle_terminal()       ⌘J: the same for a terminal
--   Panel.step_tab(by)            ⌃Tab/⌃⇧Tab while the panel has the keyboard
--   Panel.focus_moved()           the window root's `on_focus` calls it, so
--                                 the panel knows the keyboard left it
--
-- Plugins: the panel is `hook:panel`, the strip's end (after "+")
-- `hook:panel.strip`, the empty page's choices `hook:panel.empty`.
-- Closing a tab with unsaved changes asks first, out of their reach.

local ds = require("@ds")
local control = require("@ds/control")
local token = require("@ds/token")
local motion = require("@ds/motion")
local Icon = require("@ds/icon")
local FileIcon = require("@ds/file_icons")
local FadeText = require("@ds/fade_text")
local ContextMenu = require("@ds/context_menu")
local IconButton = require("@ds/icon_button").IconButton

local state = require("@app/screens/panel/state")
-- Tab groups. A panel's tabs can be split into groups laid out side
-- by side and above one another (the tree of screens/shell/split), each
-- with a strip of its own and the tab it shows; the focused one lifted,
-- the others dimmed. Kept with the tabs (screens/panel/state's `groups`,
-- by panel key: `{ tree, groups = { [gid] = { tabs = { id } } }, focus,
-- actives, next }`), where the focus moves with them: a tab opened,
-- whatever opens it, lands in the focused group; one turned to focuses
-- its group. The group "main" is every tab no other group holds. A tab
-- dragged to a group's edge starts a group there; onto a group's strip or
-- its middle, it moves into that group. With one group left, the panel is
-- as it always was.
local panel_groups = state.groups
local tab_dragging = ui.state("panel_tab_dragging", false)
local MAIN = state.MAIN
-- Lanes seen as their tabs come to the front (the panel home's done checks).
require("@app/screens/panel/lanes_seen")
local Files = require("@app/screens/files")
-- The shell's layout (whether a session's panel is expanded), if it's there
-- (a fixture mounts the panel alone).
local shell_view: any = nil
do
	local ok, found = pcall(function()
		return (require :: any)("@app/screens/shell/view")
	end)
	if ok then
		shell_view = found
	end
end

-- A terminal tab's view (its own module, loaded on its own: one that
-- fails to load leaves the panel's file tabs working).
-- What the panel uses of it (screens/panel/terminal's).
type TerminalModule = {
	title: (tab: state.Tab) -> string?,
	closing: (tab: state.Tab, done: (busy: boolean) -> ()) -> (),
	close: (tab: state.Tab) -> (),
	Terminal: (props: { key: string, tab: string, project: any?, session: any? }) -> Node,
}
local Terminal: TerminalModule? = nil
do
	local ok, found = pcall(function(): TerminalModule
		return (require :: any)("@app/screens/panel/terminal")
	end)
	if ok then
		Terminal = found
	else
		print(`zeron-zgx: the panel's terminals didn't load: {found}`)
	end
end

-- A lane's tab (a side chat, a subagent: screens/chat/lane_tab), loaded
-- the same way; the lanes card opens them through `open_lane_tab`.
type LaneModule = { Tab: (props: { tab: any }) -> Node, title: (tab: any) -> string, icon: (tab: any) -> string }
local Lanes: LaneModule? = nil
local lanes_card: any = nil
do
	local ok, found = pcall(function(): LaneModule
		return (require :: any)("@app/screens/chat/lane_tab")
	end)
	if ok then
		Lanes = found
		lanes_card = (require :: any)("@app/screens/chat/lanes")
	else
		print(`zeron-zgx: the panel's side chats didn't load: {found}`)
	end
end

-- A browser tab's view (screens/panel/browser), loaded the same way.
type BrowserModule = {
	title: (tab: state.Tab) -> string?,
	icon: (tab: state.Tab) -> string?,
	has_keyboard: (tab: state.Tab) -> boolean,
	open_find: (tab: state.Tab) -> (),
	close: (tab: state.Tab) -> (),
	Browser: (props: { key: string, tab: string, project: any?, session: any?, open_url: ((url: string) -> ())? }) -> Node,
}
local Browser: BrowserModule? = nil
do
	local ok, found = pcall(function(): BrowserModule
		return (require :: any)("@app/screens/panel/browser")
	end)
	if ok then
		Browser = found
	else
		print(`zeron-zgx: the panel's browser didn't load: {found}`)
	end
end

-- The panel's home, its empty page (screens/panel/home), loaded the same
-- way: one that fails to load leaves the panel's tabs working.
type HomeModule = { Page: (props: any) -> Node, key: (e: any) -> boolean }
local Home: HomeModule? = nil
do
	local ok, found = pcall(function(): HomeModule
		return (require :: any)("@app/screens/panel/home")
	end)
	if ok then
		Home = found
	else
		print(`zeron-zgx: the panel's home didn't load: {found}`)
	end
end

type Tab = state.Tab

local M = {}

-- A key nothing took on its way to the window's root (the shell's
-- `on_key`): the home's letter, while the panel shows its home and no
-- text field has the keyboard. Whether it took it.
function M.home_key(e: any): boolean
	return Home ~= nil and Home.key(e)
end

-- The panel shown now, while it has the keyboard: whose tabs, and how it
-- closes one (its own close, which asks first). One panel shows at a time.
-- The strip's scroll, now and per session (screens/panel/state keeps the
-- tabs themselves).
local strip_offset_now = ui.state("panel_strip_offset", 0)
local strip_offsets = ui.state("panel_strip_offsets", {} :: { [string]: number })

local focus_in: string? = nil
-- (Set as a focus inside the panel passes through it, read as the same
-- focus reaches the window's root.)
local claimed = false
local closer: ((id: string) -> ())? = nil
local shown_key: (() -> string?)? = nil

-- Whether the active tab's terminal has the keyboard (a terminal's focus
-- is its own: its node may not say).
local function terminal_focused(k: string): boolean
	local s = state.of(k)
	local tab = if s.active then state.find(k, s.active) else nil
	if not tab or tab.kind ~= "terminal" then
		return false
	end
	local ok, term = pcall(function(): any
		return (require :: any)("@app/screens/panel/terminal_api").term(tab.id)
	end)
	if not ok or not term then
		return false
	end
	local ok2, focused = pcall(function(): any
		return term.focused
	end)
	return ok2 and focused == true
end

-- The window's root heard a focus (its `on_focus`, the shell's): if it
-- didn't come through the panel, the panel no longer has the keyboard.
function M.focus_moved()
	if not claimed then
		focus_in = nil
	end
	claimed = false
end

-- A press reached the window's root (its `on_press`, the shell's): one
-- that didn't come through the panel (blank page, where focus goes to
-- nothing, which no `on_focus` hears) takes the keyboard from it, so ⌃Tab
-- steps sessions again, as the shipping app's does.
local pressed_in = false
function M.press_moved()
	if not pressed_in then
		focus_in = nil
	end
	pressed_in = false
end

-- The session showing's tabs' key (the shell's route), if there is one.
local function route_key(): string?
	local ok, route = pcall(function(): any
		return (require :: any)("@app/screens/shell/route")
	end)
	if not ok or not route then
		return nil
	end
	local session = if route.session then route.session:peek() else nil
	local project = if route.project then route.project:peek() else nil
	local page = if route.page then route.page:peek() else nil
	return state.key(session, project, page)
end

-- Whether the session showing has its panel open (the shell's).
local function panel_open(): boolean
	local files = if shell_view then shell_view.files else nil
	return files ~= nil and files:peek() == true
end

local function set_panel(on: boolean)
	local set_files = (if shell_view then shell_view.set_files else nil) :: ((boolean) -> ())?
	if set_files then
		set_files(on)
	end
end

-- ⌘E / ⌘J (the shipping app's: Files, the terminal): the panel showing a tab
-- of `kind` in front closes; else it opens on one (the first of that kind,
-- or a new one). Whether it acted.
local function toggle_kind(kind: state.Kind): boolean
	-- (The route's session; the panel showing, where there's no route: a
	-- fixture's.)
	local k = route_key() or (if shown_key then shown_key() else nil)
	if not k then
		return false
	end
	local s = state.of(k)
	local active = if s.active then state.find(k, s.active) else nil
	if panel_open() and active and active.kind == kind then
		set_panel(false)
		return true
	end
	local first: string? = nil
	for _, t in s.tabs do
		if t.kind == kind then
			first = t.id
			break
		end
	end
	if first then
		state.activate(k, first)
	else
		state.open(k, kind)
	end
	set_panel(true)
	return true
end

-- The panel open on a tab of `kind` in front (never closing it): Zeron's
-- `/files` and `/terminal` (the shipping app's add_files_surface,
-- add_terminal_surface). Whether it acted.
local function show_kind(kind: state.Kind): boolean
	local k = route_key() or (if shown_key then shown_key() else nil)
	if not k then
		return false
	end
	local s = state.of(k)
	local active = if s.active then state.find(k, s.active) else nil
	if panel_open() and active and active.kind == kind then
		return true
	end
	return toggle_kind(kind)
end

function M.show_files(): boolean
	return show_kind("files")
end

-- `/terminal` (the shipping app's `add_terminal_surface`): a new terminal
-- each time, the panel opened on it (and its keyboard to it, as any
-- terminal opened).
function M.show_terminal(): boolean
	local k = route_key() or (if shown_key then shown_key() else nil)
	if not k then
		return false
	end
	state.open(k, "terminal")
	set_panel(true)
	return true
end

function M.toggle_files(): boolean
	return toggle_kind("files")
end

function M.toggle_terminal(): boolean
	return toggle_kind("terminal")
end

-- ⌃Tab / ⌃⇧Tab while the panel has the keyboard: the next or previous tab
-- (`by` 1 or -1), round. Whether it acted (else the shell steps sessions).
function M.step_tab(by: number): boolean
	local k = if shown_key then shown_key() else nil
	if not k or (focus_in ~= k and not terminal_focused(k)) then
		return false
	end
	local s = state.of(k)
	local n = #s.tabs
	if n == 0 then
		return false
	end
	local at = 0
	for i, t in s.tabs do
		if t.id == s.active then
			at = i
		end
	end
	local to = ((at - 1 + by) % n) + 1
	state.activate(k, s.tabs[to].id)
	return true
end

-- The app's ⌘F: a browser tab with the keyboard (the panel's focus, or
-- the page's own) opens its find bar: true; else false, and the shell
-- goes on to its own Find.
function M.find_in_browser(): boolean
	local k = if shown_key then shown_key() else nil
	if not k or not Browser then
		return false
	end
	local s = state.of(k)
	local tab = if s.active then state.find(k, s.active) else nil
	if not tab or tab.kind ~= "browser" then
		return false
	end
	if focus_in ~= k and not Browser.has_keyboard(tab) then
		return false
	end
	Browser.open_find(tab)
	return true
end

function M.close_focused_tab(): boolean
	local k = if shown_key then shown_key() else nil
	local close_tab = closer
	if not k or not close_tab then
		return false
	end
	if focus_in ~= k and not terminal_focused(k) then
		return false
	end
	local active = state.of(k).active
	if not active then
		return false
	end
	close_tab(active)
	return true
end

-- The strip where Zeron's is: in the window's top row over the panel (its
-- title strip, 38 with 4 of top pad: crates/ui/src/theme.rs
-- TITLEBAR_HEIGHT, TITLEBAR_TOP_PAD), its tabs centred in what's under the
-- pad, level with the title bar's controls; 8 in from the panel's start,
-- and room kept at its end for the title bar's panel toggle, which stands
-- over it. What isn't a tab moves the window. The tabs look as the design
-- system's (ds/tabs, its `pill` look at `sm`): each as wide as its label,
-- the chosen one on the flat selected wash; a long title fades at
-- LABEL_MAX. (Their end keeps a slot for × and the unsaved dot.)
local CHIP_H = 26
local LABEL_MAX = 140
local SLOT = 18
-- (The panel is a card set in from the window's top by the inset's
-- gap, so its band is that much shorter and its tabs stay level with the
-- title bar's controls; its end keeps room for the panel toggle, now that
-- much nearer the card's edge.)
local INSET_GAP = (ui.theme.get("zeron.inset.gap") :: number?) or 4
local BAND_H = 38 - INSET_GAP
-- (Its tabs centred under the pad: Zeron's 4, less what the card is set
-- down by.)
local BAND_PAD_PT = math.max(0, 4 - INSET_GAP)
local BAND_PAD = ui.raw(BAND_PAD_PT, "level with the title bar's controls, the card set in from the top")
-- (Its tabs as far in from its card's left and right as from its top: the
-- band's room over a tab, which its tabs centre in.)
local STRIP_EDGE = BAND_PAD_PT + (BAND_H - BAND_PAD_PT - 26) / 2
local STRIP_START = ui.raw(STRIP_EDGE, "a strip's tabs as far in from the card's side as from its top")
-- (At the panel's top right, the title bar's panel toggle stands over the
-- strip's end (10 in from the window's right, 24 wide): the strip ends
-- before it, by the title bar's own gap between its controls.)
local TOGGLE_ROOM = 34 - INSET_GAP + 2
local STRIP_FADE = 36
local SLIDE: Motion = { duration = 150, ease = "out(3)" }
-- The title bar's controls' size (the Expand button stands with them).
local CONTROL = ds.island.metrics().control

-- A prop's value now (`control.value`: a function's answer, a state's,
-- tracked, or the value itself); a path's file name.
local read = control.value :: (v: any) -> any
local base_name = require("@app/screens/files/paths").base_name

-- A tab's title and whether it has unsaved changes (tracked).
-- The file tab of session `key` showing `path` (but `except`), if any: no
-- file is open twice in a chat's panel. (A tab not shown yet shows the file
-- it was made with.)
function M.tab_with_file(key: string, path: string, except: string?): string?
	for _, t in state.of(key).tabs do
		if t.kind == "files" and t.id ~= except then
			local shown = Files.tab_file(t.id)
			if (shown or t.path) == path then
				return t.id
			end
		end
	end
	return nil
end

-- Opens `path` in `session`'s panel (a file mentioned in a chat, the
-- composer's): its tab if it's open already; else the file tab showing, by
-- its rules (a clean file replaced, one with unsaved edits kept and this one
-- in a tab of its own); else a new file tab. The panel opens for the session
-- showing. `path` is relative to the session's folder, as the tree's are.
function M.open_file(session: any, path: string, project: any?, at: any?)
	local k = state.key(session, project)
	if not k then
		return
	end
	local other = M.tab_with_file(k, path)
	local s = state.of(k)
	local active = if s.active then state.find(k, s.active) else nil
	local shown: string
	if other then
		state.activate(k, other)
		shown = other
	elseif active and active.kind == "files" and Files.show_in_tab(active.id, path) then
		-- (Its own rules decide: see screens/files.)
		shown = active.id
	else
		shown = state.open(k, "files", { path = path })
	end
	-- At a line (`path:12`): its editor goes there once it shows (a number,
	-- or `{ line, column?, end_line? }`).
	if at ~= nil and Files.show_line then
		Files.show_line(shown, path, at)
	end
	local set_files = (if shell_view then shell_view.set_files else nil) :: ((boolean) -> ())?
	if set_files then
		set_files(true)
	end
end

-- A web link (a chat's, with "Open links in Zeron" on) in `session`'s
-- panel: the browser tab already showing it, else a new one; nil opens an
-- empty browser tab. The panel opens for the session showing (or, with
-- `page`, a plugin page's own panel). Whether a tab took it (a session or a
-- page must be open).
function M.open_url(session: any, url: string?, project: any?, page: any?): boolean
	local k = state.key(session, project, page)
	if not k then
		return false
	end
	local found: string? = nil
	if url then
		for _, t in state.of(k).tabs do
			if t.kind == "browser" and t.url == url then
				found = t.id
			end
		end
	end
	if found then
		state.activate(k, found)
	else
		state.open(k, "browser", { url = url })
	end
	local set_files = (if shell_view then shell_view.set_files else nil) :: ((boolean) -> ())?
	if set_files then
		set_files(true)
	end
	return true
end

-- A plugin's tab in `session`'s panel (screens/plugins, asked by the plugin
-- with "panel.tabs"): the one already showing that plugin's kind and title,
-- else a new tab. The panel opens for the session showing.
function M.open_plugin_tab(session: any, tab: { [string]: any })
	-- (On its own page, the page's panel: a plugin's own pages only.)
	local page = tab.page
	if type(page) == "table" and page.plugin ~= tab.plugin then
		return
	end
	local k = state.key(session, nil, page)
	if not k then
		return
	end
	-- The one already there: by `data.key` when the plugin gives one (a
	-- commit, whose subject two may share), else by its title.
	local key = if type(tab.data) == "table" then tab.data.key else nil
	local found: string? = nil
	for _, t in state.of(k).tabs do
		if t.kind == "plugin" and t.plugin == tab.plugin and t.plugin_kind == tab.kind then
			local same = if key ~= nil then type(t.data) == "table" and t.data.key == key else t.title == tab.title
			if same then
				found = t.id
			end
		end
	end
	if found then
		state.activate(k, found)
	else
		state.open(k, "plugin", { plugin = tab.plugin, plugin_kind = tab.kind, title = tab.title, icon = tab.icon, data = tab.data })
	end
	local set_files = (if shell_view then shell_view.set_files else nil) :: ((boolean) -> ())?
	if set_files then
		set_files(true)
	end
end

-- A lane (a side chat, a subagent) as a tab of `session`'s panel: the one
-- already showing it, else a new one, and the panel open.
function M.open_lane_tab(session: string, lane: { kind: string, id: string, title: string, data: any? })
	local k = state.key(session, nil)
	if not k then
		return
	end
	local found: string? = nil
	for _, t in state.of(k).tabs do
		if t.kind == lane.kind and t.chat == lane.id then
			found = t.id
		end
	end
	if found then
		state.activate(k, found)
	else
		state.open(k, lane.kind :: any, { chat = lane.id, title = lane.title, data = lane.data })
	end
	local set_files = (if shell_view then shell_view.set_files else nil) :: ((boolean) -> ())?
	if set_files then
		set_files(true)
	end
end
if lanes_card then
	lanes_card.opener = M.open_lane_tab
end

-- What a tab's menu has after Close and its kin: a side chat's, the
-- sidebar's menu for its chat (rename, copy, archive, delete), once it's
-- made; none for the rest (a subagent's has nothing of its own to do).
function M.tab_menu(tab: Tab): { any }
	if tab.kind ~= "side_chat" or not tab.chat or not lanes_card then
		return {}
	end
	if lanes_card.unsent:peek()[tab.chat] ~= nil or lanes_card.forks:peek()[tab.chat] ~= nil then
		return {}
	end
	local sidebar: any = (require :: any)("@app/screens/shell/boundary").load("@app/screens/shell/sidebar")
	local more = if sidebar and sidebar.session_menu then sidebar.session_menu(tab.chat) else nil
	return more or {}
end

local function title_of(tab: Tab): (string, boolean)
	if (tab.kind == "side_chat" or tab.kind == "subagent") and Lanes then
		return Lanes.title(tab), false
	end
	if tab.kind == "plugin" then
		return tab.title or ui.t("panel.tab.plugin"):get(), false
	end
	if tab.kind == "browser" then
		local t = if Browser then Browser.title(tab) else nil
		return if t and t ~= "" then t else ui.t("panel.tab.browser"):get(), false
	end
	if tab.kind == "files" then
		local path, dirty = Files.tab_file(tab.id)
		return if path then base_name(path) else ui.t("panel.tab.files"):get(), dirty
	end
	-- (A run action's: its name, as the shipping app titles it.)
	if tab.title and tab.title ~= "" then
		return tab.title, false
	end
	if Terminal and Terminal.title then
		local t = Terminal.title(tab)
		if t and t ~= "" then
			return t, false
		end
	end
	return ui.t("panel.tab.terminal", { n = tab.n or 1 }):get(), false
end

-- `hold`: how far left of the panel's moving edge its strip stands (0 or
-- less): the shell holds the strip where it stands open as the panel slides,
-- its edge clipping it, so the tabs never pass under the title bar's fixed
-- controls at the window's right (as Zeron's pane header is revealed in
-- place).
-- `docked`: whether the window has room for a file tab's tree beside its
-- file (the shell's fit, as Zeron's Files column); not given, each file tab
-- decides by its own width.
export type Props = { project: any?, session: any?, page: any?, left: (() -> number)?, hold: (() -> number)?, on_last_closed: (() -> ())?, docked: (() -> boolean)?, [any]: any }

-- Where the title bar's controls end, from the window's left (ds/title_bar).
local CONTROLS_END = ds.TitleBar.controls_end()
local STRIP_START_PT = STRIP_EDGE

M.Panel = ui.component(function(props: Props): Node
	local project, session, page = props.project, props.session, props.page
	local left_of = props.left
	local hold = props.hold
	if project == nil and session == nil then
		local route = require("@app/screens/shell/route") :: any
		project, session, page = route.project, route.session, route.page
	end
	-- Whose tabs: the session's (its project's with none open), or a
	-- plugin's page's.
	local key = ui.computed(function(): string?
		return state.key(read(session), read(project), read(page))
	end)
	local function tabs(): { Tab }
		return state.of(key:get()).tabs
	end
	-- (The strip's rows, keyed by tab: each keeps its node as others come,
	-- go and move.)
	local tab_list = ui.computed(function(): { Tab }
		return tabs()
	end)
	local function active(): string?
		return state.of(key:get()).active
	end

	-- Closing: a file tab with unsaved changes asks first; a terminal with
	-- something running asks too (its kind says).
	local asking = ui.use_state(false)
	local asked = ui.use_state(nil :: { key: string, id: string, title: string, why: string }?)
	ui.effect(function()
		if not asking:get() and asked:peek() then
			asked:set(nil)
		end
	end)
	local function close_now(k: string, id: string)
		local tab = state.find(k, id)
		state.close(k, id)
		if tab and tab.kind == "terminal" and Terminal and Terminal.close then
			Terminal.close(tab)
		elseif tab and tab.kind == "browser" and Browser then
			Browser.close(tab)
		elseif tab and (tab.kind == "side_chat" or tab.kind == "subagent") and lanes_card then
			-- Its chat let go (a reload never does this).
			lanes_card.watch(tab.id, nil)
		elseif tab and tab.kind == "files" then
			Files.close_tab(id)
		end
		-- Its last tab gone, the panel closes for this session (as the
		-- shipping app's `collapse_surfaces_if_empty`), leaving expanded with
		-- it; opened again, it shows its empty page. Only if Settings →
		-- General says so (off by default: it stays open on its home).
		local close_with_last = require("@app/screens/settings/prefs").close_panel_with_last_tab:peek() == true
		if tab and close_with_last and #state.of(k).tabs == 0 then
			local on_last = props.on_last_closed
			if on_last then
				on_last()
			else
				local set_files = (if shell_view then shell_view.set_files else nil) :: ((boolean) -> ())?
				if set_files then
					set_files(false)
				end
			end
		end
	end
	local function close(id: string)
		local k = key:peek()
		if not k then
			return
		end
		local tab = state.find(k, id)
		if not tab then
			return
		end
		local title, dirty = title_of(tab)
		if dirty then
			asked:set({ key = k, id = id, title = title, why = "unsaved" })
			asking:set(true)
		elseif tab.kind == "terminal" and Terminal and Terminal.closing then
			-- (A program running in it: the host says, then it asks.)
			Terminal.closing(tab, function(busy: boolean)
				if busy then
					asked:set({ key = k, id = id, title = title, why = "running" })
					asking:set(true)
				else
					close_now(k, id)
				end
			end)
		else
			close_now(k, id)
		end
	end
	local function close_many(ids: { string })
		for _, id in ids do
			close(id)
		end
	end

	-- Each tab's box, by id (for bringing the active one into view).
	local chip_refs: { [string]: any } = {}

	-- A tab in the strip.
	local function chip(tab: Tab, _i: number): Node
		local id = tab.id
		local hovered = ui.use_state(false)
		local ref = ui.ref()
		chip_refs[id] = ref
		local function is_active(): boolean
			return active() == id
		end
		local function title(): string
			local t = title_of(tab)
			return t
		end
		local function dirty(): boolean
			local _, d = title_of(tab)
			return d
		end
		-- Its icon, made afresh wherever it shows (the tab, its drag preview:
		-- a node lives in one place).
		local function make_icon(): Node
			return if tab.kind == "terminal"
			then Icon { name = "terminal", size = 14 }
			elseif tab.kind == "browser" then ui.presence(function(): string
					-- (The page's icon once the host has it, else the globe: a
					-- real swap per icon.)
					return if Browser then Browser.icon(tab) or "" else ""
				end, function(path: string): Node
					if path == "" then
						return Icon { name = "global", size = 14 }
					end
					return ui.image { src = path, w = 14, h = 14, fit = "contain", rounded = "$radius.sm" }
				end)
			elseif tab.kind == "plugin" then Icon { name = (tab.icon or "widget") :: any, size = 14 }
			elseif (tab.kind == "side_chat" or tab.kind == "subagent") and Lanes then Icon { name = Lanes.icon(tab) :: any, size = 14 }
			else ui.presence(function(): string
				-- (A file tab's file by its name; none yet, "": by the file,
				-- not the words, which follow the language.)
				if tab.kind == "files" then
					local path = Files.tab_file(tab.id)
					return if path then base_name(path) else ""
				end
				return title()
			end, function(name: string): Node
				-- (A file's own icon, by its name: a real swap per file.)
				if name == "" then
					return Icon { name = "folder-with-files", size = 14 }
				end
				return FileIcon { name = name, size = 14 }
			end)
		end
		local icon = make_icon()
		-- Being dragged: the tab left behind dims, its preview under the
		-- pointer (the tab itself, chosen, on the menus' glass).
		local dragging = ui.use_state(false)
		local function others(side: string?): { string }
			local out: { string } = {}
			local list = tabs()
			local me = 0
			for i, t in list do
				if t.id == id then
					me = i
				end
			end
			for i, t in list do
				if t.id ~= id and (side == nil or (side == "left" and i < me) or (side == "right" and i > me)) then
					table.insert(out, t.id)
				end
			end
			return out
		end
		return ContextMenu {
			label = ui.t("panel.tab.actions"),
			ring = "$radius.md",
			shrink = 0,
			items = function(): { any }
				local entries: { any } = {
					-- (Strings: the menu types ahead through them.)
					{ label = ui.t("panel.tab.close"):get(), icon = "close", shortcut = "cmd+w", on_select = function()
						close(id)
					end },
					{ label = ui.t("panel.tab.close_others"):get(), on_select = function()
						close_many(others(nil))
					end },
					{ label = ui.t("panel.tab.close_left"):get(), on_select = function()
						close_many(others("left"))
					end },
					{ label = ui.t("panel.tab.close_right"):get(), on_select = function()
						close_many(others("right"))
					end },
				}
				local more = M.tab_menu(tab)
				if #more > 0 then
					table.insert(entries, { separator = true })
					for _, e in more do
						table.insert(entries, e)
					end
				end
				return entries
			end,
			ui.row {
				id = `panel-tab-{id}`,
				ref = ref,
				-- (What plugins place by, and what's under them: the tab, and a
				-- file tab's file, as a token.)
				class = if tab.kind == "files" then "panel-tab file-tab" else "panel-tab",
				provide = if tab.kind == "files"
					then {
						panel = id,
						file = ui.computed(function(): string?
							return token.file(Files.tab_file(id) or tab.path)
						end),
					}
					else { panel = id },
				role = "tab",
				label = title,
				selected = is_active,
				h = CHIP_H,
				shrink = 0,
				pl = "$space.3",
				-- (The end slot sits as far in as it is from top and bottom.)
				pr = "$space.1",
				gap = "$zeron.space.tight",
				items = "center",
				rounded = "$radius.lg",
				cursor = "pointer",
				-- (As ds/tabs' pill: the chosen one on the flat wash; the
				-- others take the hover wash under the pointer, their ink
				-- brightening with it.)
				bg = function(): string
					if is_active() then
						return "$zeron.selected.flat.bg"
					end
					return if hovered:get() then "$hover" else "#00000000"
				end,
				color = function(): string
					return if is_active() or hovered:get() then "$text.primary" else "$text.secondary"
				end,
				on_hover = function(inside: boolean)
					hovered:set(inside)
				end,
				on_click = function()
					local k = key:peek()
					if k then
						state.activate(k, id)
					end
				end,
				on_aux_click = function(e)
					if e.button == "middle" then
						close(id)
					end
				end,
				-- Reordered by dragging onto another tab: the tab itself under
				-- the pointer, as it looks chosen, floating on the menus' glass;
				-- the one left behind faint until it lands.
				on_drag = function(): any
					dragging:set(true)
					-- (The panel's panes show where it can land.)
					tab_dragging:set(true)
					return { panel_tab = id }
				end,
				drag_preview = function()
					local card: { [any]: any } = {
						h = CHIP_H,
						pl = "$space.3",
						pr = "$space.3",
						gap = "$zeron.space.tight",
						items = "center",
						rounded = "$radius.lg",
						color = "$text.primary",
						ui.div { y = 0.5, shrink = 0, make_icon() },
						FadeText {
							title,
							max_w = LABEL_MAX,
							text_size = "$font.size.sm",
							weight = "$font.weight.medium",
						},
					}
					require("@ds/glass").style(card, "menu")
					return ui.row(card)
				end,
				on_drag_end = function()
					dragging:set(false)
					tab_dragging:set(false)
				end,
				opacity = function(): number
					return if dragging:get() then 0.4 else 1
				end,
				drop_when = function(payload: any): boolean
					return type(payload) == "table" and payload.panel_tab ~= nil and payload.panel_tab ~= id
				end,
				-- The dragged tab takes this one's place as the pointer crosses
				-- its middle (from either side), the strip sliding to make room
				-- (its layout animation): where it shows is where it lands.
				on_drag_over = function(payload: any, e)
					local k = key:peek()
					if not k then
						return
					end
					local from, here = nil, nil
					for i, t in tabs() do
						if t.id == payload.panel_tab then
							from = i
						elseif t.id == id then
							here = i
						end
					end
					if not from or not here then
						return
					end
					local past_middle = e.local_x > e.w / 2
					if (from < here and past_middle) or (from > here and not past_middle) then
						state.move(k, payload.panel_tab, here)
					end
				end,
				-- (Dropped: it's where it was dragged to already.)
				on_drop = function(_payload: any) end,
				-- (Half a point down: on the label's lowercase middle, not
				-- its line box's.)
				ui.div { y = 0.5, shrink = 0, icon },
				FadeText {
					title,
					max_w = LABEL_MAX,
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
				},
				-- Its end: × under the pointer, a dot while unsaved.
				ui.row {
					w = SLOT,
					h = SLOT,
					shrink = 0,
					items = "center",
					justify = "center",
					ui.show(function(): boolean
						return hovered:get()
					end, IconButton {
						label = ui.t("panel.tab.close"),
						-- zgx-lint: allow(untracked-get)
						tooltip = ui.t("panel.tab.close_tooltip", { key = ui.shortcut_text("cmd+w") }):get(),
						tooltip_side = "bottom",
						size = "sm",
						tone = "quiet",
						w = SLOT,
						h = SLOT,
						rounded = "$radius.sm",
						on_click = function()
							close(id)
						end,
						Icon { name = "close", size = 12 },
					}),
					ui.show(function(): boolean
						return dirty() and not hovered:get()
					end, ui.div {
						role = "img",
						label = ui.t("panel.tab.unsaved"),
						w = 6,
						h = 6,
						rounded = "$radius.full",
						bg = "$accent",
					}),
				},
			},
		}
	end

	-- How far the strip is scrolled: kept outside the view, per session, so
	-- a reload or a switch back finds it where it was.
	local strip_offset = strip_offset_now
	local strip_for = key:peek()
	-- (Where it was as this view is made: a scroll made before its tabs
	-- are laid out clamps its offset to 0, so it's put back once they are.)
	local strip_wanted: number? = strip_offset:peek()
	ui.effect(function()
		local k = key:get()
		if k ~= strip_for then
			strip_for = k
			strip_wanted = if k then strip_offsets:peek()[k] or 0 else 0
			strip_offset:set(strip_wanted :: number)
		end
	end)
	ui.effect(function()
		local v = strip_offset:get()
		local k = key:peek()
		if k and strip_offsets:peek()[k] ~= v then
			strip_offsets:set_key(k, v)
		end
	end)
	-- The active tab in view: a new one opens at the end, past the edge.
	-- (Once every tab is measured: a rebuilt strip's tabs are 0 wide for a
	-- moment, and would bring the wrong place into view.)
	local strip_ref = ui.ref()
	-- (What's in the strip, followed as it grows: a new tab slides in, and
	-- the end it scrolls to moves with it.)
	local strip_content = ui.ref()
	ui.effect(function()
		local a = active()
		local list = tabs()
		local _ = strip_content.width
		local view = strip_ref.width or 0
		if not a or view <= 0 then
			return
		end
		for _, t in list do
			local r = chip_refs[t.id]
			if not r or (r.width or 0) <= 0 then
				return
			end
		end
		if strip_wanted then
			local w = strip_wanted
			strip_wanted = nil
			strip_offset:set(w)
			return
		end
		local left = 0
		-- (The tabs' own gap, the strip's hair.)
		local gap = (ui.theme.get("zeron.space.hair") :: number?) or 2
		for i, t in list do
			local r = chip_refs[t.id]
			local w = if r then r.width or 0 else 0
			if t.id ~= a then
				left += w + gap
			else
				local right = left + w
				local off = strip_offset:peek()
				if left < off then
					strip_offset:set(left)
				elseif i == #list then
					-- (The last: to the end, "+" after it in view too; the
					-- offset is clamped to what there is.)
					strip_offset:set(1e6)
				elseif right > off + view then
					strip_offset:set(right - view)
				end
				return
			end
		end
	end)
	-- "+": a new tab (the empty page).
	local function new_tab_button(): Node
		return IconButton {
			label = ui.t("panel.strip.new_tab"),
			tooltip_side = "bottom",
			size = "sm",
			tone = "quiet",
			w = CHIP_H,
			h = CHIP_H,
			rounded = "$radius.lg",
			on_click = function()
				local k = key:peek()
				if k then
					state.activate(k, nil)
				end
			end,
			Icon { name = "plus", size = 14 },
		}
	end
	-- Whether the tabs run past the strip, so "+" stands pinned at its end
	-- (left of Expand) rather than scrolled away after them. From the
	-- tabs' own widths, not the content's ("+" in it or not would flip it
	-- back and forth); pinned, the scroll narrows by "+", which keeps it
	-- pinned until the tabs fit that narrower room too.
	local new_pinned = ui.computed(function(): boolean
		local list = tabs()
		if #list == 0 then
			return false
		end
		local view = strip_ref.width or 0
		if view <= 0 then
			return false
		end
		local gap = (ui.theme.get("zeron.space.hair") :: number?) or 2
		local w = 0
		for _, t in list do
			local r = chip_refs[t.id]
			w += (if r then r.width or 0 else 0) + gap
		end
		return w + CHIP_H > view + 0.5
	end)
	local strip = ui.row {
		id = "panel-strip",
		role = "tablist",
		label = ui.t("panel.strip.label"),
		w_full = true,
		h = BAND_H,
		shrink = 0,
		pt = BAND_PAD,
		-- (Clear of the title bar's controls when they stand over it; its
		-- own start otherwise. It follows the edge as it moves, so expanding
		-- and back glide with it.)
		-- (Held where it stands open as the panel slides: see `hold`.)
		x = hold,
		pl = if left_of
			then function(): number
				local left = left_of() + (if hold then hold() else 0)
				return ui.raw(math.max(STRIP_START_PT, CONTROLS_END - left), "clear of the title bar's controls, as its edge moves")
			end
			else STRIP_START,
		-- (And the window's own buttons, where they stand at the right.)
		pr = function(): number
			return ui.raw(TOGGLE_ROOM + ds.TitleBar.window_buttons_right(), "the title bar's panel toggle, and the window's buttons, stand over the strip's end")
		end,
		items = "center",
		window_drag = true,
		-- (No hairline under it: the strip and what's under it are one
		-- surface.)
		-- (The band's room: a scroll sized by nothing else is 320 wide.
		-- Its empty part still moves the window, as nothing there takes a
		-- press.)
		ui.scroll_x {
			ref = strip_ref,
			offset = strip_offset,
			grow = 1,
			min_w = 0,
			h = CHIP_H,
			-- (Each edge fades only as far as tabs run past it: none at the
			-- start, none when they all fit.)
			fade_x = { STRIP_FADE, STRIP_FADE },
			fade_overflow = true,
			ui.row {
				ref = strip_content,
				gap = "$zeron.space.hair",
				items = "center",
				ui.each(tab_list, chip, {
					key = function(t: Tab): string
						return t.id
					end,
					direction = "row",
					gap = "$zeron.space.hair",
					animate = motion.layout(SLIDE),
				}),
				-- (Only with a tab open, as Zeron's: the empty page is what it
				-- opens. Here, after the last tab, while they all fit; pinned at
				-- the strip's end, outside the scroll, once they don't.)
				ui.show(function(): boolean
					return #tabs() > 0 and not new_pinned:get()
				end, new_tab_button()),
				ui.row { hook = "panel.strip", items = "center", gap = "$space.1" },
			},
		},
		-- "+", pinned at the strip's end while the tabs run past it.
		ui.show(new_pinned, ui.div { shrink = 0, ml = "$space.1", new_tab_button() }),
		-- Expand: the panel over the session's chat, and back (the shipping
		-- app's, after the tabs, before the title bar's toggle; no key of
		-- its own there either). The shell keeps whether it is, per session.
		if shell_view and shell_view.toggle_expanded
			then ui.div {
				shrink = 0,
				ml = "$space.1",
				-- (Its name, glyph and tooltip swap as it flips: two buttons.)
				ui.presence(function(): string
					return if shell_view.expanded:get() then "collapse" else "expand"
				end, function(which: string): Node
					local name = if which == "collapse" then ui.t("panel.strip.collapse") else ui.t("panel.strip.expand")
					return IconButton {
						id = "panel-expand",
						label = name,
						tooltip_side = "bottom",
						w = CONTROL,
						h = CONTROL,
						on_click = function()
							shell_view.toggle_expanded()
						end,
						Icon { name = if which == "collapse" then "collapse-arrows" else "expand-arrows", size = 16 },
					}
				end),
			}
			else nil,
	}

	-- The empty page: the session's home (screens/panel/home): where it
	-- runs, its project's actions, what can open here (each with its key,
	-- heard wherever the keyboard is but in a text field: `M.home_key`), and
	-- its side chats and subagents.
	local function open_kind(kind: state.Kind)
		local k = key:peek()
		if k then
			state.open(k, kind)
		end
	end
	local function empty_page(k: string, visible: () -> boolean): Node
		if not Home then
			return ds.EmptyState {
				icon = "split-columns",
				size = "sm",
				fill = true,
				title = ui.t("panel.nothing.title") :: any,
				body = ui.t("panel.nothing.body") :: any,
			}
		end
		local sid = if string.sub(k, 1, 5) == "chat:" then string.sub(k, 6) else nil
		return Home.Page {
			key = k,
			session = sid,
			showing = visible,
			open_kind = function(kind: string)
				open_kind(kind :: state.Kind)
			end,
			open_plugin_tab = function(session_id: string, tab: { [string]: any })
				M.open_plugin_tab(session_id, tab)
			end,
			-- A run action: a terminal tab of its own, titled by it, opening
			-- its terminal as it shows (screens/panel/terminal).
			open_action = function(title: string, run: { [string]: any })
				state.open(k, "terminal", { title = title, run = run })
			end,
			new_side_chat = if lanes_card then lanes_card.new_side_chat else nil,
			fork = if lanes_card then lanes_card.fork else nil,
			side = if lanes_card then lanes_card.side else nil,
			subagents = if lanes_card then lanes_card.subagents else nil,
			open_side = if lanes_card
				then function(session_id: string, child: any)
					lanes_card.open(session_id, { kind = "side_chat", id = child.id, title = child.title })
				end
				else nil,
			open_subagent = if lanes_card then lanes_card.open_subagent else nil,
			open_url = function(session_id: string, url: string)
				M.open_url(session_id, url)
			end,
		}
	end

	-- A tab's body: its kind's view, keyed by the tab (a real swap: another
	-- tab is another view; each keeps its state outside, so coming back
	-- finds it as it was).
	local function body(k: string, tab: Tab): Node
		if (tab.kind == "side_chat" or tab.kind == "subagent") and Lanes then
			return Lanes.Tab { tab = tab }
		end
		if tab.kind == "plugin" then
			-- What its plugin fills, by selector (`.plugin-tab`): until then,
			-- or with its plugin off, a quiet word. What's around it gives
			-- the tab, so the plugin's node reads it in its place.
			local plugin = tab.plugin or ""
			return ui.column {
				grow = 1,
				w_full = true,
				provide = { tab = { kind = tab.plugin_kind, title = tab.title, data = tab.data } },
				ui.column {
					class = `plugin-tab plugin-{plugin}`,
					grow = 1,
					w_full = true,
					ds.EmptyState {
						icon = "widget",
						size = "sm",
						fill = true,
						title = (tab.title or ui.t("panel.tab.plugin")) :: any,
						body = ui.t("panel.plugin_off") :: any,
					},
				},
			}
		end
		if tab.kind == "browser" then
			if Browser then
				return Browser.Browser {
					key = k,
					tab = tab.id,
					project = project,
					session = session,
					-- A page the tab's page opens in a new window: a tab of
					-- its own, beside it.
					open_url = function(url: string)
						state.open(k, "browser", { url = url })
					end,
				}
			end
			return ds.EmptyState {
				icon = "global",
				size = "sm",
				fill = true,
				title = ui.t("panel.broken.browser.title") :: any,
				body = ui.t("panel.broken.browser.body") :: any,
			}
		end
		if tab.kind == "files" then
			return Files.Tab {
				project = project,
				session = session,
				view = tab.id,
				initial = tab.path,
				docked = props.docked,
				-- (Said with the code: Tab indents there.)
				-- (A string, read as the tab is made.)
				escape_hint = ui.t("panel.escape_hint"):get(),
				-- (A file open in another of this chat's tabs: that tab.)
				open_elsewhere = function(path: string): boolean
					local other = M.tab_with_file(k, path, tab.id)
					if other then
						state.activate(k, other)
						return true
					end
					return false
				end,
				-- A tab of its own (Open in New Tab, or picked past unsaved
				-- edits): the one already showing it, if there is one.
				on_open_in_new_tab = function(path: string)
					local other = M.tab_with_file(k, path)
					if other then
						state.activate(k, other)
					else
						state.open(k, "files", { path = path })
					end
				end,
			}
		end
		if Terminal then
			return Terminal.Terminal { key = k, tab = tab.id, project = project, session = session }
		end
		return ds.EmptyState {
			icon = "terminal",
			size = "sm",
			fill = true,
			title = ui.t("panel.broken.terminals.title") :: any,
			body = ui.t("panel.broken.terminals.body") :: any,
		}
	end

	-- Closed all the way (no width), what's in it isn't there for Tab or
	-- assistive technology (`hidden`): they would otherwise reach its tabs
	-- and choices where nothing shows. It stays built, though: opening
	-- again shows it as it was, at once, nothing read or fetched again (a
	-- pull request's long description, its pictures). While it eases shut
	-- it keeps its width, and shows.
	local own = ui.ref()
	local measured: { width: number? }? = nil
	local files = if shell_view then shell_view.files else nil
	local function showing(): boolean
		if files and files:get() == true then
			return true
		end
		local w = if measured then measured.width else nil
		return w == nil or w > 0.5
	end
	local function closed(): boolean
		return not showing()
	end

	local content = ui.presence(function(): string
		local k = key:get()
		if not k then
			return "none"
		end
		local a = active()
		return if a then `{k}\0{a}` else `{k}\0empty`
	end, function(which: string): Node
		local k = key:peek()
		if which == "none" or not k then
			return ds.EmptyState {
				icon = "split-columns",
				size = "sm",
				fill = true,
				title = ui.t("panel.nothing.title") :: any,
				body = ui.t("panel.nothing.body") :: any,
			}
		end
		local a = state.of(k).active
		local tab = if a then state.find(k, a) else nil
		if not tab then
			return empty_page(k, showing)
		end
		local ok, built = pcall(body, k, tab)
		if ok then
			return built
		end
		print(`zeron-zgx: a panel tab didn't build: {built}`)
		return ds.EmptyState {
			icon = "danger-triangle",
			size = "sm",
			fill = true,
			title = ui.t("panel.broken.tab.title") :: any,
			body = ui.t("panel.broken.tab.body") :: any,
		}
	end, { w_full = true, grow = 1, min_h = 0, hidden = closed })

	-- Tab groups (`panel_groups` above).
	local Split = require("@app/screens/shell/split") :: any
	local tree = Split.tree

	-- The groups as they stand for the panel's key, its tabs as they are
	-- now: { tree, lists = { [gid] = { id } }, focus, actives }; nil while
	-- there's one group (the panel as it always was).
	local groups_now = ui.computed(function(): any
		local k = key:get()
		if not k then
			return nil
		end
		local rec = panel_groups:get()[k]
		if not rec then
			return nil
		end
		local s = state.of(k)
		local open: { [string]: boolean } = {}
		for _, t in s.tabs do
			open[t.id] = true
		end
		local lists: { [string]: { string } } = {}
		local taken: { [string]: boolean } = {}
		for gid, g in rec.groups do
			local l = {}
			for _, id in g.tabs do
				if open[id] and not taken[id] then
					table.insert(l, id)
					taken[id] = true
				end
			end
			lists[gid] = l
		end
		local main = {}
		for _, t in s.tabs do
			if not taken[t.id] then
				table.insert(main, t.id)
			end
		end
		lists[MAIN] = main
		local t = rec.tree
		for _, gid in tree.leaves(rec.tree) do
			if not lists[gid] or #lists[gid] == 0 then
				t = tree.without(t, gid)
			end
		end
		if not t or #tree.leaves(t) < 2 then
			return nil
		end
		-- (Tabs no group shows, "main" gone from the layout: the focused
		-- group's, so none is lost.)
		local shown = tree.leaves(t)
		if #main > 0 and not table.find(shown, MAIN) then
			local into = if rec.focus and table.find(shown, rec.focus) then rec.focus else shown[1]
			local l = table.clone(lists[into])
			for _, id in main do
				table.insert(l, id)
			end
			lists[into] = l
			lists[MAIN] = nil
		end
		return { tree = t, lists = lists, focus = rec.focus, actives = rec.actives or {} }
	end)
	local function multi(): boolean
		return groups_now:get() ~= nil
	end
	-- The focused group: the one kept with the tabs (screens/panel/state
	-- moves it as tabs open, turn and close), while it's shown.
	local function focused_group(): string?
		local g = groups_now:get()
		if not g then
			return nil
		end
		local leaves = tree.leaves(g.tree)
		if g.focus and table.find(leaves, g.focus) then
			return g.focus
		end
		return leaves[1]
	end
	-- The tab a group shows: the active one if it's the focused group (nil:
	-- its empty page), else the one it showed last.
	local function group_shows(gid: string): string?
		local g = groups_now:get()
		if not g then
			return nil
		end
		if focused_group() == gid then
			return active()
		end
		local l = g.lists[gid] or {}
		local last = g.actives[gid]
		if last and table.find(l, last) then
			return last
		end
		return l[1]
	end
	-- The record as it now is, made from the groups as they stand (or, one
	-- group so far, a new one), changed by `change`, tidied and kept.
	local function rebuild(change: (rec: any) -> ())
		local k = key:peek()
		if not k then
			return
		end
		local g = groups_now:peek()
		local old = panel_groups:peek()[k]
		local rec: any
		if g then
			rec = { tree = g.tree, groups = {}, focus = g.focus, actives = table.clone(g.actives), next = (old and old.next) or 1 }
			for gid, l in g.lists do
				if gid ~= MAIN then
					rec.groups[gid] = { tabs = table.clone(l) }
				end
			end
		else
			rec = { tree = tree.leaf(MAIN), groups = {}, focus = MAIN, actives = {}, next = (old and old.next) or 1 }
		end
		change(rec)
		-- Tidied: each group less tabs gone; a group left empty leaves the
		-- layout ("main" too, when every tab is in another).
		local s = state.of(k)
		local open: { [string]: boolean } = {}
		for _, t in s.tabs do
			open[t.id] = true
		end
		local held = 0
		for gid, gg in rec.groups do
			local l = {}
			for _, id in gg.tabs do
				if open[id] then
					table.insert(l, id)
				end
			end
			rec.groups[gid] = if #l > 0 then { tabs = l } else nil
			held += #l
		end
		local t = rec.tree
		for _, gid in tree.leaves(rec.tree) do
			local n = if gid == MAIN then #s.tabs - held else (if rec.groups[gid] then #rec.groups[gid].tabs else 0)
			if n <= 0 then
				t = tree.without(t, gid)
			end
		end
		rec.tree = t or tree.leaf(MAIN)
		local all = table.clone(panel_groups:peek())
		all[k] = if #tree.leaves(rec.tree) >= 2 then rec else nil
		panel_groups:set(all)
	end
	-- `d` out of whichever group held it.
	local function take_out(rec: any, d: string)
		for gid, gg in rec.groups do
			local i = table.find(gg.tabs, d)
			if i then
				local l = table.clone(gg.tabs)
				table.remove(l, i)
				rec.groups[gid] = { tabs = l }
			end
		end
	end
	-- `d` in a group of its own at `target`'s side.
	local function new_group(d: string, target: string, side: string)
		local k = key:peek()
		if not k then
			return
		end
		rebuild(function(rec: any)
			take_out(rec, d)
			local gid = `g{rec.next}`
			rec.next += 1
			rec.groups[gid] = { tabs = { d } }
			rec.tree = tree.insert(rec.tree, target, gid, side) or rec.tree
			rec.focus = gid
			rec.actives[gid] = d
		end)
		state.activate(k, d)
	end
	-- `d` into the group `gid`: before or after its tab `at` (none: at its
	-- end). Into "main", it takes its place in the panel's own order.
	local function into_group(d: string, gid: string, at: string?, after: boolean?)
		local k = key:peek()
		if not k then
			return
		end
		rebuild(function(rec: any)
			take_out(rec, d)
			if gid ~= MAIN then
				local l = table.clone(if rec.groups[gid] then rec.groups[gid].tabs else {})
				local i = if at then table.find(l, at) else nil
				if i then
					table.insert(l, if after then i + 1 else i, d)
				else
					table.insert(l, d)
				end
				rec.groups[gid] = { tabs = l }
			end
			rec.focus = gid
			rec.actives[gid] = d
		end)
		if gid == MAIN and at and at ~= d then
			local list = state.of(k).tabs
			local i = nil
			for n, t in list do
				if t.id == at then
					i = n
				end
			end
			if i then
				state.move(k, d, if after then i + 1 else i)
			end
		end
		state.activate(k, d)
	end
	-- Where the groups stand in the panel (all of it, from its top: the top
	-- ones' strips in the panel's own band), as the page's split chats are:
	-- edges that are a share of the panel plus points, so resizing it or
	-- dragging its edge runs no script (screens/shell/split).
	local group_frames = ui.computed(function(): { [string]: any }
		local g = groups_now:get()
		return tree.frames(if g then g.tree else nil, INSET_GAP)
	end)
	local function gbox(gid: string): any
		return group_frames:get()[gid] or Split.WHOLE
	end
	local group_ids = ui.computed(function(): { string }
		local g = groups_now:get()
		return if g then tree.leaves(g.tree) else {}
	end)
	local function kind_icon(tab: Tab?): string
		if not tab then
			return "widget"
		end
		if tab.kind == "terminal" then
			return "terminal"
		elseif tab.kind == "browser" then
			return "global"
		elseif tab.kind == "plugin" then
			return (tab.icon or "widget") :: any
		elseif tab.kind == "side_chat" then
			return "side-chat"
		elseif tab.kind == "subagent" then
			return "bot"
		end
		return "document"
	end
	local function is_tab(payload: any): boolean
		local k = key:peek()
		return k ~= nil and type(payload) == "table" and type(payload.panel_tab) == "string" and state.find(k, payload.panel_tab) ~= nil
	end
	-- A group's tab in its strip: as the panel's own (its icon, its title,
	-- × under the pointer), shown as the group shows it; dragged, it moves
	-- (onto another tab: beside it; to a group's edge: a group of its own).
	local function group_chip(gid: string, id: string): Node
		local k = key:peek()
		local tab = if k then state.find(k, id) else nil
		local hovered = ui.use_state(false)
		local function shown(): boolean
			return group_shows(gid) == id
		end
		local function lit(): boolean
			return shown() and focused_group() == gid
		end
		local function title(): string
			if not tab then
				return ""
			end
			local t = title_of(tab)
			return t
		end
		return ui.row {
			id = `panel-tab-{id}`,
			class = if tab and tab.kind == "files" then "panel-tab file-tab" else "panel-tab",
			provide = { panel = id },
			role = "tab",
			label = title,
			selected = shown,
			h = CHIP_H,
			shrink = 0,
			pl = "$space.3",
			pr = "$space.1",
			gap = "$zeron.space.tight",
			items = "center",
			rounded = "$radius.lg",
			cursor = "pointer",
			bg = function(): string
				if lit() then
					return "$zeron.selected.flat.bg"
				end
				if shown() or hovered:get() then
					return "$hover"
				end
				return "#00000000"
			end,
			color = function(): string
				return if shown() or hovered:get() then "$text.primary" else "$text.secondary"
			end,
			on_hover = function(inside: boolean)
				hovered:set(inside)
			end,
			on_click = function()
				if k then
					state.activate(k, id)
				end
			end,
			on_aux_click = function(e)
				if e.button == "middle" then
					close(id)
				end
			end,
			on_drag = function(): any
				tab_dragging:set(true)
				return { panel_tab = id }
			end,
			drag_preview = function(): Node
				local card: { [any]: any } = {
					h = CHIP_H,
					pl = "$space.3",
					pr = "$space.3",
					gap = "$zeron.space.tight",
					items = "center",
					rounded = "$radius.lg",
					color = "$text.primary",
					Icon { name = kind_icon(tab) :: any, size = 14 },
					FadeText { title(), max_w = LABEL_MAX, text_size = "$font.size.sm", weight = "$font.weight.medium" },
				}
				require("@ds/glass").style(card, "menu")
				return ui.row(card)
			end,
			on_drag_end = function()
				tab_dragging:set(false)
			end,
			drop_when = function(payload: any): boolean
				return is_tab(payload) and payload.panel_tab ~= id
			end,
			on_drop = function(payload: any, e)
				tab_dragging:set(false)
				into_group(payload.panel_tab, gid, id, e.local_x > e.w / 2)
			end,
			ui.div { y = 0.5, shrink = 0, Icon { name = kind_icon(tab) :: any, size = 14 } },
			FadeText {
				title,
				max_w = LABEL_MAX,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
			},
			ui.row {
				w = SLOT,
				h = SLOT,
				shrink = 0,
				items = "center",
				justify = "center",
				ui.show(function(): boolean
					return hovered:get()
				end, IconButton {
					label = ui.t("panel.tab.close"),
					tooltip_side = "bottom",
					size = "sm",
					tone = "quiet",
					w = SLOT,
					h = SLOT,
					rounded = "$radius.sm",
					on_click = function()
						close(id)
					end,
					Icon { name = "close", size = 12 },
				}),
			},
		}
	end
	-- A group's strip: its tabs, "+" (its empty page, the group focused),
	-- and, for the group at the panel's top right, Expand. The top groups'
	-- strips stand in the panel's band, level with the title bar's controls
	-- (clear of them at its ends); what isn't a tab moves the window.
	local function group_strip(gid: string): Node
		local list = ui.computed(function(): { string }
			local g = groups_now:get()
			return if g then g.lists[gid] or {} else {}
		end)
		local function top_left(): boolean
			return Split.top_left(gbox(gid))
		end
		local function top_right(): boolean
			return Split.top_right(gbox(gid))
		end
		return ui.row {
			w_full = true,
			h = BAND_H,
			shrink = 0,
			pt = BAND_PAD,
			pl = function(): number
				if top_left() and left_of then
					return ui.raw(math.max(STRIP_START_PT, CONTROLS_END - left_of()), "clear of the title bar's controls")
				end
				return ui.raw(STRIP_START_PT, "the strip's start")
			end,
			pr = function(): number
				return ui.raw(if top_right() then TOGGLE_ROOM + ds.TitleBar.window_buttons_right() else STRIP_EDGE, "the title bar's panel toggle over the top right group's end, else as far in as from its top")
			end,
			items = "center",
			window_drag = true,
			-- (Dropped on its strip past its tabs: to its end.)
			drop_when = is_tab,
			on_drop = function(payload: any)
				tab_dragging:set(false)
				into_group(payload.panel_tab, gid)
			end,
			ui.scroll_x {
				grow = 1,
				min_w = 0,
				h = CHIP_H,
				fade_x = { STRIP_FADE, STRIP_FADE },
				fade_overflow = true,
				ui.row {
					gap = "$zeron.space.hair",
					items = "center",
					ui.each(list, function(id: string): Node
						return group_chip(gid, id)
					end, {
						key = function(id: string): string
							return id
						end,
						direction = "row",
						gap = "$zeron.space.hair",
						animate = motion.layout(SLIDE),
					}),
					IconButton {
						label = ui.t("panel.strip.new_tab"),
						tooltip_side = "bottom",
						size = "sm",
						tone = "quiet",
						w = CHIP_H,
						h = CHIP_H,
						rounded = "$radius.lg",
						on_click = function()
							local k = key:peek()
							if not k then
								return
							end
							local all = table.clone(panel_groups:peek())
							local r = all[k]
							if r then
								r = table.clone(r)
								r.focus = gid
								all[k] = r
								panel_groups:set(all)
							end
							state.activate(k, nil)
						end,
						Icon { name = "plus", size = 14 },
					},
				},
			},
			if shell_view and shell_view.toggle_expanded
				then ui.show(top_right, ui.div {
					shrink = 0,
					ml = "$space.1",
					ui.presence(function(): string
						return if shell_view.expanded:get() then "collapse" else "expand"
					end, function(which: string): Node
						return IconButton {
							label = if which == "collapse" then ui.t("panel.strip.collapse") else ui.t("panel.strip.expand"),
							tooltip_side = "bottom",
							w = CONTROL,
							h = CONTROL,
							on_click = function()
								shell_view.toggle_expanded()
							end,
							Icon { name = if which == "collapse" then "collapse-arrows" else "expand-arrows", size = 16 },
						}
					end),
				})
				else nil,
		}
	end
	-- Where a dragged tab is held, and where it would land: over a group's
	-- body (or the one panel's), the edge it's near ("left", "right", "top",
	-- "bottom": a group of its own there) or its middle (into the group).
	-- The body itself takes the drop, from where the pointer is in it: no
	-- layer over the tabs' views, so nothing ever stands between them and
	-- the pointer.
	local held = ui.use_state(nil :: { id: string, side: string }?)
	local EDGE = 0.3
	local function side_at(e: any, edges_only: boolean): string?
		local fx = if (e.w or 0) > 0 then e.local_x / e.w else 0.5
		local fy = if (e.h or 0) > 0 then e.local_y / e.h else 0.5
		if fx < EDGE then
			return "left"
		elseif fx > 1 - EDGE then
			return "right"
		elseif fy < EDGE then
			return "top"
		elseif fy > 1 - EDGE then
			return "bottom"
		end
		return if edges_only then nil else "middle"
	end
	-- The drop props for a body: `gid`'s, `accepts` saying whether the tab
	-- may land at that side.
	local function droppable(gid: string, edges_only: boolean, accepts: (d: string, side: string) -> boolean): { [string]: any }
		local function hold(side: string?)
			local o = held:peek()
			if side == nil then
				if o and o.id == gid then
					held:set(nil)
				end
			elseif not o or o.id ~= gid or o.side ~= side then
				held:set({ id = gid, side = side })
			end
		end
		return {
			drop_when = is_tab,
			on_drag_over = function(payload: any, e)
				local side = side_at(e, edges_only)
				hold(if side and accepts(payload.panel_tab, side) then side else nil)
			end,
			on_drag_leave = function()
				hold(nil)
			end,
			on_drop = function(payload: any, e)
				hold(nil)
				tab_dragging:set(false)
				local side = side_at(e, edges_only)
				if not side or not accepts(payload.panel_tab, side) then
					return
				end
				if side == "middle" then
					into_group(payload.panel_tab, gid)
				else
					new_group(payload.panel_tab, gid, side)
				end
			end,
		}
	end
	-- Where a tab held over a body would land: that half of it (its middle:
	-- all of it), drawn over it, taking no pointer. (Shares of the body:
	-- nothing here is worked out from its size.)
	local HALF: { [string]: { [string]: any } } = {
		middle = { left = 0, right = 0, top = 0, bottom = 0 },
		left = { left = 0, right = "50%", top = 0, bottom = 0 },
		right = { left = "50%", right = 0, top = 0, bottom = 0 },
		top = { left = 0, right = 0, top = 0, bottom = "50%" },
		bottom = { left = 0, right = 0, top = "50%", bottom = 0 },
	}
	local function drop_preview(gid: string): Node
		local function side(): string
			local o = held:get()
			return if o and o.id == gid then o.side else ""
		end
		local function edge(name: string): () -> any
			return function(): any
				return (HALF[side()] or HALF.middle)[name]
			end
		end
		return ui.show(function(): boolean
			return side() ~= ""
		end, ui.div {
			id = `panel-drop-{gid}`,
			absolute = true,
			left = edge("left"),
			right = edge("right"),
			top = edge("top"),
			bottom = edge("bottom"),
			pointer_events = "none",
			bg = "$zeron.inset.drop",
			border = 2,
			border_color = "$zeron.inset.drop_edge",
			-- (Its corners follow the card's, inside its edge.)
			rounded = "$zeron.inset.inner_radius",
		})
	end

	-- A group: a card as the page's chat panes are (the window's gap round
	-- it, its corners, its edge; the focused one's edge lit, the others
	-- dimmed all over, strip and all), its strip over the tab it shows (its
	-- empty page when it's focused with none). A press anywhere in it
	-- focuses it.
	local function group_pane(gid: string): Node
		local k = key:peek()
		local function is_focus(): boolean
			return focused_group() == gid
		end
		local function list(): { string }
			local g = groups_now:peek()
			return if g then g.lists[gid] or {} else {}
		end
		-- (Its own last tab can't leave it for its own edge: nothing would
		-- be left of it. Its middle takes tabs from other groups.)
		local function accepts(d: string, side: string): boolean
			local l = list()
			if side == "middle" then
				return not table.find(l, d)
			end
			return not (#l == 1 and l[1] == d)
		end
		local view = ui.presence(function(): string
			return group_shows(gid) or ""
		end, function(id: string): Node
			if not k then
				return ui.div {}
			end
			if id == "" then
				return empty_page(k, showing)
			end
			local tab = state.find(k, id)
			if not tab then
				return ui.div {}
			end
			local ok, built = pcall(body, k, tab)
			if ok then
				return built
			end
			return ds.EmptyState {
				icon = "danger-triangle",
				size = "sm",
				fill = true,
				title = ui.t("panel.broken.tab.title") :: any,
				body = ui.t("panel.broken.tab.body") :: any,
			}
		end, { w_full = true, grow = 1, min_h = 0 })
		-- Focused: its tab the active one (a press in it, or the keyboard
		-- landing in it).
		local function focus_me()
			if not k or is_focus() then
				return
			end
			local shows = group_shows(gid)
			if shows then
				state.activate(k, shows)
			end
		end
		local body_props: { [any]: any } = droppable(gid, false, accepts)
		body_props.w_full = true
		body_props.grow = 1
		body_props.min_h = 0
		body_props.relative = true
		body_props[1] = view
		body_props[2] = drop_preview(gid)
		local card: { [any]: any } = {
			overflow = "hidden",
			-- (The focused group lifted, the others sunk.)
			bg = function(): string
				return if is_focus() then "$zeron.inset.active" else "$zeron.inset.inactive"
			end,
			rounded = "$zeron.inset.radius",
			border = 1,
			border_color = function(): string
				return if is_focus() then "$zeron.inset.focus_edge" else "$zeron.inset.edge"
			end,
			-- (Pressed anywhere in it, a terminal or an editor included: a
			-- press bubbles up here past what took it. Not as the keyboard
			-- lands in it: a terminal taking the keys as it's shown, or as
			-- the panel comes back, would move the focus nobody chose.)
			on_press = focus_me,
			group_strip(gid),
			ui.column(body_props),
			-- Dimmed all over while another group is focused (taking no
			-- pointer: a press goes to what's under it, and focuses it).
			ui.div {
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				pointer_events = "none",
				bg = function(): string
					return if is_focus() then "#00000000" else "$zeron.inset.pane_dim"
				end,
			},
		}
		-- (`#panel-group-<gid>`: a group of the panel's tabs.)
		card.id = `panel-group-{gid}`
		Split.place(card, function(): any
			return gbox(gid)
		end)
		return ui.column(card)
	end
	-- The panel's groups, over all of it while there are two or more, the
	-- window's gap between them.
	local groups_layer = ui.column {
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		pointer_events = "none",
		hidden = function(): boolean
			return closed() or not multi()
		end,
		ui.show(multi, ui.each(group_ids, function(gid: string): Node
			return group_pane(gid)
		end, {
			key = function(gid: string): string
				return gid
			end,
			w_full = true,
			h_full = true,
			relative = true,
		})),
	}
	-- (Its groups' cards stand on the window: the panel's own card goes
	-- while they show, screens/shell/page.)
	local groups_shown = ui.shared("panel.groups_shown", false)
	ui.effect(function()
		local on = multi()
		if groups_shown:peek() ~= on then
			groups_shown:set(on)
		end
	end)
	ui.on_cleanup(function()
		groups_shown:set(false)
	end)
	-- One group: the panel's own body, where a tab dragged to an edge
	-- starts a second group (a tab alone has nowhere to go).
	local function splits_off(_d: string, _side: string): boolean
		local k = key:peek()
		return k ~= nil and #state.of(k).tabs >= 2
	end
	local panes_props: { [any]: any } = droppable(MAIN, true, splits_off)
	panes_props.w_full = true
	panes_props.grow = 1
	panes_props.min_h = 0
	panes_props.relative = true
	panes_props.hidden = function(): boolean
		return closed() or multi()
	end
	panes_props[1] = ui.show(function(): boolean
		return not multi()
	end, content)
	panes_props[2] = drop_preview(MAIN)
	local panes = ui.column(panes_props)

	local question = ui.div {
		-- (Closing loses work: no plugin reaches it.)
		hook = false,
		ds.Dialog {
			open = asking,
			title = ui.t("panel.close_question.title") :: any,
			tone = "danger",
			confirm = {
				label = ui.t("common.close") :: any,
				on_click = function()
					local a = asked:peek()
					if a then
						close_now(a.key, a.id)
					end
				end,
			},
			ui.text {
				function(): string
					local a = asked:get()
					if not a then
						return ""
					end
					if a.why == "running" then
						return ui.t("panel.close_question.running", { title = a.title }):get()
					end
					return ui.t("panel.close_question.unsaved", { title = a.title }):get()
				end,
				literal = true,
				wrap = true,
				text_size = "$font.size.md",
				color = "$text.secondary",
			},
		},
	}

	local box: { [any]: any } = {}
	for k, v in props do
		if k ~= "project" and k ~= "session" and k ~= "page" and k ~= "left" and k ~= "hold" and k ~= "on_last_closed" and k ~= "docked" and k ~= "children" then
			box[k] = v
		end
	end
	box.id = box.id or "panel"
	box.hook = "panel"
	if box.w == nil and box.w_full == nil then
		box.w_full = true
	end
	if box.h == nil and box.h_full == nil and box.grow == nil then
		box.h_full = true
	end
	box.min_h = 0
	-- The keys: close the active tab, step through them.
	-- Where the keyboard is (focus and blur reach the panel from what's in
	-- it), and how a tab closes, for the app's ⌘W.
	closer = close
	shown_key = function(): string?
		return key:peek()
	end
	-- (Focus reaches the panel from what's in it, before the window's
	-- root hears it (`focus_moved`); a blur reaches only what lost it.)
	box.on_focus = function()
		focus_in = key:peek()
		claimed = true
	end
	-- (A press inside: the root's `press_moved` leaves the claim be.)
	box.on_press = function()
		pressed_in = true
	end
	ui.on_cleanup(function()
		if closer == close then
			closer, shown_key, focus_in = nil, nil, nil
		end
	end)
	box.on_key = function(e): boolean?
		local k = key:peek()
		if not k then
			return nil
		end
		local s = state.of(k)
		-- Escape nothing in the tab took (the editor, the tree, a file's
		-- header): the keyboard to the tab's own place in the strip, a way
		-- out of what holds on to Tab (a code editor), from where Tab and
		-- the arrows go on. (A terminal or a web page takes its Escape.)
		if e.key == "Escape" and not (e.cmd or e.ctrl or e.alt or e.shift) and s.active then
			local r = chip_refs[s.active]
			local el = if r then r:peek() else nil
			if el and el:focus() then
				return true
			end
			return nil
		end
		if e.cmd and not e.shift and not e.alt and string.lower(e.key) == "w" then
			if s.active then
				close(s.active)
				return true
			end
			return nil
		end
		if e.ctrl and string.lower(e.key) == "tab" and #s.tabs > 0 then
			local at = 0
			for i, t in s.tabs do
				if t.id == s.active then
					at = i
				end
			end
			local n = #s.tabs
			local to = if e.shift then (if at <= 1 then n else at - 1) else (at % n) + 1
			state.activate(k, s.tabs[to].id)
			return true
		end
		return nil
	end
	-- (Its width, measured: `showing` reads it. Only when the shell says
	-- it's closed, too: one made again, as a reload does, measures no width
	-- for a moment.)
	box.ref = box.ref or own
	measured = box.ref
	-- (With two or more groups, each has its strip; the panel's own
	-- goes, its groups over all of it.)
	box.relative = true
	box[#box + 1] = ui.column { w_full = true, shrink = 0, hidden = function(): boolean
		return closed() or multi()
	end, strip }
	box[#box + 1] = panes
	box[#box + 1] = groups_layer
	box[#box + 1] = question
	return ui.column(box)
end)

return M
