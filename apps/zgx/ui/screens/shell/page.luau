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
local seam = require("@ds/seam")
local locale = require("@ds/locale")
local route = require("@app/screens/shell/route")
local shell_view = require("@app/screens/shell/view")
local fit = require("@app/screens/shell/fit")
local sessions = require("@account/sessions")
local appearance = require("@app/screens/shell/appearance")
local art = require("@app/screens/shell/art")
local boundary = require("@app/screens/shell/boundary")
local updates = require("@app/screens/shell/updates")
local splits = require("@app/screens/shell/split")
local session_tabs = require("@app/screens/shell/session_tabs")
local lanes = require("@app/screens/chat/lanes")
local LaneTab = require("@app/screens/chat/lane_tab")
local chat_shared = require("@app/screens/chat/shared")
local DropOverlay = require("@ds/drop_overlay")
local Dock = require("@app/screens/chat/dock")
local control = require("@ds/control")
local Glass = require("@ds/glass")

export type Props = {
	files: State<boolean>,
	width: () -> number,
	-- Settings stand where the page and its panel do: only the top bar's left.
	away: (() -> boolean)?,
	settings: (() -> Node)?,
}

-- The title bar's height (Zeron's): the pages start under it, the
-- wallpaper runs under it to the window's top.
local TITLE_H = 38
-- The top bar over the cards: a tab on the bar's line, the window's gap over
-- and under it.
local TOP_BAR = 42
local WINDOW_EDGE = 5
-- The window's gap before the sidebar's card, part of the sidebar's move
-- (screens/shell/sidebar `before`).
local SIDEBAR_GAP = 5
-- The panel's Expand on the top bar (the bar's controls' size).
local EXPAND = 24
-- What the title line's right end keeps for the bar's controls there. (The
-- line itself is the bar's: `ds.TitleBar.middle`.)
local TITLE_RIGHT = 64
-- Of that room, what lies past the controls' row (ds/title_bar's ROW_PAD),
-- which `TitleBar.actions_right` gives as it is now.
local TITLE_ROW_PAD = 10
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
-- How far up from a split pane's foot a click is on its composer.
local COMPOSER_ROOM = 130
-- What the page wants beside the columns: its panes, each at its least.
fit.page_holds(function(): number
	return fit.floor_of(splits.across:get(), splits.GAP)
end)

