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
local splits = require("@app/screens/shell/split")
local lanes = require("@app/screens/chat/lanes")
local LaneTab = require("@app/screens/chat/lane_tab")
local chat_shared = require("@app/screens/chat/shared")
local DropOverlay = require("@ds/drop_overlay")
local Dock = require("@app/screens/chat/dock")
local Glass = require("@ds/glass")

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
-- The page and its panel are cards set into the window under the
-- title bar, the window showing this much round and between them
-- (`zeron.inset.gap`, ds/tokens).
local INSET = (ui.theme.get("zeron.inset.gap") :: number?) or 4
-- A split pane's ×: its size.
local CLOSE = 22
-- Whether the panel's tabs show in groups (screens/panel sets it).
local panel_groups = ui.shared("panel.groups_shown", false)
-- How far up from a split pane's foot a click is on its composer.
local COMPOSER_ROOM = 130

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

	-- Split chats (screens/shell/split): the chats of the split showing laid
	-- out in the page, side by side and above one another, the window's gap
	-- between them; with none, the one chat (or a new chat, a plugin's page)
	-- fills it. The page's own card stands where the focused one is. (Each
	-- placed by edges that are a share of the page plus points: resizing
	-- the window or sliding the sidebar runs no script.)
	-- The chats in the page now: the split's, else the one open (so a chat
	-- dragged over it can split it); none off a session.
	local members = ui.computed(function(): { string }
		local list = splits.members:get()
		if #list > 0 then
			return list
		end
		local f = splits.focused:get()
		return if f then { f } else {}
	end)
	local grouped_now = ui.computed(function(): boolean
		return #members:get() >= 2
	end)
	local function grouped(): boolean
		return grouped_now:get()
	end
	-- A new chat with the sidebar closed: the page the whole window, as
	-- the single-page layout has it, no gap round it (its wallpaper to the
	-- window's edges).
	local is_flat = ui.computed(function(): boolean
		return home() and not on_plugin() and (shell_view.collapsed:get() or not fit.current:get().sidebar)
	end)
	-- The window's gap round the cards: none while the page is flat, eased
	-- there and back with the sidebar's own motion (at once, the page
	-- would step by the gap as the sidebar slides, and what's centred on it,
	-- the composer, would step and drift back). Eased by zgx itself, a tween
	-- bound to what shows it: no script runs as it moves.
	local gap_to = ui.computed(function(): number
		return if is_flat:get() then 0 else INSET
	end)
	local gap = motion.tween(gap_to, motion.sidebar)
	-- (The page under the card's head, where it stood in the window: the
	-- card is set down by the gap.)
	local below_head = motion.tween(ui.computed(function(): number
		return TITLE_H - gap_to:get()
	end), motion.sidebar)
	-- Whether the card has its corners (clipping the wallpaper) and its
	-- edge's room: as soon as it leaves the window's edges, and until the gap
	-- has closed going flat, so no square card shows in the gap. (Its fill
	-- and edge fade meanwhile: `card_shown`. Its timer is the effect's: going
	-- back before it's up keeps them.)
	local edged = ui.use_state(not is_flat:peek())
	ui.effect(function()
		local flat = is_flat:get()
		if not flat or motion.reduced_now() then
			edged:set(not flat)
			return
		end
		ui.after(motion.sidebar.duration or 0, function()
			edged:set(false)
		end)
	end)
	-- While a column beside the page moves, what's in the page keeps one
	-- width, so the page's content (a chat, its visuals, its composer; a
	-- split's panes) is laid out once, never at each frame, and nothing in
	-- it moves: the card, a box, follows the column's edge, clipping it or
	-- revealing room beside it.
	-- - The panel opening or closing: the page's width with the panel open
	--   at rest. Opening, it takes that width at once (the card's edge then
	--   meets it as the panel slides in); closing, it keeps it until the
	--   panel has gone. (`opening`: the page narrows by the panel's room at
	--   rest, as that settles.)
	-- - The sidebar's edge or the panel's dragged: the width it had as the
	--   drag began, until it's let go, then its own.
	-- (A window resized lays out live: its own edge would clip the page's
	-- text until it settled, and it lays out only what shows.) `since`
	-- counts the holds, so one taken over by another (a drag begun as the
	-- panel glides) takes its width afresh.
	local held = ui.use_state(nil :: { opening: boolean, since: number }?)
	local edge_dragging = ui.shared("shell.edge_dragging", false)
	-- The panel's room beside the page at rest: its width and the gap
	-- before it (known further down).
	local panel_room: () -> number = function(): number
		return 0
	end
	local function held_width(r: any, share: () -> number): () -> any
		local base: number? = nil
		local since: number? = nil
		return function(): any
			local h = held:get()
			if not h then
				base = nil
				return "100%"
			end
			if since ~= h.since then
				since = h.since
				base = nil
			end
			if base == nil then
				local el = r:peek()
				base = if el then el.width else nil
			end
			if base == nil then
				return "100%"
			end
			local delta = if h.opening then -panel_room() else 0
			return ui.raw(math.max(0, base + share() * delta), "the page's width held while a column beside it moves")
		end
	end
	local function whole(): number
		return 1
	end
	-- Where a chat's card stands in the page (alone, all of it).
	local function box_of(id: string?): any
		local b = if id then splits.frames:get()[id] else nil
		return b or splits.WHOLE
	end
	-- Whether a pane's top meets the page's top at its left end or its
	-- right (where the title bar's controls stand over it).
	local function at_top_left(id: string?): boolean
		return splits.top_left(box_of(id))
	end
	local function at_top_right(id: string?): boolean
		return splits.top_right(box_of(id))
	end
	-- Whether the session's panel is open (known further down).
	local panel_open: () -> boolean = function(): boolean
		return false
	end
	-- Where a pane's × stands from its card's right: clear of the title
	-- bar's panel toggle over the page's top right while the panel is shut.
	local function close_right(id: string?): number
		return if at_top_right(id) and not panel_open() then 34 else 8
	end

	local pane: ContainerProps = ds.wallpaper.follow(box, px, py, home)
	pane.ref = box
	pane.id = "shell-page"
	pane.hook = "shell.page"
	pane.min_w = 0
	pane.overflow = "hidden"
	-- (Alone, all of the page. In a split, over the focused chat's pane, as
	-- the panes are placed: a composer docked at its foot is at that pane's
	-- foot from the first frame, a launch included.)
	splits.place(pane, function(): any
		return if grouped() then box_of(splits.focused:get()) else splits.WHOLE
	end)
	-- (A card of its own, its corners clipping the wallpaper. In a split,
	-- every chat's transcript is its pane's own (each kept as the focus
	-- moves, so none is made again and none moves): this is only the frame
	-- of the window's composer, at the focused pane's foot, over that
	-- pane's card, letting the pointer through to it but for the composer.)
	-- (Flat, a new chat with the sidebar closed: no card, the window's.)
	local RADIUS = (function(): number
		local r: any = ui.theme.get("zeron.inset.radius")
		if type(r) ~= "number" then
			r = ui.theme.get("radius.xl")
		end
		return if type(r) == "number" then r else 12
	end)()
	-- (Its corners whole while there's any gap round it, square as it
	-- meets the window's edges: never between, where they'd show changing
	-- and not match the other corners round them.)
	pane.rounded = function(): number
		return ui.raw(if edged:get() then RADIUS else 0, "the card's corners, whole while any gap is round it")
	end
	-- (Its edge's room; the edge itself is drawn with the fill, below.)
	pane.border = function(): number
		return if edged:get() then 1 else 0
	end
	pane.border_color = "#00000000"
	-- The card's fill and edge, under all it holds: faded in and out with
	-- the sidebar's own motion as a new chat's page goes flat and back (the
	-- fill lighter than the window, lifting it), by a tween zgx runs, so no
	-- script runs and nothing is laid out as it fades, and nothing changes
	-- in one frame. None in a split: there the panes are the cards.
	local card_shown = motion.tween(ui.computed(function(): number
		return if grouped() or is_flat:get() then 0 else 1
	end), motion.sidebar)
	table.insert(pane, ui.div {
		id = "shell-page-card",
		absolute = true,
		-- (Over the card's edge's room too: the whole card.)
		left = ui.raw(-1, "over the card's 1-point edge"),
		right = ui.raw(-1, "over the card's 1-point edge"),
		top = ui.raw(-1, "over the card's 1-point edge"),
		bottom = ui.raw(-1, "over the card's 1-point edge"),
		pointer_events = "none",
		bg = "$zeron.inset.bg",
		border = 1,
		border_color = "$zeron.inset.edge",
		rounded = "$zeron.inset.radius",
		opacity = card_shown,
	})
	pane.pointer_events = function(): any
		return if grouped() then "none" else "auto"
	end
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
	local view_ref = ui.ref()
	table.insert(pane, ui.column {
		ref = view_ref,
		absolute = true,
		left = 0,
		w = held_width(view_ref, whole),
		-- (In a split, empty over the focused pane: the pointer goes to it.)
		pointer_events = function(): any
			return if grouped() then "none" else "auto"
		end,
		-- (The card is set down by the gap, so the page starts that much
		-- less far down it, where it did in the window.)
		top = below_head,
		bottom = 0,
		-- (Under a plugin's page, kept and unseen: it comes back as it was.)
		opacity = function(): number
			return if on_plugin() then 0 else 1
		end,
		-- (In a split, the panes show the chats: the chat page isn't made.)
		ui.show(function(): boolean
			return not grouped()
		end, view),
	})
	-- (In a split, the chat page isn't made: the window still watches the
	-- focused chat, for the composer and the focused pane's transcript.)
	local chat_store = require("@app/screens/chat/store")
	ui.effect(function()
		if grouped() then
			chat_store.open(splits.focused:get())
		end
	end)
	-- The one composer, over whatever shows (it hides itself on settings).
	local chat_screen = boundary.load("@app/screens/chat")
	if chat_screen and chat_screen.Composer then
		-- (Not while a new chat has nowhere to work: its page asks for a
		-- project instead, as Zeron's. The composer hides itself then: a
		-- `ui.show` round it would be a box over the whole pane, taking the
		-- pointer and the wheel from the page, as boundary's pass explains.)
		-- (At the page's width, held through the panel's glide: in a split,
		-- the focused pane's, by its share.)
		local composer_ref = ui.ref()
		local function page_share(): number
			if grouped() then
				local b = box_of(splits.focused:peek())
				return b.right.p - b.left.p
			end
			return 1
		end
		table.insert(pane, ui.div {
			ref = composer_ref,
			absolute = true,
			left = 0,
			top = 0,
			bottom = 0,
			w = held_width(composer_ref, page_share),
			pointer_events = "none",
			boundary.screen(boundary.named("shell.boundary.screen.composer"), "@app/screens/chat", function(c): Node
				return c.Composer()
			end, { pass = true }),
		})
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
	-- (The title line is the card's own head, level with the title
	-- bar's controls in the window: centred where they are, less the gap
	-- the card is set down by.)
	local title_mid = TITLE_TOP + (TITLE_H - TITLE_TOP) / 2 - INSET
	-- Where a pane's title starts in its card: past the title bar's
	-- controls when they stand over it (the sidebar collapsed, the pane at
	-- the inset's top left), else the page's inset.
	local function title_left(id: string?): number
		if at_top_left(id) and (shell_view.collapsed:get() or not fit.current:get().sidebar) then
			return ds.TitleBar.controls_end() + after_controls - INSET
		end
		return inset
	end
	table.insert(
		pane,
		ui.show(
			function(): boolean
				-- (Not over a plugin's page: it's a page of its own, not the chat.)
				-- (In a split, each pane's head says its own.)
				return has_chat:get() and not on_plugin() and not grouped()
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
				top = 0,
				h = 2 * title_mid,
				-- (Collapsed, the bar's controls stand over the card's start:
				-- past them, less the gap the card is set in by.)
				-- (In a split, only the leftmost pane's is under them.)
				left = motion.tween(ui.computed(function(): number
					return title_left(splits.focused:get())
				end), motion.sidebar),
				-- (Clear of the bar's own controls at its right; in a split,
				-- of its pane's ×.)
				right = function(): number
					if grouped() then
						return ui.raw(close_right(splits.focused:get()) + CLOSE + 6, "its pane's × at its right")
					end
					return ui.raw(TITLE_RIGHT, "the title bar's controls at the right, and their room")
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
				top = TITLE_H - INSET,
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
	-- (The card's own head, set down by the gap; the gap over the
	-- cards moves the window too, below.)
	-- (In a split, the panes' heads are there instead.)
	table.insert(pane, ui.show(function(): boolean
		return not grouped()
	end, ui.div { id = "page-drag", absolute = true, left = 0, right = 0, top = 0, h = below_head, window_drag = true }))

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
			-- (The row's room less the window round the cards; the
			-- page's card gone, there's no gap before the panel's.)
			local room: number = (row_ref.width or 0) - 2 * INSET
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
	-- (Whether the step under way opens the panel beside the page.)
	local step_opening = ui.use_state(nil :: boolean?)
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
		-- (Opening beside the page, not over it: the page narrows by it.)
		local opening = now.shown and not seen.shown and not now.expanded
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
				step_opening:set(opening)
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
	panel_room = function(): number
		return rest_w:get() + INSET
	end
	-- Which hold stands: the panel's glide's, else a drag's; none, the page's
	-- content its own width again (laid out once, its reading place kept:
	-- ds/transcript).
	local holds = 0
	ui.effect(function()
		local want: { opening: boolean }? = nil
		if revealing:get() and step_opening:get() ~= nil then
			want = { opening = step_opening:get() == true }
		elseif edge_dragging:get() or drag_held:get() ~= nil then
			want = { opening = false }
		end
		local now = held:peek()
		if want == nil then
			if now ~= nil then
				held:set(nil)
			end
		elseif now == nil or now.opening ~= want.opening then
			holds += 1
			held:set({ opening = want.opening, since = holds })
		end
	end)
	ui.effect(function()
		if not revealing:get() and step_opening:peek() ~= nil then
			step_opening:set(nil)
		end
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
	-- (The page's card goes, not to a sliver of its edge, while the
	-- panel stands over it.)
	pane.opacity = function(): number
		return if expanded() then 0 else 1
	end
	-- Whether any of it shows: none at all under 2 points. (Shut is shut:
	-- what's left of its width as it eases closed, a point or so, would
	-- stand at the window's right as a sliver.) And the gap before its card,
	-- closing with it. (Each worked out as its width eases, telling what
	-- shows them only when they change: its edge's, its fill's and its
	-- gap's bindings don't run on every frame of its glide.)
	local panel_seen = ui.computed(function(): boolean
		return open_w:get() >= 2
	end)
	local gap_before = ui.computed(function(): number
		if expanded() then
			return 0
		end
		return math.min(INSET, if panel_seen:get() then open_w:get() else 0)
	end)
	local beside = ui.column {
		id = "shell-files",
		hook = "shell.files",
		w = open_w,
		h_full = true,
		shrink = 0,
		overflow = "hidden",
		relative = true,
		-- (A card of its own beside the page's, the window between
		-- them as it opens; its hairline stronger under the pointer at its
		-- edge and as it's dragged. Shut, nothing of it shows.)
		-- (Expanded over the page, the page's card is gone: no gap before
		-- it, so it stands the window's gap in from the sidebar, as every
		-- card does.)
		ml = function(): number
			return ui.raw(gap_before:get(), "the gap before the panel's card, closing with it")
		end,
		-- (Its tabs split into groups, each a card of its own on the window
		-- as the chat panes are: its own card goes, but for its edge under
		-- the pointer.)
		bg = function(): string
			return if panel_groups:get() then "#00000000" else "$zeron.inset.bg"
		end,
		rounded = "$zeron.inset.radius",
		-- (Its groups' cards each have their edge: its own none, so they
		-- stand the window's gap from what's round them, as every card does,
		-- not its edge's width more.)
		-- (Shut, none either: an edge on a card of no width still stands its
		-- width at each side, a sliver at the window's right where a flat
		-- page meets it.)
		border = function(): number
			return if panel_groups:get() or not panel_seen:get() then 0 else 1
		end,
		border_color = function(): string
			if edge_over:get() or drag_held:get() ~= nil then
				return "$zeron.inset.edge_hover"
			end
			return "$zeron.inset.edge"
		end,
		opacity = function(): number
			return if panel_seen:get() then 1 else 0
		end,
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
						project = route.project,
						session = route.session,
						page = route.page,
						-- (Where its left edge is in the window: expanded, or with
						-- the sidebar collapsed, its tabs clear the title bar's
						-- controls there. Its card ends the gap short of
						-- the window's right.)
						left = function(): number
							return props.width() - INSET - open_w:get()
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
		-- (No hairline down its left; its card's edge is that.)
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
	panel_open = function(): boolean
		return shown:get()
	end

	-- Split chats: what stands over the page's panes. A chat dragged over a
	-- pane (a sidebar row, or another pane's head) lands at the edge it's
	-- held over: left or right, beside it; top or bottom, above or below it.
	-- A preview of where shows as it's held there; the pane's middle stays
	-- the chat page's own (a chat dropped there is mentioned).
	local over = ui.use_state(nil :: { id: string, side: string }?)
	-- (A drag that ended anywhere leaves no preview.)
	ui.effect(function()
		if not splits.dragging:get() and over:peek() ~= nil then
			over:set(nil)
		end
	end)
	-- Every chat of the split is watched by the host in a lane of its own,
	-- focused or not, for as long as it's in the split: each pane shows its
	-- own chat from its own lane, and the focus moving changes no pane's
	-- source. (Let go as it leaves. A reload asks for the same chats again,
	-- which the host already has: nothing more happens.)
	local watched: { [string]: boolean } = {}
	ui.effect(function()
		local want: { [string]: boolean } = {}
		if grouped() then
			for _, id in members:get() do
				want[id] = true
			end
		end
		for id in watched do
			if not want[id] then
				lanes.watch(`split-{id}`, nil)
			end
		end
		for id in want do
			if not watched[id] then
				lanes.watch(`split-{id}`, id)
			end
		end
		watched = want
	end)
	-- Where a chat dragged over a pane lands. Its edges split: a band a
	-- quarter of the pane deep (48 points at least) along each side, the
	-- sides running from its top (owning its top corners) down to its
	-- composer, the top and bottom between them, the bottom ending above the
	-- composer. Its middle and its composer attach it (the chat page's own
	-- drop: a chat mentioned). Each point is one's: the bands stand over
	-- the page, and nothing else does. (Shares and points: nothing here is
	-- worked out from the pane's size.)
	local BAND, LEAST = "25%", 48
	local ZONES: { [string]: { [string]: any } } = {
		top = { left = BAND, right = BAND, top = 0, h = BAND, min_h = LEAST },
		bottom = { left = BAND, right = BAND, h = BAND, min_h = LEAST },
		left = { left = 0, top = 0, w = BAND, min_w = LEAST },
		right = { right = 0, top = 0, w = BAND, min_w = LEAST },
	}
	-- The room a pane's composer takes at its foot: the window's own,
	-- measured, over the focused one; a pane's own at rest over another.
	local function composer_room(id: string): number
		local h = if splits.focused:get() == id then chat_shared.dock_h:get() else Dock.REST
		return Dock.GAP + h
	end
	local function zone(id: string, side: string): Node
		local props: { [any]: any } = table.clone(ZONES[side])
		props.absolute = true
		if side ~= "top" then
			props.bottom = function(): number
				return ui.raw(composer_room(id), "the pane's composer, which attaches")
			end
		end
		props.pointer_events = "auto"
		props.drop_when = function(payload: any): boolean
			local d = splits.dragged(payload)
			return d ~= nil and d ~= id and splits.can_take(d)
		end
		props.on_drag_over = function(_payload: any)
			local o = over:peek()
			if not o or o.id ~= id or o.side ~= side then
				over:set({ id = id, side = side })
			end
		end
		props.on_drag_leave = function()
			local o = over:peek()
			if o and o.id == id and o.side == side then
				over:set(nil)
			end
		end
		props.on_drop = function(payload: any)
			local d = splits.dragged(payload)
			over:set(nil)
			splits.dragging:set(false)
			if d then
				-- (A chat from the sidebar is focused as it lands; a pane
				-- moved leaves the focus where it was.)
				splits.drop(d, id, side, payload.split_pane == nil)
			end
		end
		return ui.div(props)
	end
	-- A pane's title, as the session's head has it (its agent's mark, its
	-- title), quieter: it isn't the one focused.
	local function pane_title(about: any): Node
		return ui.row {
			grow = 1,
			min_w = 0,
			h_full = true,
			gap = "$zeron.space.tight",
			items = "center",
			pointer_events = "none",
			ui.presence(function(): string
				return about:get().agent or ""
			end, function(mark: string): Node
				if mark == "" then
					return ui.div {}
				end
				return ui.div { shrink = 0, opacity = 0.7, ds.Mark { name = mark :: any, size = 14, bare = true } }
			end),
			ds.FadeText {
				function(): string
					return about:get().title
				end,
				grow = 1,
				min_w = 0,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.secondary",
			},
		}
	end
	-- Where a chat held over a pane's edge would land: that half of it.
	local HALF: { [string]: { [string]: any } } = {
		left = { left = 0, top = 0, bottom = 0, right = "50%" },
		right = { left = "50%", top = 0, bottom = 0, right = 0 },
		top = { left = 0, right = 0, top = 0, bottom = "50%" },
		bottom = { left = 0, right = 0, top = "50%", bottom = 0 },
	}
	-- One chat's place in the page. The focused one's card is the page's
	-- (`pane`, under these); each other's is its transcript, watched in a
	-- lane of its own, dimmed, a click on it focusing it. Every pane of a
	-- split has a head: dragged, it moves the pane; its × takes the chat
	-- out of the split.
	local function pane_item(id: string): Node
		local lane_slot = `split-{id}`
		local about = splits.chat(id)
		local function focused(): boolean
			return splits.focused:get() == id
		end
		local hovered = ui.use_state(false)
		local card_ref = ui.ref()
		local lane_ref = ui.ref()
		-- Files (and the files pane's, and a sidebar chat) carried over it,
		-- dropped anywhere on it, as on the chat page: attached to its draft
		-- (a chat, mentioned). Focused, the chat page's own drop, its
		-- overlay over this pane; not, focused first, then dropped, its
		-- overlay its own meanwhile.
		local files_zone = chat_shared.drop_zone(`split-{id}`)
		local carrying = ui.use_state(nil :: string?)
		local function after_focus(f: () -> ())
			carrying:set(nil)
			splits.focus(id)
			ui.after(0, f)
		end
		-- Its card: its own transcript, kept for as long as it's in the
		-- split, focused or not (so the focus moving makes none again, and
		-- none moves), and, while it isn't focused, a composer of its own
		-- showing its draft (screens/chat/lane_tab's `split`); focused, the
		-- window's composer stands at its foot instead. Every part of it
		-- takes the pointer as the chat page's does (its transcript
		-- scrolls); a press anywhere in it (heard as it comes up from what
		-- took it) focuses it, and a press on its composer gives the
		-- window's composer, its draft the same, the keys.
		local card = ui.column {
			ref = card_ref,
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			overflow = "hidden",
			-- (The focused pane lifted, the others sunk.)
			bg = function(): string
				return if focused() then "$zeron.inset.active" else "$zeron.inset.inactive"
			end,
			rounded = "$zeron.inset.radius",
			border = 1,
			border_color = function(): string
				return if focused() then "$zeron.inset.focus_edge" else "$zeron.inset.edge"
			end,
			on_hover = function(inside: boolean)
				hovered:set(inside)
			end,
			on_file_hover = function(inside: boolean)
				if focused() then
					files_zone.on_file_hover(inside)
				else
					carrying:set(if inside then "files" else nil)
				end
			end,
			on_file_drop = function(files: { any })
				if focused() then
					files_zone.on_file_drop(files)
				else
					after_focus(function()
						files_zone.on_file_drop(files)
					end)
				end
			end,
			drop_when = files_zone.drop_when,
			on_drag_over = function(payload: any, e: any)
				if focused() then
					files_zone.on_drag_over(payload, e)
				else
					carrying:set(if type(payload) == "table" and type(payload.session) == "string" then "session" else "files")
				end
			end,
			on_drag_leave = function(payload: any)
				if focused() then
					files_zone.on_drag_leave(payload)
				else
					carrying:set(nil)
				end
			end,
			on_drop = function(payload: any, e: any)
				if focused() then
					files_zone.on_drop(payload, e)
				else
					after_focus(function()
						files_zone.on_drop(payload, e)
					end)
				end
			end,
			on_press = function(e)
				if focused() then
					return
				end
				local on_composer = (e.local_y or 0) > (card_ref.height or 0) - COMPOSER_ROOM
				splits.focus(id)
				if on_composer then
					ui.after(0, function()
						chat_shared.focus:add(1)
					end)
				end
			end,
			-- (The keyboard landing in a pane that isn't focused (Tab into
			-- its composer): it's focused, and the window's composer, its
			-- chat's draft in it, takes the keys. Only the window's composer
			-- types: one writer for each chat's draft.)
			on_focus = function()
				if not focused() then
					splits.focus(id)
					ui.after(0, function()
						chat_shared.focus:add(1)
					end)
				end
			end,
			ui.column {
				ref = lane_ref,
				absolute = true,
				left = 0,
				-- (Its width held through the panel's glide, by its share of
				-- the page.)
				w = held_width(lane_ref, function(): number
					local b = box_of(id)
					return b.right.p - b.left.p
				end),
				top = TITLE_H - INSET,
				bottom = 0,
				LaneTab.Tab {
					tab = {
						id = lane_slot,
						kind = "side_chat",
						chat = id,
						title = "",
						data = { split = true, focused = focused },
					},
				},
			},
			-- Dimmed while another is focused, lighter under the pointer;
			-- it takes no pointer of its own.
			ui.div {
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				pointer_events = "none",
				bg = function(): string
					if focused() then
						return "#00000000"
					end
					return if hovered:get() then "$zeron.inset.dim_hover" else "$zeron.inset.dim"
				end,
			},
			-- What's carried over it, while it isn't focused (focused, the
			-- chat page's overlay stands over it).
			DropOverlay {
				id = `split-drop-{id}`,
				shown = function(): boolean
					return carrying:get() == "files"
				end,
			},
			DropOverlay {
				id = `split-drop-mention-{id}`,
				shown = function(): boolean
					return carrying:get() == "session"
				end,
				label = ui.t("chat.composer.drop_to_mention") :: any,
			},
		}
		local head = ui.row {
			absolute = true,
			top = 0,
			left = 0,
			right = 0,
			h = TITLE_H - INSET,
			pl = function(): number
				return ui.raw(title_left(id), "where its title starts, as the session's head")
			end,
			pr = function(): number
				return ui.raw(close_right(id), "its × clear of the title bar's panel toggle")
			end,
			gap = "$zeron.space.tight",
			items = "center",
			on_hover = function(inside: boolean)
				if not focused() then
					hovered:set(inside)
				end
			end,
			on_click = function()
				if not focused() then
					splits.focus(id)
				end
			end,
			on_drag = function(): any
				splits.dragging:set(true)
				-- (Its session too: dropped on another's middle or composer,
				-- it's mentioned there, as a sidebar row is.)
				return { split_pane = id, session = id }
			end,
			drag_preview = function(): Node
				local card_props: { [any]: any } = {
					w = 240,
					h = 30,
					px = "$space.3",
					gap = "$zeron.space.tight",
					items = "center",
					rounded = "$radius.md",
					color = "$text.primary",
					ds.FadeText {
						about:peek().title,
						grow = 1,
						min_w = 0,
						text_size = "$font.size.sm",
						weight = "$font.weight.medium",
					},
				}
				Glass.style(card_props, "menu")
				return ui.row(card_props)
			end,
			drag_preview_anchor = { 0.5, 0 },
			drag_preview_offset = { 0, 14 },
			on_drag_end = function()
				splits.dragging:set(false)
			end,
			pane_title(about),
			ui.div { grow = 1, pointer_events = "none" },
			ds.IconButton {
				label = "Close pane",
				tooltip = "Take this chat out of the split",
				tooltip_side = "bottom",
				size = "sm",
				tone = "quiet",
				w = CLOSE,
				h = CLOSE,
				rounded = "$radius.sm",
				on_click = function()
					splits.close(id)
				end,
				ds.Icon { name = "close", size = 12 },
			},
		}
		local item: { [any]: any } = {
			-- (`#split-pane-<session>`, `.split-pane`: a chat's pane, the
			-- session under it.)
			id = `split-pane-{id}`,
			class = "split-pane",
			provide = { session = id },
			pointer_events = "none",
			-- (The panel expanded over the page: none of the split shows.)
			opacity = function(): number
				return if expanded() then 0 else 1
			end,
			ui.show(grouped, card),
			ui.show(grouped, head),
		}
		splits.place(item, function(): any
			return box_of(id)
		end)
		return ui.column(item)
	end
	-- Where a chat dragged over a pane would land, over everything (the
	-- page's card included: a drop goes to what's on top), while a chat is
	-- dragged: its edges, each taking the drop, and the half it would
	-- take, lit as it's held there. Its middle is left to the chat page (a
	-- chat dropped there is mentioned).
	local function landing(id: string): Node
		local function held(): string
			local o = over:get()
			return if o and o.id == id then o.side else ""
		end
		local preview = ui.show(function(): boolean
			return held() ~= ""
		end, ui.div {
			id = "split-landing",
			absolute = true,
			left = function(): any
				return (HALF[held()] or HALF.left).left
			end,
			right = function(): any
				return (HALF[held()] or HALF.left).right
			end,
			top = function(): any
				return (HALF[held()] or HALF.left).top
			end,
			bottom = function(): any
				return (HALF[held()] or HALF.left).bottom
			end,
			pointer_events = "none",
			bg = "$zeron.inset.drop",
			border = 2,
			border_color = "$zeron.inset.drop_edge",
			rounded = "$zeron.inset.radius",
		})
		-- (The sides last: over the top's and bottom's ends, they own the
		-- corners.)
		local item: { [any]: any } = {
			pointer_events = "none",
			preview,
			zone(id, "top"),
			zone(id, "bottom"),
			zone(id, "left"),
			zone(id, "right"),
		}
		splits.place(item, function(): any
			return box_of(id)
		end)
		return ui.column(item)
	end
	-- The page's room: the page's card at the focused chat's place, the
	-- split's panes under it.
	local split_area = ui.row {
		grow = 1,
		min_w = 0,
		h_full = true,
		relative = true,
		-- (The split's panes, under the page's card: in a split, their cards
		-- are the chats, and the page's card is the composer's frame over
		-- the focused one's.)
		ui.div {
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			pointer_events = "none",
			ui.each(members, function(id: string): Node
				return pane_item(id)
			end, {
				key = function(id: string): string
					return id
				end,
				w_full = true,
				h_full = true,
				relative = true,
			}),
		},
		-- (What the page's card stands in, placed by its box in a split.)
		ui.column {
			grow = 1,
			min_w = 0,
			h_full = true,
			relative = true,
			-- (In a split, the frame of the window's composer alone, over the
			-- panes: the pointer goes through to them, but for the composer.)
			pointer_events = function(): any
				return if grouped() then "none" else "auto"
			end,
			ui.column(pane),
		},
		ui.show(function(): boolean
			return splits.dragging:get() and not expanded()
		end, ui.div {
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			pointer_events = "none",
			ui.each(members, landing, {
				key = function(id: string): string
					return id
				end,
				w_full = true,
				h_full = true,
				relative = true,
			}),
		}),
	}

	-- Two cards, the page and its panel, set in from the window's edges and
	-- the sidebar by the gap, the title bar's line inside them. The window
	-- round them, along the title bar's strip (over them, beside them and
	-- between them), moves the window as the bar does: a strip under the
	-- cards, which let the pointer through but for what takes it.
	return ui.column {
		grow = 1,
		min_w = 0,
		h_full = true,
		relative = true,
		ui.div { id = "shell-drag", absolute = true, left = 0, right = 0, top = 0, h = TITLE_H, window_drag = true },
		ui.row {
			ref = row_ref,
			w_full = true,
			grow = 1,
			min_h = 0,
			pt = gap,
			pb = gap,
			pl = gap,
			pr = gap,
			pointer_events = "none",
			split_area,
			beside,
		},
	}
end
