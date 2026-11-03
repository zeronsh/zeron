-- The main window, after setup: Zeron's shell. The sidebar (your sessions,
-- grouped, sorted, pinned, in your sections) runs to the window's top; the
-- page beside it shows where the route is (a new chat, a session, settings)
-- with the window's one composer travelling between a new chat and a
-- session; the title bar lies over both. Collapsed, the sidebar folds away
-- entirely (its rail, floating at the window's bottom left, is kept behind
-- `view.rail`, off).
--
-- Every part is its own module, required as `@app/screens/shell/…`, so a
-- trusted plugin can replace one whole: `route` (where the window is),
-- `sidebar`, `title_bar`, `page`, `settings`, `appearance`, `boundary`.
-- Each screen loads through `boundary`: one that fails shows so in its
-- place and the window goes on. Where the window is lives outside them
-- (the route, @account/sessions, named states), so a screen made again,
-- its module saved or replaced, comes back where it was. Plugins add at
-- the declared hooks: `shell.sidebar`, `sidebar.actions`, `rail.actions`,
-- `titlebar.actions`, `shell.page`, `shell.files`, `settings.nav`,
-- `settings.page`, and the slots `shell.page` and `settings.nav`. You (the
-- sidebar's foot) and Account are out of their reach.
--
-- The keys: ⌘N a new chat, ⌘K the palette, ⌘, settings, ⇧⌘P pin the
-- session open, ⌘B the sidebar, ⌘R a session's panel, ⌘1–9 the sidebar's
-- sessions, ⌃Tab and ⌃⇧Tab the next and previous, ⌘⇧A archive the one open,
-- ⌘E its files, ⌘J its terminal, ⌘/ the model picker, ⌘U a wallpaper at
-- random, ⌘W its active
-- tab (while the panel has the keyboard; else the window); ⌘[ and ⌘] back
-- and forward
-- (the router's own commands).

local ds = require("@ds")
local material = require("@ds/material")
local auth = require("@account/auth")
local sessions = require("@account/sessions")
local route = require("@app/screens/shell/route")
local boundary = require("@app/screens/shell/boundary")
local view = require("@app/screens/shell/view")
local Links = require("@ds/links")
local MenuAt = require("@ds/menu_at")
local prefs = require("@app/screens/settings/prefs")
local fit = require("@app/screens/shell/fit")
local keys = require("@app/screens/shell/keys")
local host = require("@app/screens/settings/host")
-- (The look chosen, applied as it loads.)
local appearance = require("@app/screens/shell/appearance")
local motion = require("@ds/motion")

local TITLE_H = 38

-- The shell's screens (and the others' it shows), loaded with the script,
-- so what they make as they load (the sidebar's sync with the engine, its
-- computeds) is the script's for as long as it runs, not the first
-- screen's that asked for them. (Each still loads through the boundary:
-- one that fails shows so in its place.)
for _, module in {
	"@app/screens/shell/sidebar",
	"@app/screens/shell/page",
	"@app/screens/shell/title_bar",
	"@app/screens/shell/palette",
	"@app/screens/shell/add_project",
	"@app/screens/shell/settings",
	"@app/screens/settings",
	"@app/screens/chat",
	"@app/screens/panel",
	"@app/screens/files",
} do
	boundary.load(module)
end

-- The finished session last marked seen (kept across the shell being made
-- again).
local seen_told: string? = nil