return function(props: Props): Node
	local function away(): boolean
		return props.away ~= nil and props.away()
	end
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
	-- Where a chat's card stands in the page (alone, all of it).
	local function box_of(id: string?): any
		local b = if id then splits.shown_frames:get()[id] else nil
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
	-- What the panel takes beside the page's card, its gap included (known
	-- further down): the wallpaper holds still as it opens and closes.
	local panel_beside: () -> number = function(): number
		return 0
	end
	-- Where a pane's × stands from its card's right: clear of the title
	-- bar's panel toggle over the page's top right while the panel is shut
	-- (10 in and 24 wide; past the window's own buttons and the divider
	-- where they stand at the right: `TitleBar.actions_right`).
	local function close_right(id: string?): number
		return 8
	end

	-- (Where the pointer is, only while a wallpaper reads it: a plugin's.
	-- Your picture, or none, never does, and its writes on every move
	-- went nowhere.)
	local pane: ContainerProps = ds.wallpaper.follow(box, px, py, function(): boolean
		return home() and string.find(appearance.showing:peek(), "^plugin:") ~= nil
	end)
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
	-- (Whole corners: the cards' token, which follows the window's own corner
	-- where the host says it: ds/tokens.)
	pane.rounded = "$zeron.inset.radius"
	-- (Its edge's room; the edge itself is drawn with the fill, below.)
	pane.border = 1
	pane.border_color = "#00000000"
	-- The card's fill and edge, under all it holds (the fill lighter than the
	-- window, lifting it), faded by a tween zgx runs as a split starts and ends,
	-- so no script runs and nothing is laid out as it fades. None in a split:
	-- there the panes are the cards.
	local card_shown = motion.tween(ui.computed(function(): number
		return if grouped() then 0 else 1
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
	-- No `on_navigate`: a trackpad's swipe over the page scrolls and never
	-- steps the history (that's the title bar's arrows, the mouse's buttons,
	-- ⌘[ and ⌘]).
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
			-- (Laid at the window's width, as a state zgx follows: a resize
			-- runs no script here.)
			width = fit.viewport,
			beside = function(): number
				return panel_beside()
			end,
			picture = function(): any
				return if appearance.showing:peek() == "image" then appearance.image:peek() else nil
			end,
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
		-- (In a split, empty over the focused pane: the pointer goes to it.)
		pointer_events = function(): any
			return if grouped() then "none" else "auto"
		end,
		top = 0,
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
	-- (The transcript at the window's composer's foot, whose "Jump to
	-- latest" its pill pairs with: the chat page's, or the focused pane's.
	-- Its lane is `split-<chat>`: the panes', below.)
	ui.effect(function()
		local key = if grouped() then `lane.split-{splits.focused:get()}` else "page"
		if chat_shared.at_foot:peek() ~= key then
			chat_shared.at_foot:set(key)
		end
	end)
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
	local inset = (ui.theme.get("space.4") :: number?) or 16
	-- (Collapsed, past the bar's controls by as much as Zeron's title
	-- stands past its last control's glyph.)
	-- Where a pane's title starts in its card: past the title bar's
	-- controls when they stand over it (the sidebar collapsed, the pane at
	-- the inset's top left), else the page's inset.
	-- (`full`, `tracked`: as ds.TitleBar's `controls_end`, the bar's
	-- controls in full screen or out of it.)
	local function title_left(id: string?, full: boolean?, tracked: boolean?): number
		return inset
	end
	-- How far a title past the bar's controls moves with them as the window
	-- goes in or out of full screen (macOS hides the traffic lights there).
	local function full_screen_shift(id: string?, full: boolean?, tracked: boolean?): number
		return title_left(id, full, tracked) - title_left(id, false, tracked)
	end
	-- (Kept by the title bar's `follow`: gliding with its controls as the
	-- window goes in or out of full screen, eased as the sidebar's moves
	-- otherwise, as its `left` is.)
	local header_ref = ui.ref()
	ds.TitleBar.follow(header_ref, {
		x = function(full: boolean): number
			return full_screen_shift(splits.focused:get(), full)
		end,
	}, motion.sidebar)
	-- A chat being dragged: what follows the pointer.
	local function head_preview(id: string?, card: any): Node
		local el = if card then card:peek() else nil
		local cw, ch = if el then el.width or 0 else 0, if el then el.height or 0 else 0
		local w = math.clamp(math.floor(cw * 0.22), 140, 320)
		local h = if cw > 0 then math.floor(w * ch / cw) else 90
		if h > 220 then
			w, h = math.floor(w * 220 / h), 220
		end
		local picture: string? = if el then el:snapshot(math.ceil(w * 2)) else nil
		local title = ""
		for _, c in sessions.now:peek().chats do
			if c.id == id then
				title = c.title
			end
		end
		local card_props: { [any]: any } = {
			w = w,
			h = h,
			overflow = "hidden",
			items = "center",
			justify = "center",
			px = "$space.3",
			color = "$text.primary",
			if picture
				then ui.image { src = picture, absolute = true, left = 0, top = 0, w = w, h = h, fit = "cover" }
				else ds.FadeText { title, min_w = 0, text_size = "$font.size.sm", weight = "$font.weight.medium" },
		}
		Glass.style(card_props, "menu")
		card_props.rounded = "$radius.md"
		return ui.row(card_props)
	end
	-- What a pane is dragged by: two rows of dots on the composer's fill, centred at
	-- the pane's top, there while the pointer is near it and the pane has
	-- somewhere to go (beside another, or into another tab). While dragged, only
	-- the dragged pane's. (Unseen, not gone: hiding the source would end the drag.)
	local function pane_grip(chat: () -> string?, near: () -> boolean, card: any): Node
		local held = ui.use_state(false)
		local function show(): boolean
			if splits.dragging:get() then
				return held:get()
			end
			return near() and (#splits.members:get() >= 2 or #splits.tabs:get() > 1)
		end
		local face: { [any]: any } = {
			class = "pane-grip",
			w = 26,
			h = 16,
			items = "center",
			justify = "center",
			color = "$text.secondary",
			rounded = "$radius.md",
			bg = function(): string
				return if ds.material.see_through() then "$zeron.composer.tint" else "$zeron.composer.solid"
			end,
			backdrop_blur = function(): number
				return if ds.material.see_through() then 16 else 0
			end,
			pointer_events = "auto",
			cursor = function(): string
				return if held:get() then "grabbing" else "grab"
			end,
			opacity = ui.tween(ui.computed(function(): number
				return if show() then 1 else 0
			end), { duration = 160 }),
			on_drag = function(): any
				local id = chat()
				if not id then
					return nil
				end
				held:set(true)
				splits.dragging:set(true)
				return { split_pane = id, session = id }
			end,
			drag_preview = function(): Node
				return head_preview(chat(), card)
			end,
			drag_preview_anchor = { 0.5, 0 },
			drag_preview_offset = { 0, 18 },
			on_drag_end = function()
				held:set(false)
				splits.dragging:set(false)
			end,
			ds.Icon { name = "grip-horizontal", size = (ui.theme.get("zeron.menu.glyph.size") :: number?) or 16 },
		}
		return ui.row {
			absolute = true,
			top = ui.raw(6, "off the card's top"),
			left = 0,
			right = 0,
			justify = "center",
			pointer_events = "none",
			hidden = function(): boolean
				return not show()
			end,
			ui.column(face),
		}
	end
	-- Whether a pointer moving over a pane is near enough its top for the grip.
	local NEAR_TOP = 32
	local function tracks_near(props: { [any]: any }, near: any)
		local was_move = props.on_pointer_move
		props.on_pointer_move = function(e: any)
			if was_move then
				was_move(e)
			end
			local now = e.local_y < NEAR_TOP
			if near:peek() ~= now then
				near:set(now)
			end
		end
		local was = props.on_hover
		props.on_hover = function(inside: boolean)
			if was then
				was(inside)
			end
			if not inside then
				near:set(false)
			end
		end
	end
	-- A lone chat's grip: near its card's top.
	local lone_near = ui.use_state(false)
	tracks_near(pane, lone_near)
	table.insert(pane, ui.show(function(): boolean
		return has_chat:get() and not on_plugin() and not grouped()
	end, pane_grip(function(): string?
		local s = route.session:peek()
		return if s then s.id else nil
	end, function(): boolean
		return lone_near:get()
	end, box)))
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
				h = Dock.head(),
				-- (Collapsed, the bar's controls stand over the card's start:
				-- past them, less the gap the card is set in by.)
				-- (In a split, only the leftmost pane's is under them.)
				-- (Out of full screen's place, eased as the sidebar comes and
				-- goes; full screen's shift on top, gliding with the bar's
				-- controls as the window goes in or out.)
				ref = header_ref,
				left = motion.tween(ui.computed(function(): number
					return title_left(splits.focused:get(), false, true)
				end), motion.sidebar),
				x = full_screen_shift(splits.focused:peek(), ds.TitleBar.full_screen_now(), false),
				-- (Clear of the bar's own controls at its right, and of the
				-- window's buttons where they stand there; in a split, of its
				-- pane's ×.)
				right = function(): number
					if grouped() then
						return ui.raw(close_right(splits.focused:get()) + CLOSE + 6, "its pane's × at its right")
					end
					return ui.raw(TITLE_RIGHT - TITLE_ROW_PAD + ds.TitleBar.actions_right(), "the title bar's controls at the right, and their room")
				end,
				gap = "$zeron.space.tight",
				items = "center",
				pointer_events = "none",
				ui.row {
					id = "page-title",
					-- (The session's tab on the top bar says its name.)
					hidden = true,
					ds.FadeText {
						function(): string
							return chat_title:get()
						end,
						text_size = "$font.size.sm",
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

	-- A session's files, beside it: its card growing in from the window's
	-- edge as the toggle opens them, what's in it standing still at its own width as the card's edge
	-- uncovers it (nothing re-lays out on the way).
	-- Its docked tree's room while the window's fit shows Zeron's Files:
	-- the tree's own width, so that as the tree goes to float over the file
	-- the panel narrows by just that, and the file stays where it was.
	local TREE = fit.TREE
	-- The width it would take, its tree docked: as last split (opening, a
	-- neighbour opening or closing) or dragged. Kept across launches.
	local files_w = ui.persist("shell_files_width", FILES_W)
	-- Whether a width was chosen (its edge dragged, or keyed) since it was
	-- last opened or closed: until its toggle, it holds that width (another
	-- chat's panel, a relaunch), the fit narrowing it only as it must. Its
	-- toggle forgets it, as Zeron's does: it opens at its share again, and
	-- keeps to its share as the window is resized.
	local chosen = ui.persist("shell_panel_width_chosen", false)
	local from_x, from_w, resizing = 0, 0, false
	-- Its edge being dragged: whether its tree was docked at the press (kept
	-- until release: a drag across the line docks or floats it then, not as
	-- it goes). Its content follows the edge as it moves, filling the panel.
	-- (It used to keep its width from the press, which left it against the
	-- left with room beside it as the panel widened.) nil while not dragged.
	local drag_held = ui.use_state(nil :: { w: number }?)
	-- The pointer over its edge (its hairline stronger, as the sidebar's).
	local edge_over = ui.use_state(false)
	-- Its edge's drag: a bounce past its least or most, added to its width.
	local edge_drag = seam.drag()
	-- Whether a step (opening, closing, expanding, a column beside it going
	-- or coming) is under way: its edge is out of reach until it lands.
	local stepping = ui.use_state(false)
	-- (Open for this session, and the window has room for it: hidden for
	-- room, it stays open, and comes back as the window widens.)
	-- (A plugin's page has a panel of its own too: its plugin's tabs.)
	local shown = ui.computed(function(): boolean
		return props.files:get() and (in_session() or on_plugin() or home()) and fit.current:get().right
	end)
	-- Where the sidebar settles in the window's fit (its width, or none).
	local function sidebar_settled(): number
		local f = fit.current:get()
		return if f.sidebar and not shell_view.collapsed:get() then f.sidebar_limit else 0
	end
	local function tree_part(): number
		return if fit.current:get().files then TREE else 0
	end
	-- The window `v` wide less its gaps round the cards beside a sidebar `sd`
	-- wide (none: the window's edge before the page; one: its gap before
	-- the sidebar's card and the gap between cards after it; then the gap
	-- before the panel's card and the window's edge after it): what the
	-- chat's card and the panel split 50/50. Zeron's columns had no gaps;
	-- left in, they all came off the chat, its card 18 points short of the
	-- panel at "half" each. (The leasts count them already: the chat's 300
	-- and a split's floor are its column's, gaps and all, as Wing set them.)
	local function less_gaps(v: number, sd: number): number
		local before = if sd > 0 then SIDEBAR_GAP + INSET else WINDOW_EDGE
		return v - before - INSET - WINDOW_EDGE
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
	-- Its part beside the tree as it stands. A window resize shares its
	-- change between the chat and this equally, as Zeron's does, so a panel
	-- at its share stays at half of the room (each keeping its least; a
	-- column the fit hides or shows is shared out too). Its width dragged
	-- since its toggle, it holds instead, moving only when it must:
	-- narrowing, the chat gives up its room first, down to its least, and
	-- then this; widening, while the chat is at its least, this takes the
	-- room back first, up to what it would take, then the chat grows.
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
	local alloc = { vw = fit.viewport:peek(), side = sidebar_settled(), tree = tree_part(), pref = right_pref(), floor = fit.page_floor:peek() }
	ui.effect(function()
		local vw, side, tree, pref, floor = fit.viewport:get(), sidebar_settled(), tree_part(), right_pref(), fit.page_floor:get()
		local was = alloc
		alloc = { vw = vw, side = side, tree = tree, pref = pref, floor = floor }
		-- (Hidden (another chat with none, a new chat), its width holds as
		-- it was: what it shows at again is worked out as it opens (`split`).
		-- Followed while hidden, the fit took its tree away there, and the
		-- least below pushed a width chosen narrow up to the least without
		-- a tree, so a chat with its panel open arrived wide, then eased
		-- back to its width.)
		if vw <= 0 or not shown:get() then
			return
		end
		local allowed = fit.panel_room(vw, side, tree, floor)
		local r = right_w:peek()
		-- (Its share of a window `v` wide beside a sidebar `sd` wide.)
		local function share(v: number, sd: number): number
			return fit.content_split(math.max(less_gaps(v, sd) - sd - reserve(), 0), floor)
		end
		-- (The sidebar folded or brought back, the window as it was.)
		local toggled = (side > 0) ~= (was.side > 0) and vw == was.vw and floor == was.floor
		if toggled and floor <= fit.CHAT_PANEL_MIN and not shell_view.expanded:peek() then
			-- The sidebar folded or back: the chat and this split again, as
			-- Zeron's `toggle_sidebar` re-split them (`resplit_panels`), a
			-- width dragged forgotten as there. Below, it eases there.
			if chosen:peek() then
				chosen:set(false)
			end
			local part = share(vw, side)
			alloc.pref = part
			files_w:set(reserve() + part)
			r = math.min(part, allowed)
		elseif pref ~= was.pref then
			-- Split or dragged: what it would take, as far as there's room.
			r = math.min(pref, allowed)
		elseif not chosen:peek() and floor <= fit.CHAT_PANEL_MIN and floor == was.floor and vw ~= was.vw then
			-- At its share: the change shared out, at once (the window's edge is
			-- under the hand). What it would take moves with it, so a sidebar
			-- coming or going gives it back what it gave.
			local by = share(vw, side) - share(was.vw, was.side)
			r = math.min(r + by, allowed)
			local moved_pref = math.max(pref + by, 0)
			if moved_pref ~= pref then
				alloc.pref = moved_pref
				files_w:set(reserve() + moved_pref)
			end
		elseif r > allowed then
			-- No room left for the chat's least: it gives way. (Beside a split, as
			-- below: all the window's narrowing, the panes keeping theirs.)
			r = if floor > fit.CHAT_PANEL_MIN and vw < was.vw
				then math.max(math.min(allowed, r - (was.vw - vw)), fit.RIGHT_PANE_MIN - tree)
				else allowed
		elseif floor > fit.CHAT_PANEL_MIN then
			-- A split's panes keep their size: the panel never grows by
			-- itself, all new room is theirs, and it gives up its own first:
			-- as the window narrows, and as a pane comes beside it.
			local least = fit.RIGHT_PANE_MIN - tree
			if floor > was.floor then
				r = least
			elseif vw < was.vw then
				r = math.max(r - (was.vw - vw), least)
			end
		elseif vw > was.vw and r < pref then
			-- Widening while it gave way: back first, the chat at its least.
			local was_allowed = fit.panel_room(was.vw, was.side, was.tree, was.floor)
			if r >= was_allowed - 0.5 then
				r = math.min(pref, allowed)
			end
		elseif (side < was.side or floor < was.floor) and r < pref then
			-- The sidebar gone that it gave way to, or a pane: back to its own
			-- width, as far as there's room. (Not its tree floating: the file
			-- holds.)
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
	-- Its share, as Zeron's: its tree's allowance and half of what the
	-- sidebar and the tree leave the chat and it. Beside a split, its least:
	-- the room is the panes'. (Dragged wider there, it stays so until it
	-- closes; the width chosen is kept for a chat alone.)
	local function resplit()
		local vw = fit.viewport:peek()
		if vw <= 0 then
			return
		end
		local r = math.max(fit.RIGHT_PANE_MIN - tree_part(), 0)
		local floor = fit.page_floor:peek()
		if floor <= fit.CHAT_PANEL_MIN then
			local side, keep = sidebar_settled(), reserve()
			local part = fit.content_split(math.max(less_gaps(vw, side) - side - keep, 0), floor)
			files_w:set(keep + part)
			r = math.min(part, fit.panel_room(vw, side, tree_part(), floor))
		end
		if r ~= right_w:peek() then
			right_w:set(r)
		end
	end
	-- (Whose panel shows: the session's, a plugin page's; none on a new
	-- chat. Another one is the route moving, not the person opening or
	-- closing this one, and the panel lands at once, as it is there.)
	local function owner(peek: boolean): string
		local c: any = if peek then route.chat:peek() else route.chat:get()
		if c.name == "plugin" then
			return `page:{c.plugin or ""}/{c.page or ""}`
		end
		return if c.name == "session" then `chat:{c.session or ""}` else c.name or ""
	end
	-- It opens at its share, unless a width was chosen since its toggle
	-- (its edge dragged, then another chat's panel shown, or the fit hiding
	-- it for room and bringing it back). Its toggle, opening or closing it,
	-- forgets that width. (The sidebar folding or coming back splits them
	-- again too, above, as Zeron's does: Gaël's call over Wing's earlier
	-- rule, where the chat took all the sidebar gave and the panel held.)
	local split = { on = shown:peek(), open = props.files:peek(), owner = owner(true) }
	ui.effect(function()
		local on = shown:get()
		local open, who = props.files:get(), owner(false)
		local changed = on ~= split.on
		local toggled = open ~= split.open and who == split.owner
		split = { on = on, open = open, owner = who }
		if toggled and chosen:peek() then
			chosen:set(false)
		end
		local vw = fit.viewport:peek()
		if changed and on and not shell_view.expanded:peek() and vw > 0 then
			local side = sidebar_settled()
			if fit.page_floor:peek() > fit.CHAT_PANEL_MIN or not chosen:peek() then
				resplit()
			else
				-- (The width chosen, as far as there's room: not what it had
				-- given way to before it closed.)
				local allowed = fit.panel_room(vw, side, tree_part(), fit.page_floor:peek())
				local r = math.max(math.min(math.max(right_pref(), fit.RIGHT_PANE_MIN - tree_part()), allowed), 0)
				if r ~= right_w:peek() then
					right_w:set(r)
				end
			end
		end
	end)
	-- The sidebar's edge double-clicked (its width put back): the chat and
	-- this split again, the width chosen forgotten, as Zeron's
	-- `reset_panel_widths` (a seam's reset restores the whole layout).
	local resets = shell_view.seam_reset:peek()
	ui.effect(function()
		local n = shell_view.seam_reset:get()
		if n == resets then
			return
		end
		resets = n
		if not shown:peek() or shell_view.expanded:peek() then
			return
		end
		if chosen:peek() then
			chosen:set(false)
		end
		resplit()
	end)
	-- (Before what follows, so a panel arriving with the route lands where this puts it.)
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
			-- (The row's room inside its padding, the window round the cards:
			-- the gap before the page's card and the window's edge at its
			-- right, where the panel's card ends unexpanded too. The page's
			-- card gone, there's no gap before the panel's.)
			local before = if fit.current:get().sidebar then INSET else WINDOW_EDGE
			local room: number = (row_ref.width or 0) - before - WINDOW_EDGE
			-- (Settings' list stands where the sidebar does, in this row: as beside the
			-- sidebar, the panel has what's past it.)
			if away() and not shell_view.collapsed:get() then
				local sidebar: any = require("@app/screens/shell/sidebar")
				room -= math.clamp(sidebar.width:get(), fit.SIDEBAR_MIN, fit.SIDEBAR_MAX) + 6
			end
			if room > 0 then
				return room
			end
		end
		return if shown:get() then rest_w:get() else 0
	end
	local open_w = ui.state("shell_files_shown_width", target())
	-- Where its width is going (`base` below, told), and whether a step that
	-- opens, closes, expands or collapses it is under way: then what's in it
	-- is laid out at that from the step's first frame, so nothing in it
	-- squeezes as its edge moves. (A column the fit takes away or gives as
	-- the window is resized, it follows the edge: that's the window's edge
	-- under the hand.) (The build's own, as the animation is.)
	local goal = ui.use_state(open_w:peek())
	local holding = ui.use_state(false)
	-- (The width its content showed at as it began to close, and its
	-- content's width as each frame laid it out: `inner_w`'s.)
	local left_at: number? = nil
	local showing_w = 0
	-- What's in it (its tabs and what they show) and the page's own content
	-- (held under the panel expanded over it: `page_hold`), for their drift
	-- and fade as they come and go (ds/motion `content`).
	local content_el: any = nil
	local page_el: any = nil
	-- The page's content width while the panel expands over it or gives it
	-- back: it keeps the width it had (expanding) or takes the one it's
	-- going to (coming back), clipped by its edge, never squeezed to nothing
	-- or growing from it. nil: the page's own.
	local page_hold = ui.use_state(nil :: number?)
	-- (Its width as the panel expanded, with the window's and the panel's.)
	local held_from: { w: number, vw: number, panel: number }? = nil
	-- Its width is where it's going (`base`, which a window resize moves at
	-- once) and, after a step (opened, closed, expanded, a column going or
	-- coming beside it, a neighbour's share), what's left of that step
	-- (`off`), easing to nothing on its own clock (Zeron's column motion,
	-- ds/motion). So a step glides whatever the window does meanwhile, and
	-- is over in its time; nothing lags the window's edge.
	local base = open_w:peek()
	local off = ui.use_state(0)
	local easing: any = nil
	local steps = 0
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
			side = if f.sidebar and not shell_view.collapsed:peek() then f.sidebar_limit else 0,
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
			side = if f.sidebar and not shell_view.collapsed:get() then f.sidebar_limit else 0,
			files = f.files,
			owner = owner(false),
		}
		local to = target()
		local opened = now.shown ~= seen.shown or now.expanded ~= seen.expanded
		local flipped = opened or now.sidebar ~= seen.sidebar or now.files ~= seen.files
		local resized = now.vw ~= seen.vw
		local moved = now.owner ~= seen.owner
		local was = seen
		seen = now
		-- What comes or goes with it stands still under its edge and only
		-- fades a little (ds/motion `content`): its content as it opens or
		-- closes, the page's as the panel expands over it. The route moving
		-- shows each as it is there, at once.
		if now.shown ~= was.shown and content_el then
			anime.animate(content_el, motion.content(now.shown, moved))
		end
		if now.expanded ~= was.expanded then
			if page_el then
				-- (Given back from rest, where it stood hidden under the panel, it
				-- comes from the floor, not from nothing.)
				anime.animate(page_el, motion.content(not now.expanded, moved, not now.expanded and easing == nil))
			end
			-- (Measured before this step changes anything: the page's room now,
			-- and the row it shares with the panel.)
			local area = splits.area().w
			if moved and not now.expanded then
				page_hold:set(nil)
			elseif now.expanded then
				page_hold:set(area)
				held_from = { w = area, vw = now.vw, panel = base + off:peek() }
			elseif held_from and held_from.vw == now.vw and held_from.panel == to then
				-- (Back to where it was: the width it had.)
				page_hold:set(held_from.w)
			else
				-- (The row less where the panel goes and the gap before it; none
				-- while it stood expanded.)
				local whole = area + base + off:peek()
				page_hold:set(math.max(whole - to - (if to > 0 then INSET else 0), 0))
			end
		end
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
			holding:set(false)
			off:set(0)
			base = to
		elseif resizing then
			-- Its edge dragged: under the pointer, no step left.
			if easing then
				easing:cancel()
				easing = nil
			end
			holding:set(false)
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
			-- (Opening or closing, what's in it holds still under its edge. Between
			-- two open widths (expanding, collapsing), it follows its edge, as
			-- Zeron's `panel_content_tween` eases it: laid out at the far width
			-- from the first frame, a home centred in it jumped half the way at
			-- once, and collapsing left the card empty beside it.)
			if now.shown ~= was.shown then
				-- (Closing, at the width it shows now: expanded, the wide one.)
				left_at = if now.shown then nil else showing_w
				holding:set(true)
			elseif opened then
				holding:set(false)
			end
			off:set(showing - to)
			-- (Split again as the sidebar folds or comes back: on the sidebar's
			-- own clock, its curve and its time for its column's travel, so the
			-- chat between them only ever widens or narrows, as in Zeron, where
			-- both rode one tween. On the panel's quicker curve, the chat
			-- narrowed a little first as the sidebar folded, then widened.)
			local params = if now.sidebar ~= was.sidebar and not opened
				then motion.column(math.max(now.side, was.side) + SIDEBAR_GAP, 0)
				else motion.panel(0)
			-- (By number: a callback holding its own clip would keep both for good.)
			steps += 1
			local this = steps
			params.onComplete = function()
				if steps == this then
					easing = nil
					stepping:set(false)
					holding:set(false)
					-- (The page under the panel expanded: hidden, not left at the
					-- fade's floor as a sliver of its edge.)
					if page_el and shown:peek() and shell_view.expanded:peek() then
						anime.animate(page_el, { opacity = 0, duration = 0 } :: any)
					end
					-- (The page given back: its own width again, the one it was
					-- held at.)
					if not (shown:peek() and shell_view.expanded:peek()) and page_hold:peek() ~= nil then
						page_hold:set(nil)
					end
				end
			end
			easing = anime.animate(off, params)
		else
			-- The window resized (or the room it fills while expanded moved):
			-- where it goes moves at once; a step under way goes on.
			base = to
		end
		if not easing and not now.expanded and page_hold:peek() ~= nil then
			page_hold:set(nil)
		end
		if goal:peek() ~= base then
			goal:set(base)
		end
		if stepping:peek() ~= (easing ~= nil) then
			stepping:set(easing ~= nil)
		end
		open_w:set(base + off:peek() + edge_drag.nudge:peek())
	end)
	-- (Following the step as it eases, and its edge's bounce.)
	ui.effect(function()
		local o = off:get()
		local w = base + o + edge_drag.nudge:get()
		if open_w:peek() ~= w then
			open_w:set(w)
		end
	end)
	-- The panel's content width: opening, where its width is going, from
	-- the step's first frame, its edge sweeping over it as it gets there;
	-- closing and shut, the width it rested at. Otherwise its own: under the
	-- hand as it's dragged or the window is resized, following its edge as
	-- it expands or collapses.
	local function inner_w(): number
		local w: number
		if not holding:get() and shown:get() then
			w = open_w:get()
		else
			local g = goal:get()
			w = if g > 0 then g else left_at or rest_w:get()
		end
		showing_w = w
		return w
	end
	-- The widest it may be asked to be: what leaves the page what it wants.
	local function most_w(): number
		return math.max(FILES_MIN, fit.viewport:peek() - sidebar_settled() - fit.page_floor:peek())
	end
	-- Its width asked for (its edge dragged, or keyed): never past what
	-- leaves the page what it wants; kept as the width it takes with its tree
	-- docked.
	local function set_width(want: number)
		local tree = if fit.current:peek().files then TREE else 0
		local w = math.clamp(want, FILES_MIN, most_w())
		files_w:set(reserve() + w - tree)
		if not chosen:peek() then
			chosen:set(true)
		end
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
		shrink = 0,
		h_full = true,
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
		opacity = function(): number
			return if panel_seen:get() then 1 else 0
		end,
		-- (From the window's top: the panel's own band is the title strip
		-- over it, its tabs in it, a window-drag region of its own, room
		-- left at its right for the title bar's Panel toggle.)
		-- (Its card is its box, as the sidebar's is; what's in it stands at
		-- its right, the side it's attached to, at `inner_w`: the edge
		-- sweeping in from the left reveals it and covers it again. Shut, and
		-- its width run out, out of layout until it opens: kept as it was,
		-- not laid out at its width in a box of none.)
		ui.column {
			w_full = true,
			h_full = true,
			hidden = function(): boolean
				return not shown:get() and not panel_seen:get()
			end,
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
						no_project = true,
						content = {
							id = "shell-files-content",
							w = inner_w,
							-- (At the card's right, out of its flow: laid out at its
							-- width while the card's edge sweeps over it, a clip
							-- meant, not content overflowing the card.)
							absolute = true,
							right = 0,
							top = 0,
							bottom = 0,
							on_mount = function(el: any)
								content_el = el
								anime.animate(el, motion.content(shown:peek(), true))
							end,
						},
					}
				end)
				else ui.column {
					w = inner_w,
					self = "end",
					h_full = true,
					boundary.screen(boundary.named("shell.boundary.screen.files"), "@app/screens/files", function(files): Node
						return files.Pane { project = route.project, session = route.session }
					end),
				},
		},
	}
	panel_open = function(): boolean
		return shown:get()
	end
	panel_beside = function(): number
		return open_w:get() + gap_before:get()
	end
	-- (No hairline down its left; its card's edge is that.)
	-- Its edge, dragged: wider or narrower (the page keeps its least),
	-- bouncing past either; double-clicked, back to its share, easing there,
	-- the width chosen forgotten. The gap before its card, taking the pointer
	-- `seam.REACH` each side of the gap's middle: the gap is where a hand goes
	-- for it, and the card clips what's its own, so this stands beside it.
	-- (Pressed itself, so it keeps the pointer as it's dragged. Not while
	-- expanded: it's as wide as it goes. While the panel moves a press there
	-- does nothing, as Zeron's handle; the pointer stays its own all along,
	-- so a hand resting on it finds it lit once the panel lands.)
	local panel_edge = ui.div {
			absolute = true,
			right = function(): number
				return ui.raw(WINDOW_EDGE + open_w:get() + gap_before:get() / 2 - seam.REACH, "the seam's reach past the gap's middle")
			end,
			top = ui.raw(TOP_BAR, "as the panel's top"),
			bottom = ui.raw(WINDOW_EDGE, "the window's edge"),
			w = function(): number
				return if expanded() then 0 else ui.raw(2 * seam.REACH, "the seam's reach either side")
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
				if stepping:peek() then
					return
				end
				if e.clicks == 2 then
					if chosen:peek() then
						chosen:set(false)
					end
					resplit()
					return
				end
				-- (From where it stands.)
				from_x, from_w, resizing = e.x, open_w:peek(), true
				drag_held:set({ w = inner_w() })
			end,
			on_pointer_move = function(e)
				if resizing then
					local want = from_w - (e.x - from_x)
					edge_drag.at(want, FILES_MIN, most_w())
					set_width(want)
				end
			end,
			on_release = function()
				resizing = false
				drag_held:set(nil)
				edge_drag.release()
			end,
			-- What lights: the gap, as between two panes.
			ui.div {
				absolute = true,
				left = function(): number
					return ui.raw(seam.REACH - gap_before:get() / 2, "the gap, in the seam's reach")
				end,
				top = 0,
				bottom = 0,
				w = gap_before,
				rounded = "$radius.full",
				pointer_events = "none",
				bg = "$border.strong",
				opacity = seam.lit(function(): boolean
					return edge_over:get() or drag_held:get() ~= nil
				end, edge_drag),
			},
	}

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
		-- (From where it starts over a composer at rest: a taller one takes from it,
		-- rather than pushing it up the pane into the middle.)
		bottom = { left = BAND, right = BAND, top = "75%", mt = ui.raw(-(Dock.GAP + Dock.REST), "over a composer at rest"), min_h = LEAST },
		left = { left = 0, top = 0, w = BAND, min_w = LEAST },
		right = { right = 0, top = 0, w = BAND, min_w = LEAST },
	}
	-- The room a pane's composer takes at its foot: the window's own, measured, over
	-- the focused one; a pane's own over another.
	local function zone(id: string, side: string): Node
		local props: { [any]: any } = table.clone(ZONES[side])
		props.absolute = true
		if side ~= "top" then
			-- (A pane's own, as its lane measures it: a draft with a chip stands taller.)
			local own = LaneTab.dock_height(`split-{id}`)
			props.bottom = function(): number
				local h = if splits.focused:get() == id then chat_shared.dock_h:get() else own:get()
				return ui.raw(Dock.GAP + h, "the pane's composer, which attaches")
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
				-- (A chat from the sidebar is focused as it lands; a pane moved
				-- already is, by the press on its grip.)
				splits.drop(d, id, side, payload.split_pane == nil)
			end
		end
		return ui.div(props)
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
	-- lane of its own, dimmed, a click on it focusing it. Its grip moves it; ⌘W
	-- takes the focused one out of the split.
	-- (None while expanded or while a chat is carried. Not a `ui.show` round
	-- them: its box would lie over the page and take the pointer from it.)
	local divider_ids = ui.computed(function(): { string }
		local out: { string } = {}
		if expanded() or splits.dragging:get() then
			return out
		end
		for _, d in splits.dividers:get() do
			table.insert(out, d.id)
		end
		return out
	end)
	local function pane_item(id: string): Node
		local lane_slot = `split-{id}`
		local function focused(): boolean
			return splits.focused:get() == id
		end
		local hovered = ui.use_state(false)
		local near = ui.use_state(false)
		local card_ref = ui.ref()
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
			-- (Opening, at the size it's opening to: its pane's edge reveals it.)
			min_w = function(): number
				local size = splits.opening_size(id)
				return if size then ui.raw(size.w, "the size it's opening to") else 0
			end,
			min_h = function(): number
				local size = splits.opening_size(id)
				return if size then ui.raw(size.h, "the size it's opening to") else 0
			end,
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
			ui.column((function()
				local lane: { [any]: any } = {
					id = `split-lane-{id}`,
					absolute = true,
					left = 0,
					right = 0,
					top = 0,
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
					pane_grip(function(): string?
						return id
					end, function(): boolean
						return near:get()
					end, card_ref),
				}
				-- (Made as it's added to the split: it stands where it lands as
				-- the gap it opens out of reveals it, only fading in a little.)
				lane.on_mount = function(el: any)
					if splits.just_added(id) then
						anime.animate(el, motion.content(true, false, true))
					end
				end
				tracks_near(lane, near)
				return lane
			end)()),
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
		local item: { [any]: any } = {
			-- (`#split-pane-<session>`, `.split-pane`: a chat's pane, the
			-- session under it.)
			id = `split-pane-{id}`,
			class = "split-pane",
			provide = { session = id },
			pointer_events = "none",
			-- (Opening, its edge clips its card, in the card's corners.)
			rounded = "$zeron.inset.radius",
			overflow = function(): string
				return if splits.opening_size(id) then "hidden" else "visible"
			end,
			ui.show(grouped, card),
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
	-- A divider between two panes: dragging it moves their split, at once,
	-- bouncing at either pane's least; double-clicked, its panes take equal
	-- shares again, easing there. It takes the pointer `seam.REACH` each side
	-- of the gap's middle, its gap lit under it.
	local PANE_LEAST_WIDE, PANE_LEAST_TALL = 288, 160
	local DIVIDER_REACH = seam.REACH - splits.GAP / 2
	local function divider(id: string): Node
		local function find(list: { any }): any
			for _, d in list do
				if d.id == id then
					return d
				end
			end
			return nil
		end
		local function now(): any
			return find(splits.dividers:get()) or { left = splits.WHOLE.left, right = splits.WHOLE.right, top = splits.WHOLE.top, bottom = splits.WHOLE.bottom }
		end
		local first = find(splits.dividers:peek())
		local row = first == nil or first.axis == "row"
		local lit = ui.use_state(false)
		local held = ui.use_state(false)
		local drag = seam.drag()
		-- The press, and the last move (`moved`, `least`): what its bounce
		-- goes past, after the drag too.
		local start: any = nil
		local last: any = nil
		local function apply(s: any)
			splits.resize(s.path, s.part, s.shares, s.moved, s.least, drag.nudge:peek() / s.length)
		end
		local was_nudge = 0
		ui.effect(function()
			local n = drag.nudge:get()
			if n == 0 and was_nudge == 0 then
				return
			end
			was_nudge = n
			if last then
				apply(last)
			end
		end)
		local item: { [any]: any } = {
			id = `split-divider-{id}`,
			pointer_events = "auto",
			cursor = if row then "col_resize" else "row_resize",
			on_hover = function(inside: boolean)
				lit:set(inside)
			end,
			on_press = function(e: any)
				local d = find(splits.dividers:peek())
				if not d then
					return
				end
				last = nil
				if e.clicks == 2 then
					splits.even(d.path)
					return
				end
				local area = splits.area()
				local span = if row then area.w else area.h
				start = { at = if row then e.x else e.y, shares = splits.shares_at(d.path), path = d.path, part = d.at, length = d.length.p * span + d.length.q }
				held:set(true)
			end,
			on_pointer_move = function(e: any)
				local s = start
				if s and s.length > 0 then
					local moved = ((if row then e.x else e.y) - s.at) / s.length
					local least = math.min(0.45, (if row then PANE_LEAST_WIDE else PANE_LEAST_TALL) / s.length)
					local lo, hi = splits.bounds(s.shares, s.part, least)
					drag.at((s.shares[s.part] + moved) * s.length, lo * s.length, hi * s.length)
					last = { path = s.path, part = s.part, shares = s.shares, moved = moved, least = least, length = s.length }
					apply(last)
				end
			end,
			on_release = function()
				start = nil
				held:set(false)
				drag.release()
			end,
			-- What lights: the gap itself.
			ui.div {
				absolute = true,
				left = if row then DIVIDER_REACH else 0,
				right = if row then DIVIDER_REACH else 0,
				top = if row then 0 else DIVIDER_REACH,
				bottom = if row then 0 else DIVIDER_REACH,
				rounded = "$radius.full",
				pointer_events = "none",
				bg = "$border.strong",
				opacity = seam.lit(function(): boolean
					return lit:get() or held:get()
				end, drag),
			},
		}
		-- Placed at its gap's insets, reaching a little past it each side to take the
		-- pointer.
		local WHY = "a divider's place: a share of the page, plus points"
		local along = if row then DIVIDER_REACH else 0
		local across = if row then 0 else DIVIDER_REACH
		item.absolute = true
		item.snap = true
		item.left = function(): string
			return `{now().left.p * 100}%`
		end
		item.ml = function(): number
			return ui.raw(now().left.q - along, WHY)
		end
		item.right = function(): string
			return `{(1 - now().right.p) * 100}%`
		end
		item.mr = function(): number
			return ui.raw(-now().right.q - along, WHY)
		end
		item.top = function(): string
			return `{now().top.p * 100}%`
		end
		item.mt = function(): number
			return ui.raw(now().top.q - across, WHY)
		end
		item.bottom = function(): string
			return `{(1 - now().bottom.p) * 100}%`
		end
		item.mb = function(): number
			return ui.raw(-now().bottom.q - across, WHY)
		end
		return ui.div(item)
	end
	-- (Each pane's landing, only while a chat is carried. Not a `ui.show` round them:
	-- its box would lie over the page and keep a drop from the chat under it.)
	local landing_ids = ui.computed(function(): { string }
		return if splits.dragging:get() and not expanded() then members:get() else {}
	end)
	-- The page's room: the page's card at the focused chat's place, the
	-- split's panes under it.
	local split_area = ui.row {
		grow = 1,
		min_w = 0,
		h_full = true,
		relative = true,
		hidden = function(): boolean
			return away()
		end,
		on_mount = function(el: any)
			splits.set_area(el)
		end,
		-- (In a split, the frame of the window's composer alone, over the
		-- panes: the pointer goes through to them, but for the composer.)
		pointer_events = function(): any
			return if grouped() then "none" else "auto"
		end,
		-- (While the panel expands over the page or gives it back, what's in
		-- it is held at a width (`page_hold`) and this clips it, in the cards'
		-- corners: its edge reads as the page's card's own.)
		overflow = function(): string
			return if page_hold:get() then "hidden" else "visible"
		end,
		rounded = "$zeron.inset.radius",
		ui.div {
			id = "shell-page-room",
			absolute = true,
			left = 0,
			top = 0,
			bottom = 0,
			w = function(): any
				local held = page_hold:get()
				return if held then ui.raw(held, "the page's width held as the panel expands over it") else "100%"
			end,
			pointer_events = function(): any
				return if grouped() then "none" else "auto"
			end,
			-- (Its fade as the panel expands over it: ds/motion `content`.
			-- Gone, it takes no clicks.)
			on_mount = function(el: any)
				page_el = el
				anime.animate(el, { opacity = if expanded() then 0 else 1, duration = 0 } :: any)
			end,
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
			-- (The page's card, placed by its box in a split: there the frame of
			-- the window's composer alone, over the panes.)
			ui.column(pane),
			ui.div {
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				pointer_events = "none",
				ui.each(divider_ids, divider, {
					key = function(id: string): string
						return id
					end,
					w_full = true,
					h_full = true,
					relative = true,
				}),
			},
			ui.div {
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				pointer_events = "none",
				ui.each(landing_ids, landing, {
					key = function(id: string): string
						return id
					end,
					w_full = true,
					h_full = true,
					relative = true,
				}),
			},
		},
	}

	-- Two cards, the page and its panel, set in from the window's edges and
	-- the sidebar by the gap, the title bar's line inside them. The window
	-- round them, along the title bar's strip (over them, beside them and
	-- between them), moves the window as the bar does: a strip under the
	-- cards, which let the pointer through but for what takes it.
	return ui.row {
		ref = row_ref,
		grow = 1,
		min_w = 0,
		h_full = true,
		relative = true,
		pt = ui.raw(TOP_BAR, "room for the top bar"),
		-- (The window's edges a little more than the gap between cards: the
		-- same to the eye.)
		pb = ui.raw(WINDOW_EDGE, "the window's edge"),
		-- (The gap between cards beside the sidebar's, the window's edge
		-- without it: the one going to the other only over the sidebar's
		-- last points, so the gap beside its edge holds as it folds.)
		pl = function(): number
			-- (Its column, the window's gap before its card included.)
			local shown = math.min(shell_view.sidebar_shown:get(), WINDOW_EDGE)
			return ui.raw(WINDOW_EDGE - (WINDOW_EDGE - INSET) * shown / WINDOW_EDGE, "the window's edge, or the gap beside the sidebar")
		end,
		pr = ui.raw(WINDOW_EDGE, "the window's edge"),
		pointer_events = "none",
		ui.div { id = "shell-drag", absolute = true, left = 0, right = 0, top = 0, h = TITLE_H, window_drag = true },
		split_area,
		-- (Settings, in the page's place.)
		if props.settings
			then ui.column {
				grow = 1,
				min_w = 0,
				h_full = true,
				overflow = "hidden",
				-- (This row lets the pointer through but for what takes it.)
				pointer_events = "auto",
				hidden = function(): boolean
					return not away()
				end,
				props.settings(),
			}
			else nil,
		beside,
		panel_edge,
		-- The sessions open, along the whole top bar: from just past the sidebar's
		-- "…" (with it shut, past Back and Forward) to the panel toggle.
		session_tabs.Strip {
			window_x = function(): number
				-- (The row ends at the window's right: where it starts is what its
				-- width leaves, followed as the sidebar moves.)
				return props.width() - (row_ref.width or props.width())
			end,
			start = function(): number
				local past = ds.TitleBar.last_control_end() + 8
				if sidebar_settled() <= 0 then
					return past
				end
				local button = (ui.theme.get("zeron.icon_button.md") :: number?) or 24
				return past + button + 8
			end,
			-- (Clear of the panel toggle, and of Expand while the panel's open.)
			right = function(): number
				local toggle = 34 + 8 - 10 + ds.TitleBar.actions_right()
				local right = if shown:get() then toggle + EXPAND + 8 else toggle
				-- (And of what there is to update.)
				local button = updates.width()
				return if button > 0 then right + button + 8 else right
			end,
		},
		-- What there is to update, beside the panel toggle (and Expand, with the
		-- panel open): nothing with nothing to say.
		ui.show(updates.shown, ui.div {
			absolute = true,
			right = function(): number
				local toggle = 24 + 8 + ds.TitleBar.actions_right()
				return ui.raw(if shown:get() then toggle + EXPAND + 8 else toggle, "off the panel toggle")
			end,
			top = function(): number
				return ds.TitleBar.middle() - 14
			end,
			pointer_events = "auto",
			updates.Button {},
		}),
		-- Expand: the panel over the session's chat, and back, on the top bar beside
		-- the panel toggle, as far off it as Back is off the sidebar's toggle.
		ui.show(function(): boolean
			return shown:get()
		end, ui.div {
			absolute = true,
			right = function(): number
				return ui.raw(24 + 8 + ds.TitleBar.actions_right(), "off the panel toggle")
			end,
			top = function(): number
				return ds.TitleBar.middle() - EXPAND / 2
			end,
			pointer_events = "auto",
			-- (Its name, glyph and tooltip swap as it flips: two buttons.)
			ui.presence(function(): string
				return if expanded() then "collapse" else "expand"
			end, function(which: string): Node
				return ds.IconButton {
					id = "panel-expand",
					label = if which == "collapse" then ui.t("panel.strip.collapse") else ui.t("panel.strip.expand"),
					tooltip_side = "bottom",
					w = EXPAND,
					h = EXPAND,
					-- (One of the title bar's: its glyph held under the pointer.)
					tone = "bar",
					on_click = function()
						shell_view.toggle_expanded()
					end,
					ds.Icon { name = if which == "collapse" then "collapse-arrows" else "expand-arrows", size = 14, glyph_stroke = true },
				}
			end),
		}),
	}
end
