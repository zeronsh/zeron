-- A project's files and the one they show, as one pane: Zeron's files
-- panel beside its editor (crates/ui/src/shell/files_panel.rs), here one
-- surface. The tree at the left, the file beside it, and one band across
-- the top of both: the tree's filter and tools in its first part, the
-- file's name and actions in the rest, one hairline under the whole of it,
-- the tree's divider starting under that; so the pane reads as one thing,
-- not two glued together.
--
-- The tree hides and shows: a toggle at the band's start (the sidebar's
-- glyph, its panel narrowing as the tree goes), ⇧⌘E while the pane has
-- focus (an app binds it everywhere with `ui.command` and `commands.toggle`),
-- or `shown` set from outside. The toggle never moves: it stands at the
-- pane's edge, over the tree's head while the tree shows and over the
-- file's while it doesn't, and the file's name slides to meet it. Hiding,
-- the tree stands where it is and the file's edge sweeps over it, taking
-- the room, its ink fading a little as it goes (ds/motion `content`, as
-- the sidebar folds); showing is the same back. `shown` is the caller's,
-- so it can be remembered. With `column` (the tree's room its own, outside
-- the file's: the app's panel grows by it), it comes at once and goes once
-- the panel's edge, sweeping over it instead, has covered it.
--
-- Its edge drags it wider or narrower (`width`, `least` to `most`),
-- bouncing at either; double-clicked, back to `default` (ds/seam).
--
-- With no file open and too little room for one beside it, the tree takes the
-- whole pane, until a file is chosen.
--
-- `FilesPane { shown = state, tree = { FileTree's props }, viewer = { FileViewer's props }, head = "line" }`.

local FileTree = require("@self/file_tree")
local FileViewer = require("@self/file_viewer")
local SidebarIcon = require("@self/sidebar_icon")
local IconButton = require("@self/icon_button").IconButton
local motion = require("@self/motion")
local seam = require("@self/seam")
local locale = require("@self/locale")

export type Props = {
	-- Whether the tree shows beside the file (the caller's, remembered).
	shown: State<boolean>,
	tree: FileTree.Props,
	viewer: FileViewer.Props,
	-- The file's header shape (ds/file_viewer): one line by default.
	head: FileViewer.Head?,
	-- The tree's width beside the file (the theme's): a state, its edge drags
	-- it between `least` and `most` and a double click puts back `default`;
	-- `dragging` is told while it's dragged.
	width: (number | State<number>)?,
	least: number?,
	most: number?,
	default: number?,
	dragging: State<boolean>?,
	-- The tree's room is a column of its own (the app's panel grows by it):
	-- it comes at once and goes once the panel's edge has covered it (the
	-- panel's step, ds/motion `PANEL_MS`), that edge sweeping over it.
	column: (() -> boolean?)?,
	-- The app's panel stepping to a new width: the tree, its column gone,
	-- stays docked until that step lands (covered by then).
	moving: (() -> boolean)?,
	-- While its column comes or goes, the app's panel's step: its width
	-- from, to, and now. The tree's room follows it, so the file takes or
	-- gives the room a frame at a time and the tree is wholly uncovered (or
	-- covered) as the step lands, standing where it is all along.
	-- `held`: what's in the panel laid out at its far width meanwhile, its
	-- edge sweeping over it; else following that edge (a live resize).
	step: (() -> { from: number, to: number, coming: boolean, held: boolean, now: number }?)?,
	-- The pane's last width, if the caller keeps it: a pane made again starts
	-- exactly as it was.
	room: State<number>?,
	-- Filled with `toggle`, for a command bound outside.
	commands: { [string]: () -> () }?,
	[any]: any,
}

local OWN = { shown = true, tree = true, viewer = true, head = true, width = true, least = true, most = true, default = true, dragging = true, column = true, moving = true, step = true, commands = true, room = true, children = true }

-- How the tree moves: the sidebar's slide, and its ink a beat quicker.
local SLIDE = { duration = 260, ease = "out(3)" }
local INK = { duration = 160 }
-- The keys that hide and show it.
local KEYS = "cmd+shift+e"
-- The least a file's side is worth having, with no file to show yet.
local FILE_LEAST = 240

local function num(token: string, fallback: number): number
	return (ui.theme.get(token) :: number?) or fallback
end