return function(): Node
	-- The window's router (screens/shell/route): made with the shell, so
	-- each load makes it anew where the window was.
	route.make()
	-- Kept across launches: the sidebar collapsed, a session's files shown.
	local files = view.files
	-- The sidebar as it shows: folded by its toggle, or hidden by the
	-- window's fit for room (screens/shell/fit: still open, back as the
	-- window widens), and the most it may take.
	local collapsed = ui.computed(function(): boolean
		return view.collapsed:get() or not fit.current:get().sidebar
	end)
	local function sidebar_limit(): number
		return fit.sidebar_room:get()
	end
	-- Whether the sidebar's width changing now follows the window at once:
	-- the window's width moved since the sidebar last asked (it asks every
	-- time it moves, reading the width, so this is never stale).
	local sidebar_seen_vw = fit.viewport:peek()
	local function sidebar_direct(): boolean
		local vw = fit.viewport:get()
		local moved = vw ~= sidebar_seen_vw
		sidebar_seen_vw = vw
		return moved
	end

	-- The window's commands, in the menu bar with their shortcuts (src/main.rs
	-- `serve_commands`): a shortcut reaches the app only as its menu item's,
	-- after whatever has focus leaves the key alone (a field keeps its own
	-- ⌘F). Each is made from its line in screens/shell/keys (its title, its
	-- key, its menu; Settings → Shortcuts lists the same), with what it does
	-- here, in the order keys lists them.
	local runs: { [string]: () -> ...unknown } = {}
	-- ⌘F: the app's own Find, so zgx adds no find in page of its own (it
	-- does unless the menus bind ⌘F). Whatever has focus has it first (the
	-- code editor's Find, a field); with nothing taking it, settings' search
	-- on the settings page, then a chat's find bar on the chat page
	-- (screens/chat/find), else nothing.
	runs["shell.find"] = function()
		-- (A browser tab with the keyboard keeps ⌘F for its page, as the
		-- editor keeps its own: screens/panel/browser's find bar.)
		local panel: any = boundary.load("@app/screens/panel")
		if panel and panel.find_in_browser and (panel.find_in_browser :: any)() == true then
			return true
		end
		local search: any = boundary.load("@app/screens/settings/search")
		if search ~= nil and search.focus ~= nil and (search.focus :: any)() == true then
			return true
		end
		local find: any = boundary.load("@app/screens/chat/find")
		return find ~= nil and find.open ~= nil and (find.open :: any)() == true
	end
	runs["shell.new_chat"] = function()
		route.new_chat()
	end
	runs["shell.palette"] = function()
		view.palette:set(not view.palette:peek())
	end
	runs["shell.settings"] = function()
		route.settings()
	end
	runs["shell.toggle_sidebar"] = function()
		fit.toggle_sidebar()
	end
	runs["shell.toggle_files"] = function()
		local name = route.current:peek().name
		if name == "session" or name == "plugin" then
			fit.toggle_panel()
		end
	end
	-- ⌘W: the panel's active tab, while the panel has the keyboard (as
	-- × closes it: a file with changes, a busy terminal, ask first). Else
	-- the window, as Zeron's closes it, and with it the app: what's kept is
	-- saved as it closes (src/main.rs), the engine and its terminals go on.
	-- (In a split, else the pane in front: the focused chat out of it, its
	-- neighbour focused, the last left alone. A chat alone closes the
	-- window, as it always has.)
	runs["shell.close"] = function()
		local panel: any = boundary.load("@app/screens/panel")
		if panel and panel.close_focused_tab and (panel.close_focused_tab :: any)() then
			return
		end
		if route.current:peek().name == "session" then
			local splits: any = boundary.load("@app/screens/shell/split")
			local focused = if splits then splits.focused:peek() else nil
			if splits and focused and splits.shown:peek() ~= nil then
				splits.close(focused)
				return
			end
		end
		ui.invoke("close")
	end
	-- The next and previous session, in the sidebar's order, round from the
	-- end (from a new chat, its first or last). (Zeron's, shell/tabs.rs.)
	local function step(by: number)
		-- (While the panel has the keyboard, its tabs, as Zeron's right pane.)
		local panel: any = boundary.load("@app/screens/panel")
		if panel and panel.step_tab and (panel.step_tab :: any)(by) then
			return
		end
		local sidebar: any = boundary.load("@app/screens/shell/sidebar")
		local ids: { string } = if sidebar then sidebar.order:peek() else {}
		if #ids == 0 then
			return
		end
		local at = table.find(ids, route.current:peek().session or "")
		local i = if at then (at - 1 + by) % #ids + 1 elseif by > 0 then 1 else #ids
		route.open_session(ids[i])
	end
	runs["shell.next_session"] = function()
		step(1)
	end
	runs["shell.previous_session"] = function()
		step(-1)
	end
	-- A web link in a chat (ds/markdown's): the panel's browser tab beside
	-- the session showing (or the plugin page showing: its own panel), with
	-- Settings → Links "Open links in" Zeron (the shipping app's
	-- `open_web_links_in_zeron`); else, or with neither open, the default
	-- browser.
	-- (`panel`: that tab whatever the setting, a link's menu's "Open in
	-- browser panel"; `in_app`: where its Open goes.)
	Links.panel = function(url: string): boolean
		local panel: any = boundary.load("@app/screens/panel")
		local session = route.session:peek()
		local page = if session then nil else route.page:peek()
		if not panel or not panel.open_url or not (session or page) then
			return false
		end
		return (panel.open_url :: any)(session, url, route.project:peek(), page) == true
	end
	Links.in_app = function(): boolean
		return prefs.open_links:peek() == "zeron"
	end
	Links.panel_ready = function(): boolean
		return route.session:peek() ~= nil or route.page:peek() ~= nil
	end
	Links.opener = function(url: string): boolean
		if prefs.open_links:peek() ~= "zeron" then
			return false
		end
		return (Links.panel :: (string) -> boolean)(url)
	end
	-- ⌘E, ⌘J: the session's panel on its files, on its terminal (open there,
	-- or closed if that's what it shows). (Zeron's Files and terminal.)
	local function panel_call(name: string): () -> ()
		return function()
			local panel: any = boundary.load("@app/screens/panel")
			local call = panel and panel[name]
			if call then
				call()
			end
		end
	end
	runs["shell.toggle_files_tab"] = panel_call("toggle_files")
	runs["shell.toggle_terminal"] = panel_call("toggle_terminal")
	-- ⌘/: the composer's model picker (a focused code editor keeps ⌘/ for
	-- its comments: it takes the key first).
	runs["shell.model_picker"] = function()
		local chat: any = boundary.load("@app/screens/chat")
		if chat and chat.open_model_picker then
			(chat.open_model_picker :: any)()
		end
	end
	-- ⌘⇧A: the session open archived (never unarchived; not while its
	-- agent works, as its menu has it). (⌘A is a field's Select all.)
	runs["shell.archive"] = function()
		local id = route.current:peek().session
		if not id then
			return
		end
		for _, c in sessions.now:peek().chats do
			if c.id == id and c.status ~= "working" then
				local sidebar: any = boundary.load("@app/screens/shell/sidebar")
				if sidebar then
					sidebar.archive(id)
				end
			end
		end
	end
	-- ⌘1–9: the sidebar's first nine sessions, as it shows them (Zeron's
	-- jumps, shell.rs), its hints kept up through them (screens/shell/sidebar
	-- `jump_hints`).
	for i = 1, 9 do
		runs[`shell.session_{i}`] = function()
			local sidebar: any = boundary.load("@app/screens/shell/sidebar")
			local id = sidebar and sidebar.order:peek()[i]
			if id then
				route.open_session(id)
			end
		end
	end
	-- ⌘⇧N: New project's palette, or closed if it's open (not on settings,
	-- as Zeron's).
	runs["shell.new_project"] = function()
		if route.current:peek().name == "settings" then
			return
		end
		local add: any = boundary.load("@app/screens/shell/add_project")
		if add then
			if add.shown:peek() then
				add.shown:set(false)
			else
				add.open()
			end
		end
	end
	-- Reload Zeron: the app quits and starts again, the same launch, its
	-- engine left running (src/relaunch.rs). Reload interface: every script
	-- evaluated again in place, as a save does (src/main.rs `serve`).
	-- (Through settings' host calls, which a fixture answers.)
	runs["shell.reload"] = function()
		host.invoke("app.relaunch")
	end
	runs["shell.reload_interface"] = function()
		host.invoke("app.reload_interface")
	end
	runs["shell.pin"] = function()
		local s = route.current:peek().session
		local sidebar = boundary.load("@app/screens/shell/sidebar")
		if s and sidebar then
			sidebar.pin(s)
		end
	end
	-- Each on its key now (Settings → Shortcuts rebinds it at once). A
	-- command's key goes to its menu item before any `on_key` hears it, so
	-- each but ⌘1–9 tells the sidebar's ⌘ hints a key went down itself.
	for _, k in keys.LIST do
		local run = runs[k.id]
		if run then
			local jump = string.match(k.id, "^shell%.session_%d$") ~= nil
			ui.command {
				id = k.id,
				title = keys.title(k),
				menu = k.menu,
				key = keys.key(k.id),
				reserved = k.reserved,
				run = function()
					local sidebar: any = boundary.load("@app/screens/shell/sidebar")
					if sidebar and sidebar.key_pressed then
						sidebar.key_pressed(jump)
					end
					run()
				end,
			}
		end
	end

	-- A session open that finished since it was last seen: seen now, on
	-- every device (its "done" goes).
	-- (Told once per finish: the computed holds while it says the same,
	-- so the chats changing meanwhile ask nothing more of the engine.)
	local finished = ui.computed(function(): string?
		local s = route.session:get()
		if not s then
			return nil
		end
		for _, c in sessions.now:get().chats do
			if c.id == s.id and (c.status == "done" or c.status == "failed") then
				return c.id
			end
		end
		return nil
	end)
	ui.effect(function()
		local id = finished:get()
		-- (Told once a finish: the shell made again doesn't tell it again;
		-- finished once more later, it does.)
		if id == seen_told then
			return
		end
		seen_told = id
		if id then
			sessions.seen(id)
		end
	end)
	-- Signed out here (Settings, or the session ending): setup again. (A
	-- window opened without an account stays: signing out is a change.)
	local was_signed_in = auth.signed_in:peek()
	ui.effect(function()
		local state = auth.state:get()
		if state == "signed_in" then
			was_signed_in = true
		elseif state == "signed_out" and was_signed_in then
			was_signed_in = false
			ui.invoke("app.onboard")
		end
	end)

	local root = ui.ref()
	local function window_w(): number
		return root.width or 1240
	end
	-- Whether settings shows, over the whole window.
	local function in_settings(): boolean
		return route.current:get().name == "settings"
	end
	-- Leaving settings (Esc, Back to chats, a step back), its focus goes
	-- with it: nothing of the chats lights up for having been next.
	local was_settings = in_settings()
	ui.effect(function()
		local now = in_settings()
		if was_settings and not now then
			ui.after(0, function()
				local el = root:peek()
				if el then
					el:blur()
				end
			end)
		end
		was_settings = now
	end)
	-- Whether the title bar stands over a wallpaper: a new chat's.
	local function art(): boolean
		return route.current:get().name == "home" and appearance.showing:get() ~= "none"
	end
	local frosted = ui.computed(function(): number
		return if material.frosted:get() then 1 else 0
	end)
	return ui.column {
		ref = root,
		-- (The window's width: the columns fit it.)
		on_resize = function(size)
			fit.set_viewport(size.w)
		end,
		id = "shell",
		-- The accent chosen (Settings → Appearance, setup's Make it yours), as
		-- setup's frame wears it, and the interface's text size (ds.look):
		-- nothing for the theme's own.
		theme = function()
			return ds.look.window_theme()
		end,
		-- What plugins anywhere in the window read (plain values, never
		-- records): the look, where the window is. (Not who's signed in:
		-- your account is "profile.read"'s, its id in `zeron.profile_id`.)
		provide = {
			appearance = ui.computed(function(): string
				local kind = ui.theme.kind()
				return if kind == "light" or kind == "high-contrast-light" then "light" else "dark"
			end),
			route = ui.computed(function(): { [string]: string }
				local c = route.current:get()
				return { name = c.name, session = c.session, page = c.page } :: any
			end),
		},
		w_full = true,
		h_full = true,
		relative = true,
		-- A bare Escape nothing took (nothing focused): the chat's, which
		-- stops its turn when settings say Escape does (and does nothing
		-- otherwise). Menus and dialogs have it first, holding focus. Keys
		-- bubble here from whatever has focus, too.
		on_key = function(e): boolean
			-- Any key down while ⌘ is held: the sidebar's hints go (⌘C), but
			-- for the ⌘1–9 they offer. (`cmd` is Ctrl or Command, as the
			-- platform has it: both down, the other is held too.)
			local sidebar: any = boundary.load("@app/screens/shell/sidebar")
			if sidebar and sidebar.key_pressed then
				sidebar.key_pressed(string.match(e.key, "^[1-9]$") ~= nil and not (e.shift or e.alt or (e.ctrl and e.meta)))
			end
			if e.key == "Escape" and not (e.shift or e.cmd or e.alt or e.ctrl) then
				local store: any = boundary.load("@app/screens/chat/store")
				return store ~= nil and store.escape ~= nil and (store.escape :: any)() == true
			end
			-- A bare I nothing took, with no text field holding the keyboard,
			-- on a chat or a new one: into the composer, as vim's insert
			-- (Escape there gives the keyboard back: ds/composer).
			if e.key == "i" and not (e.editing or e.shift or e.cmd or e.alt or e.ctrl or e.meta or e["repeat"]) then
				local name = route.current:peek().name
				if name == "session" or name == "home" then
					local shared: any = boundary.load("@app/screens/chat/shared")
					if shared and shared.focus then
						shared.focus:add(1)
						return true
					end
				end
			end
			-- A bare letter nothing took, with no text field holding the
			-- keyboard: the panel home's (B, F, T…), while it shows.
			local panel: any = boundary.load("@app/screens/panel")
			if panel and panel.home_key and (panel.home_key :: any)(e) == true then
				return true
			end
			return false
		end,
		-- Every focus passes here last: the panel's claim on the keyboard
		-- (for ⌘W) ends when it lands outside the panel.
		on_focus = function()
			local panel: any = boundary.load("@app/screens/panel")
			if panel and panel.focus_moved then
				(panel.focus_moved :: any)()
			end
		end,
		-- The window's glass, or (opaque) its colour.
		bg = ui.tween(frosted, { from = "$zeron.window", to = "$background", duration = 320, ease = "outQuad" }),
		-- The chats: the sidebar, the page (its composer, a session's files),
		-- the rail and the title bar. Kept while settings shows, out of
		-- sight and out of the way, so they come back as they were (scroll,
		-- draft, files and all).
		ui.column {
			id = "shell-chats",
			w_full = true,
			h_full = true,
			relative = true,
			hidden = in_settings,
			ui.row {
				grow = 1,
				min_h = 0,
				w_full = true,
				boundary.screen(boundary.named("shell.boundary.screen.sidebar"), "@app/screens/shell/sidebar", function(Sidebar): Node
					return Sidebar { collapsed = collapsed :: any, limit = sidebar_limit, direct = sidebar_direct, top = TITLE_H }
				end),
				boundary.screen(boundary.named("shell.boundary.screen.page"), "@app/screens/shell/page", function(Page): Node
					return Page { files = files, width = window_w }
				end),
			},
			-- The rail, while the sidebar is collapsed: floating over the page at
			-- the window's bottom left. (Not mounted while `view.rail` is off.)
			if view.rail
				then ui.div {
					id = "shell-rail",
					absolute = true,
					left = ds.island.metrics().inset,
					bottom = ds.island.metrics().inset,
					boundary.screen(boundary.named("shell.boundary.screen.rail"), "@app/screens/shell/sidebar", function(Sidebar): Node
						return Sidebar { collapsed = collapsed :: any, island = true }
					end, { pass = true }),
				}
				else nil,
			boundary.screen(boundary.named("shell.boundary.screen.title_bar"), "@app/screens/shell/title_bar", function(TitleBar): Node
				return TitleBar { collapsed = collapsed :: any, files = files, art = art }
			end, { pass = true }),
		},
		-- A session's card, its row rested on (Zeron's hover card): over the
		-- window beside the row, one surface of the menus' glass blurring
		-- what's under it. (Not zgx's tooltip, whose popup frames it again
		-- and blurs nothing.)
		-- (An anchor at the window's corner, no box of its own: a layer the
		-- window's size, even one passing the pointer through, stands in
		-- front of everything for the pointer's cursor, and no resize edge
		-- or button showed its own.)
		ui.div {
			absolute = true,
			left = 0,
			top = 0,
			w = 0,
			h = 0,
			overflow = "visible",
			pointer_events = "none",
			hidden = in_settings,
			ui.presence(function(): string
				local sidebar: any = boundary.load("@app/screens/shell/sidebar")
				local c = sidebar and sidebar.card:get()
				return if c then c.id else ""
			end, function(id: string): Node
				local sidebar: any = boundary.load("@app/screens/shell/sidebar")
				local at = sidebar and sidebar.card:peek()
				if id == "" or not at then
					return ui.div {}
				end
				local function chat(): any
					for _, c in sessions.now:get().chats do
						if c.id == id then
							return c
						end
					end
					return nil
				end
				local function device(): (string?, boolean)
					local c = chat()
					local now = sessions.now:get()
					for _, d in now.devices do
						if c and d.id == c.device then
							return d.name, d.here or d.id == now.here
						end
					end
					return nil, false
				end
				local card = ui.ref()
				return ui.div {
					ref = card,
					absolute = true,
					left = at.x + at.w + 10,
					-- (Beside the row's top, kept inside the window.)
					top = function(): number
						local h = card.height or 0
						return math.max(8, math.min(at.y, (root.height or 0) - h - 8))
					end,
					ds.SessionCard {
						session = id,
						title = function(): string
							local c = chat()
							return if c then c.title else ""
						end,
						time = function(): number?
							local c = chat()
							return if c then c.updated else nil
						end,
						device = function(): string?
							return (device())
						end,
						here = function(): boolean
							local _, here = device()
							return here
						end,
						project = function(): string?
							local c = chat()
							if not c or not c.project then
								return nil
							end
							for _, p in sessions.now:get().projects do
								if p.id == c.project then
									return p.name
								end
							end
							return nil
						end,
						branch = function(): string?
							local c = chat()
							return if c then c.branch else nil
						end,
					},
				}
			end, {
				w_full = true,
				h_full = true,
				enter = motion.transition({ opacity = { 0, 1 }, duration = 140 }),
				exit = motion.transition({ opacity = 0, duration = 100 }),
			}),
		},
		-- Settings: a page of its own over the whole window, its own list of
		-- pages down its side and the way back to the chats at its head.
		-- (Esc goes back: taken here, so a field or a menu elsewhere keeps
		-- its own Esc.)
		ui.show(
			in_settings,
			ui.column {
				id = "shell-settings",
				-- Zeron's Settings are no plugin's to change or add to: what you
				-- allow plugins and templates is decided here (Settings →
				-- Plugins and Templates). Sealed, with all it holds.
				hook = false,
				w_full = true,
				h_full = true,
				focusable = true,
				focus = { outline = 0 },
				-- (Focus comes here as it shows: none left on a row of the chats
				-- now out of sight, where keys would go unseen.)
				on_mount = function(el)
					el:focus()
				end,
				on_key = function(e): boolean
					if e.key == "Escape" then
						route.leave_settings()
						return true
					end
					return false
				end,
				on_navigate = function(e)
					if e.direction == "back" then
						route.back()
					else
						route.forward()
					end
				end,
				boundary.screen(boundary.named("shell.boundary.screen.settings"), "@app/screens/shell/settings", function(settings): Node
					return settings.View()
				end),
			}
		),
		-- What's said for a moment (a plugin's toast, a notice): over
		-- everything, once for the window.
		ds.toast.Host {},
		-- A link's context menu (ds/menu_at: a span has no box to hang
		-- one from), once for the window.
		MenuAt.Host {},
		-- The engine's updates watched, whatever shows (C's
		-- screens/engine/update: an idle engine of this app's that's out of
		-- date restarted on the spot; it draws nothing).
		boundary.screen(boundary.named("shell.boundary.screen.update_watch"), "@app/screens/engine/update", function(update): Node
			return update.Watch()
		end, { pass = true }),
		-- The palette (⌘K, or the sidebar's Search): over everything, taking
		-- no room of its own.
		ui.div {
			absolute = true,
			left = 0,
			top = 0,
			boundary.screen(boundary.named("shell.boundary.screen.search"), "@app/screens/shell/palette", function(Palette): Node
				return Palette()
			end),
			-- New project's (⌘⇧N, its "New project"s).
			boundary.screen(boundary.named("shell.boundary.screen.new_project"), "@app/screens/shell/add_project", function(AddProject): Node
				return AddProject()
			end),
		},
	}
end
