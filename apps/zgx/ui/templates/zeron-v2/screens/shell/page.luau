-- The main window's page, beside the sidebar: where the route is (a new
-- chat over its wallpaper, a session, settings), the window's one composer
-- over it, travelling between a new chat and a session (the chat page's),
-- and a session's files beside it (the files pane's), shown and hidden by
-- the title bar's toggle.
--
-- `Page { files = state, width = fn }`: `files` whether a session's files
-- show, `width` the window's (the wallpaper is laid at it, so it holds
-- still as the sidebar comes and goes).
--
-- The chat page and the files pane are other screens' (`@app/screens/chat`,
-- `@app/screens/files`), each loaded on its own: one that fails to load
-- shows what went wrong in its place, and the rest of the window works.

local ds = require("@ds")
local motion = require("@ds/motion")
local locale = require("@ds/locale")
local route = require("@app/screens/shell/route")
local shell_view = require("@app/screens/shell/view")
local fit = require("@app/screens/shell/fit")
local sessions = require("@account/sessions")
local appearance = require("@app/screens/shell/appearance")
local art = require("@app/screens/shell/art")
local boundary = require("@app/screens/shell/boundary")

export type Props = {
	files: State<boolean>,
	width: () -> number,
}

-- The title bar's height (Zeron's): the pages start under it, the
-- wallpaper runs under it to the window's top.
local TITLE_H = 38
-- The title line's own room: under the bar's 4 of top padding, its
-- controls centred 21 down; and what its right end keeps for them.
local TITLE_TOP, TITLE_RIGHT = 4, 64
-- How wide a session's files stand beside it: at least, and by default
-- (room for the tree beside a file, ds/files_pane's 288 + 360).
-- The panel's least (Zeron's surface host's: screens/shell/fit) and its
-- width until it's first split or dragged.
local FILES_MIN, FILES_W = fit.RIGHT_PANE_MIN, 660

return function(props: Props): Node
	local box = ui.ref()
	local px = ui.state("shell_wallpaper_px", 0.5)
	local py = ui.state("shell_wallpaper_py", 0.5)
	-- (The chat route: held while settings shows over the window, so what
	-- this page shows stays as it was, scroll and all, under it.)
	local function home(): boolean
		return route.chat:get().name == "home"
	end
	local function in_session(): boolean
		return route.chat:get().name == "session"
	end
	-- A plugin's page open: a page of its own, the chat's kept under it as
	-- it was but not seen (its wallpaper, its view, the composer).
	local function on_plugin(): boolean
		return route.current:get().name == "plugin"
	end

	-- What the chat route shows, under the title bar: a new chat, or a
	-- session (a page swap; another session is the same page, showing it,
	-- in place).
	local view = ui.presence(function(): string
		if route.chat:get().name == "plugin" then
			return "none"
		end
		return if in_session() then "session" else "home"
	end, function(kind: string): Node
		-- (A plugin's page is a route of its own: no chat under it.)
		if kind == "none" then
			return ui.div {}
		end
		if kind == "session" then
			return boundary.screen(boundary.named("shell.boundary.screen.session_page"), "@app/screens/chat", function(chat): Node
				return chat.Session()
			end)
		end
		return boundary.screen(boundary.named("shell.boundary.screen.new_chat_page"), "@app/screens/chat", function(chat): Node
			return chat.Home()
		end)
	end, { w_full = true, h_full = true })

	local pane: ContainerProps = ds.wallpaper.follow(box, px, py, home)
	pane.ref = box
	pane.id = "shell-page"
	pane.hook = "shell.page"
	pane.grow = 1
	pane.min_w = 0
	pane.h_full = true
	pane.relative = true
	pane.overflow = "hidden"
	-- A trackpad's swipe between pages (the mouse's back and forward too):
	-- the history, as the router's own view takes it.
	pane.on_navigate = function(e)
		if e.direction == "back" then
			route.back()
		else
			route.forward()
		end
	end
	-- Whether it shows, for what stands over it (screens/shell/art).
	ui.effect(function()
		local on = home() and not on_plugin() and appearance.showing:get() ~= "none"
		if art.showing:peek() ~= on then
			art.showing:set(on)
		end
	end)
	-- A new chat's wallpaper: your picture (Settings → Appearance), or what
	-- a plugin draws in `#new-chat-background` while it's used as the
	-- wallpaper (it replaces that box, and reads where the pointer is as
	-- `pointer`); Zeron's effect over either.
	table.insert(
		pane,
		ds.Wallpaper {
			effect = appearance.effect_now,
			shown = function(): boolean
				return home() and not on_plugin()
			end,
			width = props.width,
			key = function(): string
				local showing = appearance.showing:get()
				if showing == "image" then
					local image = appearance.image:get()
					-- (Framed again after Adjust: its place is in the key.)
					return if image then `image:{image.x}:{image.y}:{image.zoom}:{image.path}` else ""
				end
				return if showing == "none" then "" else showing
			end,
			content = function(key: string): Node
				local image = appearance.image:peek()
				if string.find(key, "^image:") and image then
					return ds.wallpaper.Picture { src = image.path, w = image.w, h = image.h, x = image.x, y = image.y, zoom = image.zoom }
				end
				-- Where a plugin draws: nothing of Zeron's own.
				return ui.div { id = "new-chat-background", absolute = true, left = 0, right = 0, top = 0, bottom = 0 }
			end,
			-- (A record, not two numbers: a plugin reading it follows it
			-- with `zeron.live`, never built again as the pointer moves.)
			provide = {
				pointer = ui.computed(function(): { x: number, y: number }
					return { x = px:get(), y = py:get() }
				end),
			},
		}
	)
	table.insert(pane, ui.column {
		absolute = true,
		left = 0,
		right = 0,
		top = TITLE_H,
		bottom = 0,
		-- (Under a plugin's page, kept and unseen: it comes back as it was.)
		opacity = function(): number
			return if on_plugin() then 0 else 1
		end,
		view,
	})
	-- The one composer, over whatever shows (it hides itself on settings).
	local chat_screen = boundary.load("@app/screens/chat")
	if chat_screen and chat_screen.Composer then
		-- (Not while a new chat has nowhere to work: its page asks for a
		-- project instead, as Zeron's. The composer hides itself then: a
		-- `ui.show` round it would be a box over the whole pane, taking the
		-- pointer and the wheel from the page, as boundary's pass explains.)
		table.insert(pane, boundary.screen(boundary.named("shell.boundary.screen.composer"), "@app/screens/chat", function(c): Node
			return c.Composer()
		end, { pass = true }))
	end
	-- Where plugins put something over the page (a banner, a tour).
	table.insert(pane, ui.div {}) --PERFOFF
	-- A session's name on the title line, as the shipping app has it: its
	-- agent's mark and its title, 16 in from the page's edge (past the
	-- bar's controls while the sidebar is collapsed), on the line the bar's
	-- controls centre on. Under the strip that moves the window: it takes
	-- no press of its own.
	-- (Looked for once a change, and what the line shows of it each its
	-- own: the title's bindings wake when the title changes, not whenever
	-- any session does.)
	local open_chat = ui.computed(function(): sessions.Chat?
		local s = route.session:get()
		if not s then
			return nil
		end
		for _, c in sessions.now:get().chats do
			if c.id == s.id then
				return c
			end
		end
		for _, c in sessions.now:get().archived or {} do
			if c.id == s.id then
				return c
			end
		end
		return nil
	end)
	local has_chat = ui.computed(function(): boolean
		return open_chat:get() ~= nil
	end)
	local chat_title = ui.computed(function(): string
		local c = open_chat:get()
		return if c then c.title else ""
	end)
	local chat_agent = ui.computed(function(): string
		local c = open_chat:get()
		return if c and c.agent then c.agent else ""
	end)
	local inset = (ui.theme.get("space.4") :: number?) or 16
	-- (Collapsed, past the bar's controls by as much as Zeron's title
	-- stands past its last control's glyph.)
	local after_controls = (ui.theme.get("space.1") :: number?) or 4
	-- Where the title starts: past the title bar's controls while they
	-- stand over it (the sidebar collapsed), else the page's inset.
	-- (`full`, `tracked`: as ds.TitleBar's `controls_end`.)
	local function title_left(full: boolean?, tracked: boolean?): number
		if shell_view.collapsed:get() or not fit.current:get().sidebar then
			return ds.TitleBar.controls_end(full, tracked) + after_controls
		end
		return inset
	end
	-- How far it moves with the controls as the window goes in or out of
	-- full screen (macOS hides the traffic lights there), kept by the title
	-- bar's `follow`: still while macOS animates, then sliding with them;
	-- eased as the sidebar's moves otherwise, as its `left` is.
	local function full_screen_shift(full: boolean?, tracked: boolean?): number
		return title_left(full, tracked) - title_left(false, tracked)
	end
	local header_ref = ui.ref()
	ds.TitleBar.follow(header_ref, {
		x = function(full: boolean): number
			return full_screen_shift(full)
		end,
	}, motion.sidebar)
	table.insert(
		pane,
		ui.show(
			function(): boolean
				-- (Not over a plugin's page: it's a page of its own, not the chat.)
				return has_chat:get() and not on_plugin()
			end,
			-- (`#session-header`: what plugins place on a session's head by,
			-- the session under them; `#page-title`, its name.)
			ui.row {
				id = "session-header",
				provide = {
					session = ui.computed(function(): string?
						local s = route.session:get()
						return if s then s.id else nil
					end),
				},
				absolute = true,
				-- (Centred on the bar's line: 21 down, or the window's own
				-- buttons' middle where they set it.)
				top = function(): number
					return ds.TitleBar.middle() - (TITLE_H - TITLE_TOP) / 2
				end,
				h = TITLE_H - TITLE_TOP,
				ref = header_ref,
				left = motion.tween(ui.computed(function(): number
					return title_left(false, true)
				end), motion.sidebar),
				x = full_screen_shift(ds.TitleBar.full_screen_now(), false),
				-- (Clear of the bar's own controls at its right, and of the
				-- window's buttons where they stand there; TITLE_RIGHT counts the
				-- bar's row padding, 10, which `actions_right` gives as it is.)
				right = function(): number
					return ui.raw(TITLE_RIGHT - 10 + ds.TitleBar.actions_right(), "the title bar's controls at the right, and their room")
				end,
				gap = "$zeron.space.tight",
				items = "center",
				pointer_events = "none",
				ui.row {
					id = "page-title",
					grow = 1,
					min_w = 0,
					h_full = true,
					gap = "$zeron.space.tight",
					items = "center",
					ui.presence(function(): string
						return chat_agent:get()
					end, function(mark: string): Node
						if mark == "" then
							return ui.div {}
						end
						return ui.div { shrink = 0, ds.Mark { name = mark :: any, size = 14, bare = true } }
					end),
					ds.FadeText {
						function(): string
							return chat_title:get()
						end,
						grow = 1,
						min_w = 0,
						text_size = "$font.size.sm",
						weight = "$font.weight.medium",
						color = "$zeron.title",
					},
				},
			}
		)
	)
	-- A plugin's page ("pages.add"; screens/plugins lists them in the
	-- sidebar): a page of its own, on the window's material as any page is;
	-- the chat's stays as it was under it, unseen (`on_plugin`). Its plugin
	-- fills it by selector (`.plugin-page`); until then, or with its plugin
	-- off, a quiet word. What's around it gives the page.
	local function plugin_page(key: string): Node
		local plugin, page = string.match(key, "^([^/]*)/(.*)$")
		local listed: any = nil
		for _, p in (ui.shared("plugins.pages", {} :: { any }):peek() or {}) :: { any } do
			if p.plugin == plugin and p.id == page then
				listed = p
			end
		end
		local title = if listed then listed.title else "Plugin"
		return ui.column {
			w_full = true,
			h_full = true,
			-- (The page, and its panel: what its plugin opens tabs in.)
			provide = { page = { id = page, title = title }, panel = { page = { plugin = plugin, id = page } } },
			ui.column {
				class = `plugin-page plugin-{plugin}`,
				w_full = true,
				h_full = true,
				ds.EmptyState {
					icon = "widget",
					size = "sm",
					fill = true,
					-- (Unlisted, a plugin's word for it: the plugin's own context
					-- keeps the English.)
					title = if listed then title else function(): string
						return locale.s("shell.page.plugin")
					end,
					body = ui.t("shell.page.plugin_off") :: any,
				},
			},
		}
	end
	table.insert(
		pane,
		ui.show(
			function(): boolean
				return route.current:get().name == "plugin"
			end,
			ui.column {
				id = "plugin-page-area",
				absolute = true,
				left = 0,
				right = 0,
				top = TITLE_H,
				bottom = 0,
				ui.presence(function(): string
					local c = route.current:get()
					return `{c.plugin or ""}/{c.page or ""}`
				end, plugin_page, { w_full = true, h_full = true }),
			}
		)
	)

	-- The title bar's strip over the page moves the window (a double click
	-- zooms it): over all of the page (the composer's room, the wallpaper),
	-- under the bar's controls, which keep their presses.
	table.insert(pane, ui.div { id = "page-drag", absolute = true, left = 0, right = 0, top = 0, h = TITLE_H, window_drag = true })

	-- A session's files, beside it: sliding in from the window's edge as
	-- the toggle opens them, at their own width (the pane keeps it as it
	-- slides, so nothing re-lays out on the way).
	-- Its docked tree's room while the window's fit shows Zeron's Files:
	-- the tree's own width, so that as the tree goes to float over the file
	-- the panel narrows by just that, and the file stays where it was.
	local TREE = fit.TREE
	-- The width it would take, its tree docked: as last split (opening, a
	-- neighbour opening or closing) or dragged. Kept across launches.
	local files_w = ui.persist("shell_files_width", FILES_W)
	-- Whether a width was chosen (its edge dragged, or keyed): from then on
	-- it opens at that width, as Zeron's does (its `right_pane_width`), the
	-- fit narrowing it only as it must. Until then each opening splits.
	local chosen = ui.persist("shell_panel_width_chosen", false)
	local from_x, from_w, resizing = 0, 0, false
	-- Its edge being dragged: whether its tree was docked at the press (kept
	-- until release: a drag across the line docks or floats it then, not as
	-- it goes). Its content follows the edge as it moves, filling the panel.
	-- (It used to keep its width from the press, which left it against the
	-- left with room beside it as the panel widened.) nil while not dragged.
	local drag_held = ui.use_state(nil :: { w: number, docked: boolean }?)
	-- The pointer over its edge (its hairline stronger, as the sidebar's).
	local edge_over = ui.use_state(false)
	-- (Open for this session, and the window has room for it: hidden for
	-- room, it stays open, and comes back as the window widens.)
	-- (A plugin's page has a panel of its own too: its plugin's tabs.)
	local shown = ui.computed(function(): boolean
		return props.files:get() and (in_session() or on_plugin()) and fit.current:get().right
	end)
	-- Where the sidebar settles in the window's fit (its width, or none).
	local function sidebar_settled(): number
		local f = fit.current:get()
		return if f.sidebar and not shell_view.collapsed:get() then f.sidebar_limit else 0
	end
	local function tree_part(): number
		return if fit.current:get().files then TREE else 0
	end
	-- (Expanded over the session (the panel's Expand): all of the page's
	-- room, the chat's column narrowing to nothing.)
	local row_ref = ui.ref()
	local function expanded(): boolean
		return shown:get() and shell_view.expanded:get()
	end
	-- A drag ends however it ends: its edge hidden (the panel closed or
	-- expanded mid-drag) never hears its release, and a drag left on
	-- would hold the panel's content and width where they were.
	ui.effect(function()
		if (not shown:get() or expanded()) and (resizing or drag_held:peek() ~= nil) then
			resizing = false
			drag_held:set(nil)
		end
	end)
	-- Its part beside the tree as it stands. A window resize moves it only
	-- when it must: narrowing, the chat gives up its room first, down to
	-- its least, and then this; widening, while the chat is at its least,
	-- this takes the room back first, up to what it would take, then the
	-- chat grows. A column the fit hides gives its room to the chat; one it
	-- shows takes the chat's, and this only gives way if it must. Nothing
	-- jumps as a column goes or comes.
	-- The tree's room inside `files_w`: a session's panel keeps it for its
	-- tree (docked, or floating with the panel narrowed by it); a plugin's
	-- page has no tree, so its panel is the whole width. (It took TREE off
	-- there too, opening a tree's width narrower than a session's, often
	-- under its least.)
	local function reserve(): number
		return if on_plugin() then 0 else TREE
	end
	local right_w = ui.state("shell_panel_right_now", math.max(files_w:peek() - reserve(), 0))
	local function right_pref(): number
		return math.max(files_w:get() - reserve(), 0)
	end
	local alloc = { vw = fit.viewport:peek(), side = sidebar_settled(), tree = tree_part(), pref = right_pref() }
	ui.effect(function()
		local vw, side, tree, pref = fit.viewport:get(), sidebar_settled(), tree_part(), right_pref()
		local was = alloc
		alloc = { vw = vw, side = side, tree = tree, pref = pref }
		-- (Hidden (another chat with none, a new chat), its width holds as
		-- it was: what it shows at again is worked out as it opens (`split`).
		-- Followed while hidden, the fit took its tree away there, and the
		-- least below pushed a width chosen narrow up to the least without
		-- a tree, so a chat with its panel open arrived wide, then eased
		-- back to its width.)
		if vw <= 0 or not shown:get() then
			return
		end
		local allowed = vw - side - tree - fit.CHAT_PANEL_MIN
		local r = right_w:peek()
		if pref ~= was.pref then
			-- Split or dragged: what it would take, as far as there's room.
			r = math.min(pref, allowed)
		elseif r > allowed then
			-- No room left for the chat's least: it gives way.
			r = allowed
		elseif vw > was.vw and r < pref then
			-- Widening while it gave way: back first, the chat at its least.
			local was_allowed = was.vw - was.side - was.tree - fit.CHAT_PANEL_MIN
			if r >= was_allowed - 0.5 then
				r = math.min(pref, allowed)
			end
		elseif side < was.side and r < pref then
			-- The sidebar gone that it gave way to: back to its own width, as
			-- far as there's room. (Not its tree floating: the file holds.)
			r = math.min(pref, allowed)
		end
		-- Never under the panel's least as it shows: a width dragged narrow
		-- beside a docked tree (the tree floating over the file) was left
		-- as it was when the fit then took the tree away for room, so the
		-- panel stood a tree's width under its least.
		r = math.max(r, math.min(fit.RIGHT_PANE_MIN - tree, allowed))
		r = math.max(r, 0)
		if r ~= right_w:peek() then
			right_w:set(r)
		end
	end)
	-- Its tree docked beside the file: while the window's fit has room for
	-- it, and the panel, as it rests, leaves the file beside it its least
	-- (ds/files_pane's `zeron.files.view`). A panel dragged narrower floats
	-- its tree over the file, the panel as wide as it was; it docks again a
	-- little past that least, so an edge dragged about it doesn't flicker.
	local VIEW = (ui.theme.get("zeron.files.view") :: number?) or fit.RIGHT_PANE_MIN
	local AGAIN = 24
	local floated_narrow = false
	local tree_docked = ui.computed(function(): boolean
		local held = drag_held:get()
		if held then
			return held.docked
		end
		if not fit.current:get().files then
			floated_narrow = false
			return false
		end
		local least = if floated_narrow then VIEW + AGAIN else VIEW
		local docked = right_w:get() >= least - 0.5
		floated_narrow = not docked
		return docked
	end)
	-- The width it rests at, open; closing, the last it rested at (its
	-- content keeps that as its edge clips it).
	local held_rest: number? = nil
	local rest_w = ui.computed(function(): number
		if shown:get() then
			local r = tree_part() + right_w:get()
			held_rest = r
			return r
		end
		return held_rest or files_w:get()
	end)
	local function target(): number
		-- (The page's room read only while expanded: otherwise the sidebar
		-- moving would wake this every frame.)
		if expanded() then
			local room: number = row_ref.width or 0
			if room > 0 then
				return room
			end
		end
		return if shown:get() then rest_w:get() else 0
	end
	local open_w = ui.state("shell_files_shown_width", target())
	-- Opening or closing (the edge revealing what stands at its width):
	-- its content keeps the width it rests at. Otherwise it fills the panel.
	-- (The build's own, as the animation is: a reload mid-step starts the
	-- step again from where it shows, never counting what's left twice.)
	local revealing = ui.use_state(false)
	-- Its width is where it's going (`base`, which a window resize moves at
	-- once) and, after a step (opened, closed, expanded, a column going or
	-- coming beside it, a neighbour's share), what's left of that step
	-- (`off`), easing to nothing on its own clock (Zeron's column motion,
	-- ds/motion). So a step glides whatever the window does meanwhile, and
	-- is over in its time; nothing lags the window's edge.
	local base = open_w:peek()
	local off = ui.use_state(0)
	local easing: any = nil
	-- Whose panel shows (the session's, a plugin page's; none on a new
	-- chat): another one is the route moving, not the person opening or
	-- closing this one, and the panel lands at once, as it is there.
	local function owner(peek: boolean): string
		local c: any = if peek then route.chat:peek() else route.chat:get()
		if c.name == "plugin" then
			return `page:{c.plugin or ""}/{c.page or ""}`
		end
		return if c.name == "session" then `chat:{c.session or ""}` else c.name or ""
	end
	local function now_of(): { [string]: any }
		local f = fit.current:peek()
		return {
			vw = fit.viewport:peek(),
			shown = shown:peek(),
			expanded = expanded(),
			sidebar = f.sidebar and not shell_view.collapsed:peek(),
			files = f.files,
			owner = owner(true),
		}
	end
	local seen: { [string]: any } = now_of()
	ui.effect(function()
		local f = fit.current:get()
		local now: { [string]: any } = {
			vw = fit.viewport:get(),
			shown = shown:get(),
			expanded = expanded(),
			sidebar = f.sidebar and not shell_view.collapsed:get(),
			files = f.files,
			owner = owner(false),
		}
		local to = target()
		local opened = now.shown ~= seen.shown or now.expanded ~= seen.expanded
		local flipped = opened or now.sidebar ~= seen.sidebar or now.files ~= seen.files
		local resized = now.vw ~= seen.vw
		local moved = now.owner ~= seen.owner
		seen = now
		if to == base and not moved then
			return
		end
		if moved then
			-- Another session (or a new chat, a plugin's page): the panel as
			-- it is there, at once: shown at its width or gone, no step.
			if easing then
				easing:cancel()
				easing = nil
			end
			revealing:set(false)
			off:set(0)
			base = to
		elseif resizing then
			-- Its edge dragged: under the pointer, no step left.
			if easing then
				easing:cancel()
				easing = nil
			end
			off:set(0)
			base = to
		elseif flipped or not (resized or (now.expanded and not opened)) then
			-- A step: from where it shows now, gliding to where it goes.
			local showing = base + off:peek()
			base = to
			if easing then
				easing:cancel()
				easing = nil
			end
			if opened then
				revealing:set(true)
			end
			off:set(showing - to)
			local params = motion.column(off:peek(), 0)
			local this: any = nil
			params.onComplete = function()
				if easing == this then
					easing = nil
					revealing:set(false)
				end
			end
			this = anime.animate(off, params)
			easing = this
		else
			-- The window resized (or the room it fills while expanded moved):
			-- where it goes moves at once; a step under way goes on.
			base = to
		end
		open_w:set(base + off:peek())
	end)
	-- (Following the step as it eases.)
	ui.effect(function()
		local o = off:get()
		local w = base + o
		if open_w:peek() ~= w then
			open_w:set(w)
		end
	end)
	-- Its share, as Zeron's: its tree's allowance and half of what the
	-- sidebar and the tree leave the chat and it, as it opens. A dragged
	-- edge holds until then. (Not as the sidebar opens or closes, as Zeron
	-- does: halving it then squeezed a panel the chat had room beside, its
	-- tree going to float. It gives way only as it must, above, and comes
	-- back as the sidebar goes.)
	local split = { on = shown:peek() }
	ui.effect(function()
		local on = shown:get()
		local changed = on ~= split.on
		split = { on = on }
		local vw = fit.viewport:peek()
		if changed and on and not shell_view.expanded:peek() and vw > 0 then
			local side = sidebar_settled()
			if not chosen:peek() then
				local keep = reserve()
				files_w:set(keep + fit.content_split(math.max(vw - side - keep, 0)))
			else
				-- (The width chosen, as far as there's room: not what it had
				-- given way to before it closed.)
				local allowed = vw - side - tree_part() - fit.CHAT_PANEL_MIN
				local r = math.max(math.min(math.max(right_pref(), fit.RIGHT_PANE_MIN - tree_part()), allowed), 0)
				if r ~= right_w:peek() then
					right_w:set(r)
				end
			end
		end
	end)
	-- The panel's content width: opening or closing, the width it rests at
	-- (its edge reveals it, its strip held: see `hold`); else the panel's
	-- own, filling it as it moves (a column going or coming beside it, its
	-- tree going to float: the file stays where it was as the tree slides
	-- away).
	local function inner_w(): number
		if revealing:get() then
			return math.max(rest_w:get(), open_w:get())
		end
		return open_w:get()
	end
	-- Its width asked for (its edge dragged, or keyed): never past what
	-- leaves the chat its least; kept as the width it takes with its tree
	-- docked.
	local function set_width(want: number)
		local tree = if fit.current:peek().files then TREE else 0
		local most = math.max(FILES_MIN, fit.viewport:peek() - sidebar_settled() - fit.CHAT_PANEL_MIN)
		local w = math.clamp(want, FILES_MIN, most)
		files_w:set(reserve() + w - tree)
		if not chosen:peek() then
			chosen:set(true)
		end
	end
	local beside = ui.column {
		id = "shell-files",
		hook = "shell.files",
		w = open_w,
		h_full = true,
		shrink = 0,
		overflow = "hidden",
		relative = true,
		-- (From the window's top: the panel's own band is the title strip
		-- over it, its tabs in it, a window-drag region of its own, room
		-- left at its right for the title bar's Panel toggle.)
		ui.column {
			-- (Its own width, or, expanding, the one it grows to.)
			w = inner_w,
			h_full = true,
			-- The session's panel (its tabs: the files, a terminal), or, until
			-- that screen is there, the files pane alone.
			if boundary.load("@app/screens/panel")
				then boundary.screen(boundary.named("shell.boundary.screen.panel"), "@app/screens/panel", function(panel): Node
					return panel.Panel {
						visible = function(): boolean
							return shown:get()
						end,
						project = route.project,
						session = route.session,
						page = route.page,
						-- (Where its left edge is in the window: expanded, or with
						-- the sidebar collapsed, its tabs clear the title bar's
						-- controls there.)
						left = function(): number
							return props.width() - open_w:get()
						end,
						-- (Its strip held where it stands open as the panel slides,
						-- its right end against the title bar's controls: what shows
						-- of the panel is less than its content's width.)
						hold = function(): number
							return math.min(0, open_w:get() - inner_w())
						end,
						-- (Its file tabs' trees docked beside the file, or floating
						-- over it: `tree_docked`.)
						docked = function(): boolean
							return tree_docked:get()
						end,
					}
				end)
				else boundary.screen(boundary.named("shell.boundary.screen.files"), "@app/screens/files", function(files): Node
					return files.Pane { project = route.project, session = route.session }
				end),
		},
-- A hairline down its left, between it and the session, full height
		-- (as the sidebar's at its right).
		-- (Not expanded: its edge is then the sidebar's, which has its own.)
		-- (Stronger under the pointer at its edge and as it's dragged, as the
		-- sidebar's.)
		ui.div { absolute = true, left = 0, top = 0, bottom = 0, w = 1, bg = function(): string
			if expanded() then
				return "#00000000"
			end
			return if edge_over:get() or drag_held:get() ~= nil then "$border.strong" else "$border.subtle"
		end, pointer_events = "none" },
		-- Its edge, dragged: wider or narrower (the page keeps its least).
		-- (Pressed itself, so it keeps the pointer as it's dragged. Not while
		-- expanded: it's as wide as it goes.)
		ui.div {
			absolute = true,
			left = 0,
			top = 0,
			bottom = 0,
			w = function(): number
				return if expanded() then 0 else 6
			end,
			cursor = "col_resize",
			label = ui.t("shell.page.resize_files"),
			on_hover = function(inside: boolean)
				edge_over:set(inside)
			end,
			-- (From the keys too: ← wider, → narrower, a step each, ⇧ four.
			-- To assistive technology a slider of its width (zgx has no
			-- separator role); what it's set to is kept within the chat's
			-- least, as a drag is.)
			role = "slider",
			min = FILES_MIN,
			max = 8192,
			step = 16,
			value = function(): number
				return open_w:get()
			end,
			value_text = function(): string
				return `{math.floor(open_w:get())} points wide`
			end,
			on_change = function(v: number)
				set_width(v)
			end,
			focusable = true,
			-- (Only while there's an edge to move: closed or expanded, it's
			-- not there, and Tab passes it by.)
			hidden = function(): boolean
				return not shown:get() or expanded()
			end,
			-- (Focused from the keys: the edge itself, strong.)
			focus = { outline = 0, bg = "$border.strong" },
			on_key = function(e): boolean
				if e.key ~= "ArrowLeft" and e.key ~= "ArrowRight" then
					return false
				end
				local by = (if e.shift then 64 else 16) * (if e.key == "ArrowLeft" then 1 else -1)
				set_width(open_w:peek() + by)
				return true
			end,
			on_press = function(e)
				-- (From where it stands, its tree docked or floating.)
				from_x, from_w, resizing = e.x, open_w:peek(), true
				drag_held:set({ w = inner_w(), docked = tree_docked:peek() })
			end,
			on_pointer_move = function(e)
				if resizing then
					set_width(from_w - (e.x - from_x))
				end
			end,
			on_release = function()
				resizing = false
				drag_held:set(nil)
			end,
		},
	}
	return ui.row {
		ref = row_ref,
		grow = 1,
		min_w = 0,
		h_full = true,
		ui.column(pane),
		beside,
	}
end