local M = {}
M.keys = KEYS

local made = ui.component(function(props: Props): Node
	local shown = props.shown
	local theme_w = num("zeron.files.tree", 288)
	local width_state: State<number>? = if props.width ~= nil and type(props.width) ~= "number" then props.width :: any else nil
	-- Its edge's drag (below): a bounce past either end, added to its width.
	local edge_drag = if width_state then seam.drag() else nil
	-- The tree's width now (a state's, followed).
	local function tree_w(): number
		if width_state then
			local nudge = if edge_drag then edge_drag.nudge:get() else 0
			return math.clamp(width_state:get(), props.least or 0, props.most or math.huge) + nudge
		end
		return (props.width :: number?) or theme_w
	end
	-- (nil: nobody lays a column out for it, it stands inside the pane.)
	local function column_now(): boolean?
		return if props.column then props.column() else nil
	end
	local function column(): boolean
		return column_now() == true
	end
	-- Whether it stands beside the file: shown, and, given a column, while
	-- the app has room for that column (short of it, it goes, as Zeron's
	-- Files did; it's back with the room).
	local function in_place(): boolean
		return shown:get() and column_now() ~= false
	end
	local head_h = num(if props.head == "stack" then "zeron.files.stack" else "zeron.files.head", 40)
	local square = num("zeron.icon_button.lg", 28)
	local pad = num("space.2", 8)
	-- Room the heads keep at their start: the inset, the toggle, a step.
	local lead_w = pad + square + pad

	-- How wide the pane is. (Kept by the caller, a pane made again knows it on its
	-- first frame, so nothing opens, closes or fades that was at rest.)
	local room = props.room or ui.use_state(0)
	local settled = false
	local function out(): boolean
		return shown:get()
	end
	local parked = ui.use_state(false)
	-- No file open, and no room for one beside the tree: the tree has it all.
	local doc = props.viewer.doc :: any
	local function whole(): boolean
		local open = if type(doc) == "function" then doc() else doc:get()
		local w = room:get()
		return shown:get() and open == nil and w > 0 and w < tree_w() + FILE_LEAST
	end
	-- The tree's room beside the file, eased as it comes and goes by the tree's own
	-- slide.
	local function dock_target(): number
		if whole() then
			return room:get()
		end
		if not (in_place() or parked:get()) then
			return 0
		end
		-- (Its column coming or going: from what the file gives up as the
		-- panel widens to all of it, or back.)
		local s = props.step and props.step()
		if s and math.abs(s.from - s.to) > 0.5 then
			local p = math.clamp((s.now - s.from) / (s.to - s.from), 0, 1)
			local gap = if s.held then math.abs(s.to - s.from) else 0
			local a, b = if s.coming then gap else tree_w(), if s.coming then tree_w() else gap
			return math.clamp(a + (b - a) * p, 0, tree_w())
		end
		return tree_w()
	end
	local dock_w = ui.use_state(dock_target())
	-- (Its column gone (hidden, or no room left for it): it stays docked, as
	-- the panel's edge covers it, for that edge's step; then it goes,
	-- covered.)
	local column_shown = column() and in_place()
	local unparking = false
	local saw_moving = false
	ui.effect(function()
		local on = column() and in_place()
		if on then
			column_shown = true
			if parked:peek() then
				parked:set(false)
			end
		elseif column_shown then
			column_shown = false
			-- (Nothing steps with motion reduced: it goes at once.)
			if not motion.reduced_now() then
				saw_moving = false
				parked:set(true)
				-- (Should no step come, it goes all the same.)
				ui.after(2 * motion.PANEL_MS, function()
					if parked:peek() and not saw_moving then
						unparking = true
						parked:set(false)
					end
				end)
			end
		end
	end)
	-- (Parked: it goes as the step that covers it lands.)
	ui.effect(function()
		local m = props.moving ~= nil and props.moving()
		if not parked:get() then
			return
		end
		if m then
			saw_moving = true
		elseif saw_moving then
			saw_moving = false
			unparking = true
			parked:set(false)
		end
	end)
	local dock_easing: any = nil
	local was_whole = whole()
	ui.effect(function()
		local to = dock_target()
		local now_whole = whole()
		-- (All of it already, the pane resizing: with the pane, not after it.)
		local follows = now_whole and was_whole
		was_whole = now_whole
		if dock_easing then
			dock_easing:cancel()
			dock_easing = nil
		end
		-- (A column of its own: at once, the panel's edge sweeping. Its edge
		-- dragged: under the hand.)
		local at_once = unparking
		unparking = false
		local stepping = props.step ~= nil and props.step() ~= nil
		if not settled or follows or at_once or stepping or column() or parked:get() or (props.dragging ~= nil and props.dragging:get()) then
			dock_w:set(to)
		else
			dock_easing = anime.animate(dock_w, motion.params({ value = to, duration = SLIDE.duration, ease = SLIDE.ease }))
		end
	end)
	-- Its ink (and the toggle's glyph): on while it is out; the ink eased
	-- once the pane has been measured, set before.
	local lit = ui.use_state(out())
	local ink = ui.use_state(if out() then 1 else 0)
	local fading: any = nil
	ui.effect(function()
		local on = out()
		lit:set(on)
		if fading then
			fading:cancel()
			fading = nil
		end
		-- (Only a little: the edge covering it does the rest.)
		local to = if on then 1 else motion.FADE_FLOOR
		if settled then
			fading = anime.animate(ink, motion.params({ value = to, duration = motion.FADE_MS, ease = motion.EASE }))
		else
			ink:set(if on then 1 else 0)
		end
	end)
	local function toggle()
		shown:set(not shown:peek())
	end
	if props.commands then
		props.commands.toggle = toggle
	end

	-- The tree's width: its own, or all it's been given (it never slides narrower,
	-- only away).
	local function tree_now(): number
		return math.max(tree_w(), dock_w:get())
	end
	-- The tree's props, beside the file.
	local function tree_props(): FileTree.Props
		local tp: FileTree.Props = table.clone(props.tree)
		tp.head = head_h
		tp.lead = lead_w
		-- The toggle's room, in the tree's own points as it slides (its
		-- right edge less where the tree's start is): the head is cut
		-- there, so it never shows under the toggle. At rest that's left
		-- of the head's field (`lead`), so nothing changes there.
		tp.head_cut = function(): number
			return pad + square
		end
		tp.w = tree_now :: any
		tp.h_full = true
		return tp
	end
	-- Beside the file: at its full width, slid under the pane's edge as far
	-- as it is in.
	local function beside(): Node
		return ui.column {
			id = "files-tree-beside",
			absolute = true,
			left = 0,
			top = 0,
			bottom = 0,
			w = tree_now,
			-- (Its ink goes as the user hides it.)
			opacity = function(): number
				return ink:get()
			end,
			-- (Gone, its ink is out and nothing in it can be touched.)
			pointer_events = function(): PointerEvents
				return if dock_w:get() > 0.5 then "auto" else "none"
			end,
			FileTree(tree_props()),
			-- Its edge, from under the band (the band's hairline runs across
			-- both heads).
			-- (None with no file's side to part it from.)
			ui.div {
				absolute = true,
				right = 0,
				top = head_h,
				bottom = 0,
				w = 1,
				bg = "$border.subtle",
				opacity = function(): number
					return if dock_w:get() >= room:get() - 0.5 then 0 else 1
				end,
			},
		}
	end
	-- (Standing at the pane's left, its room clipping it as the file's edge
	-- sweeps over it.)
	local tree = ui.div {
		absolute = true,
		left = 0,
		top = 0,
		bottom = 0,
		w = dock_w,
		overflow = "hidden",
		pointer_events = "none",
		beside(),
	}
	-- Its edge, dragged: wider or narrower, bouncing past either end;
	-- double-clicked, back to its default, easing there (ds/seam). It takes
	-- the pointer `seam.REACH` each side of the tree's edge, under the band.
	local edge: Node? = nil
	if width_state then
		local over = ui.use_state(false)
		local held_edge = ui.use_state(false)
		local drag = edge_drag :: seam.Drag
		local from_x, from_w, pressing = 0, 0, false
		local least, most = props.least or 0, props.most or math.huge
		local function dragging(on: boolean)
			held_edge:set(on)
			if props.dragging and props.dragging:peek() ~= on then
				props.dragging:set(on)
			end
		end
		edge = ui.div {
			absolute = true,
			z = 2,
			left = function(): number
				return ui.raw(dock_w:get() - seam.REACH, "the seam's reach either side of the tree's edge")
			end,
			top = head_h,
			bottom = 0,
			w = ui.raw(2 * seam.REACH, "the seam's reach either side"),
			cursor = "col_resize",
			label = locale.t("ds.files_pane.resize_tree"),
			pointer_events = function(): PointerEvents
				return if shown:get() and not whole() and dock_w:get() > 0.5 then "auto" else "none"
			end,
			on_hover = function(inside: boolean)
				over:set(inside)
			end,
			on_press = function(e)
				if e.clicks == 2 then
					local ws = width_state :: State<number>
					ws:set(props.default or theme_w)
					return
				end
				from_x, from_w, pressing = e.x, (width_state :: State<number>):peek(), true
				dragging(true)
			end,
			on_pointer_move = function(e)
				if pressing then
					local want = from_w + e.x - from_x
					drag.at(want, least, most)
					local ws = width_state :: State<number>
					ws:set(math.clamp(want, least, most))
				end
			end,
			on_release = function()
				pressing = false
				dragging(false)
				drag.release()
			end,
			-- What lights: the tree's edge, stronger.
			ui.div {
				absolute = true,
				left = ui.raw(seam.REACH - 1, "the tree's edge, in the seam's reach"),
				top = 0,
				bottom = 0,
				w = 2,
				rounded = "$radius.full",
				pointer_events = "none",
				bg = "$border.strong",
				opacity = seam.lit(function(): boolean
					return over:get() or held_edge:get()
				end, drag),
			},
		}
	end

	-- The file, beside the tree's room; as the tree goes, its header keeps
	-- room for the toggle instead, so its name comes to meet it.
	local viewer_props: FileViewer.Props = table.clone(props.viewer)
	viewer_props.head = props.head
	viewer_props.lead = function(): number
		return math.max(0, lead_w - dock_w:get())
	end
	viewer_props.grow = 1
	viewer_props.min_w = 0
	viewer_props.h_full = true
	-- (With no file, what its side says goes before it has no room to say it.)
	viewer_props.opacity = function(): number
		local open = if type(doc) == "function" then doc() else doc:get()
		return if open then 1 else math.clamp((room:get() - dock_w:get() - 100) / 100, 0, 1)
	end

	-- The toggle, at the band's start, over whichever head is under it.
	local toggle_button = ui.div {
		absolute = true,
		z = 3,
		left = pad,
		top = (head_h - square) / 2,
		IconButton {
			label = locale.t("ds.files_pane.toggle_tree"),
			tooltip = locale.s("ds.files_pane.toggle_tree_tip", { keys = ui.shortcut_text(KEYS) }),
			tooltip_side = "bottom",
			size = "lg",
			tone = "quiet",
			on_click = toggle,
			SidebarIcon { open = lit, size = 16 },
		},
	}

	local box: { [any]: any } = {}
	for k, v in props :: any do
		if not OWN[k] then
			box[k] = v
		end
	end
	box.relative = true
	box.overflow = "hidden"
	box.on_resize = function(size: { w: number, h: number })
		if room:peek() ~= size.w then
			room:set(size.w)
		end
		if not settled then
			ui.after(0, function()
				settled = true
			end)
		end
	end
	box.on_key = function(e): boolean?
		if e.cmd and e.shift and not e.alt and not e.ctrl and string.lower(e.key) == "e" then
			toggle()
			return true
		end
		return nil
	end
	-- The tree's room beside the file.
	box[#box + 1] = ui.div {
		shrink = 0,
		h_full = true,
		w = dock_w,
	}
	-- (The toggle and the tree first, as they stand left of the file: Tab
	-- reaches them before it. Drawn over it by `z`.)
	box[#box + 1] = toggle_button
	box[#box + 1] = tree
	box[#box + 1] = FileViewer(viewer_props)
	if edge then
		box[#box + 1] = edge
	end
	return ui.row(box)
end)

-- Typed as what it is, a table called as a function: through `__call` the
-- checker would match a call's props exactly rather than as an argument.
type FilesPane = typeof(M) & ((props: Props) -> Node)
local FilesPane: FilesPane = setmetatable(M, {
	__call = function(_, props: Props): Node
		return made(props)
	end,
}) :: any -- (the checker doesn't see a `__call` table as a function)
return FilesPane
