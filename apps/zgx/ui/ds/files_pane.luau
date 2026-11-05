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
-- file's while it doesn't, and the file's name slides to meet it. Hiding
-- slides the tree away whole under the pane's edge, its ink going first,
-- the file taking the room as it goes (the sidebar's motion); showing is
-- the same back. `shown` is the caller's, so it can be remembered.
--
-- A pane too narrow to hold both (less than a file's least width beside
-- the tree: Zeron's narrowest pane, 360) hides the tree; shown there, it
-- floats over the file instead: a panel of the menus' glass (ds/glass),
-- one inset in from the pane's start, the band and its foot, its corner
-- the radius scale's and all in it concentric (FileTree's `float`). It
-- slides in from the start, and goes as a file is picked, the toggle
-- pressed, the file clicked or Esc.
--
-- `FilesPane { shown = state, tree = { FileTree's props }, viewer = { FileViewer's props }, head = "line" }`.

local FileTree = require("@self/file_tree")
local FileViewer = require("@self/file_viewer")
local SidebarIcon = require("@self/sidebar_icon")
local IconButton = require("@self/icon_button").IconButton
local glass = require("@self/glass")
local motion = require("@self/motion")
local locale = require("@self/locale")

export type Props = {
	-- Whether the tree shows beside the file (the caller's, remembered).
	shown: State<boolean>,
	tree: FileTree.Props,
	viewer: FileViewer.Props,
	-- The file's header shape (ds/file_viewer): one line by default.
	head: FileViewer.Head?,
	-- The tree's width beside the file (the theme's).
	width: number?,
	-- The pane's last width and whether the drawer is out, if the caller
	-- keeps them: a pane made again starts exactly as it was.
	room: State<number>?,
	drawer: State<boolean>?,
	-- Whether the tree has room beside the file, as the window's layout
	-- says (an app's fit, as Zeron's Files column): not docked, it floats.
	-- Not given, the pane decides by its own width (below).
	docked: (() -> boolean)?,
	-- Filled with `toggle`, for a command bound outside (it knows which way
	-- the tree shows here: beside the file, or over it).
	commands: { [string]: () -> () }?,
	[any]: any,
}

local OWN = { docked = true, shown = true, tree = true, viewer = true, head = true, width = true, commands = true, room = true, drawer = true, children = true }

-- How the tree moves: the sidebar's slide, and its ink a beat quicker.
local SLIDE = { duration = 260, ease = "out(3)" }
local INK = { duration = 160 }
-- The keys that hide and show it.
local KEYS = "cmd+shift+e"

local function num(token: string, fallback: number): number
	return (ui.theme.get(token) :: number?) or fallback
end

local M = {}
M.keys = KEYS

local made = ui.component(function(props: Props): Node
	local shown = props.shown
	local tree_w = props.width or num("zeron.files.tree", 288)
	local view_min = num("zeron.files.view", 360)
	local head_h = num(if props.head == "stack" then "zeron.files.stack" else "zeron.files.head", 40)
	local square = num("zeron.icon_button.lg", 28)
	local pad = num("space.2", 8)
	-- Room the heads keep at their start: the inset, the toggle, a step.
	local lead_w = pad + square + pad

	-- How wide the pane is, and so whether it holds the tree beside a file.
	-- (Kept by the caller, with whether the drawer is out, a pane made again
	-- — a reload, a template — knows both on its first frame, so nothing
	-- opens, closes or fades that was at rest.)
	local room = props.room or ui.use_state(0)
	-- (Computed: a resize moves `room` every frame, and what follows only
	-- cares when the answer flips.)
	local docked = props.docked
	local narrow_c = ui.computed(function(): boolean
		local r = room:get()
		-- (The window's layout decides when it says.)
		if docked then
			return r > 0 and not docked()
		end
		return r > 0 and r - tree_w < view_min
	end)
	local function narrow(): boolean
		return narrow_c:get()
	end
	-- Over the file (narrow), whether the tree is out.
	local drawer = props.drawer or ui.use_state(false)
	-- Whether the tree is out at all, either way.
	local out_c = ui.computed(function(): boolean
		return if narrow() then drawer:get() else shown:get()
	end)
	local function out(): boolean
		return out_c:get()
	end
	-- How far out it is: its width beside the file, or how far the drawer
	-- has come. Eased, once the pane has been measured (before that, set:
	-- a pane opens with its tree where it was, not sliding there).
	local far = ui.use_state(if out() then tree_w else 0)
	local settled = false
	local easing: any = nil
	ui.effect(function()
		local to = if out() then tree_w else 0
		if easing then
			easing:cancel()
			easing = nil
		end
		if settled then
			easing = anime.animate(far, motion.params({ value = to, duration = SLIDE.duration, ease = SLIDE.ease }))
		else
			far:set(to)
		end
	end)
	-- The tree's room beside the file: its width while it's out there, none
	-- over it. Eased as it comes and goes, on the user's toggle by the
	-- tree's own slide; when the window's layout docks or floats it, on the
	-- columns' motion (ds/motion), which the panel's edge rides too, so the
	-- file beside it holds still as one makes the room the other gives up.
	local function dock_target(): number
		return if not narrow() and shown:get() then tree_w else 0
	end
	local dock_w = ui.use_state(dock_target())
	local dock_easing: any = nil
	local was_narrow = narrow()
	ui.effect(function()
		local to = dock_target()
		local flipped = narrow() ~= was_narrow
		was_narrow = narrow()
		if dock_easing then
			dock_easing:cancel()
			dock_easing = nil
		end
		if not settled then
			dock_w:set(to)
		elseif flipped then
			dock_easing = anime.animate(dock_w, motion.column(dock_w:peek(), to))
		else
			dock_easing = anime.animate(dock_w, motion.params({ value = to, duration = SLIDE.duration, ease = SLIDE.ease }))
		end
	end)
	-- Leaving narrow closes the drawer (once measured: before, the pane's
	-- width isn't known, and an open drawer is kept as it was).
	ui.effect(function()
		if room:peek() > 0 and not narrow() and drawer:peek() then
			drawer:set(false)
		end
	end)
	-- Its ink (and the toggle's glyph): on while it is out; the ink eased
	-- once the pane has been measured, set before (as `far` is).
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
		local to = if on then 1 else 0
		if settled then
			fading = anime.animate(ink, motion.params({ value = to, duration = INK.duration }))
		else
			ink:set(to)
		end
	end)
	local function toggle()
		if narrow() then
			drawer:set(not drawer:peek())
		else
			shown:set(not shown:peek())
		end
	end
	if props.commands then
		props.commands.toggle = toggle
	end

	-- The tree's props, beside the file or floating over it.
	local function tree_props_for(floating: boolean): FileTree.Props
		local tp: FileTree.Props = table.clone(props.tree)
		local on_open = tp.on_open
		tp.on_open = function(path: string)
			-- A file picked from the floating tree: it goes.
			if narrow() then
				drawer:set(false)
			end
			if on_open then
				on_open(path)
			end
		end
		if floating then
			tp.float = true
			tp.head = FileTree.float_head()
			tp.lead = 0
			tp.w_full = true
		else
			tp.head = head_h
			tp.lead = lead_w
			-- The toggle's room, in the tree's own points as it slides (its
			-- right edge less where the tree's start is): the head is cut
			-- there, so it never shows under the toggle. At rest that's left
			-- of the head's field (`lead`), so nothing changes there.
			tp.head_cut = function(): number
				return pad + square - (dock_w:get() - tree_w)
			end
			tp.w = tree_w
		end
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
			w = tree_w,
			x = function(): number
				return dock_w:get() - tree_w
			end,
			-- (Its ink goes as the user hides it; leaving for the floating
			-- one, it slides away whole with its room.)
			opacity = function(): number
				return if narrow() then 1 else ink:get()
			end,
			-- (Gone, its ink is out and nothing in it can be touched.)
			pointer_events = function(): PointerEvents
				return if not narrow() and dock_w:get() > 0.5 then "auto" else "none"
			end,
			FileTree(tree_props_for(false)),
			-- Its edge, from under the band (the band's hairline runs across
			-- both heads).
			ui.div { absolute = true, right = 0, top = head_h, bottom = 0, w = 1, bg = "$border.subtle" },
		}
	end
	-- Over the file (narrow): a floating panel, one inset in from the pane's
	-- start, the band and its foot (the shipping app's floating card's 12:
	-- crates/ui/src/shell/session_info.rs), as wide as the tree beside a
	-- file (or the pane less that inset either side). It comes as that card
	-- does: fading in as it slides the last 12 points from the start.
	local inset = num("space.3", 12)
	local function float_w(): number
		return math.max(0, math.min(tree_w, room:get() - 2 * inset))
	end
	local function floating(): Node
		local panel: ContainerProps = {
			id = "files-tree-float",
			absolute = true,
			-- (Points: an absolute box's insets don't take tokens; `inset` is
			-- the theme's space.3.)
			left = ui.raw(inset, "the floating panel's inset (space.3)"),
			top = ui.raw(head_h + inset, "one inset under the band"),
			bottom = ui.raw(inset, "the floating panel's inset (space.3)"),
			w = float_w,
			rounded = FileTree.FLOAT_CORNER,
			overflow = "hidden",
			x = function(): number
				return -inset * (1 - far:get() / tree_w)
			end,
			opacity = function(): number
				return far:get() / tree_w
			end,
			pointer_events = function(): PointerEvents
				return if far:get() > 0.5 then "auto" else "none"
			end,
			FileTree(tree_props_for(true)),
		}
		glass.style(panel :: any, "menu")
		return ui.column(panel)
	end
	-- (A real swap: the tree beside the file and the floating one are made
	-- differently; what they show is the caller's, kept.)
	-- (Beside while its room is still going: it slides away with it.)
	local tree = ui.presence(function(): string
		return if narrow() and dock_w:get() < 0.5 then "float" else "beside"
	end, function(which: string): Node
		return if which == "float" then floating() else beside()
	end, { absolute = true, left = 0, top = 0, right = 0, bottom = 0, pointer_events = "none", z = 2 })

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
	-- While it floats over the file, a click anywhere else closes it: on
	-- the band over it, the file beside it, or the room under it. (Each
	-- part a box of its own, so none lies under the panel.)
	local function close_float()
		drawer:set(false)
	end
	local catcher = ui.show(function(): boolean
		return narrow() and drawer:get()
	end, ui.div {
		-- (Where a click closes the panel: nothing to read or reach.)
		aria_hidden = true,
		absolute = true,
		z = 1,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		pointer_events = "none",
		ui.div {
			id = "files-tree-dismiss-band",
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			h = ui.raw(head_h + inset, "the band and the inset under it"),
			pointer_events = "auto",
			on_press = close_float,
		},
		ui.div {
			id = "files-tree-dismiss",
			label = locale.t("ds.files_pane.close_tree"),
			absolute = true,
			left = function(): any
				return ui.raw(inset + float_w(), "past the floating tree")
			end,
			right = 0,
			top = 0,
			bottom = 0,
			pointer_events = "auto",
			on_press = close_float,
		},
	})

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
		-- Esc closes the floating tree, as it would any floating panel.
		if e.key == "Escape" and narrow() and drawer:peek() then
			drawer:set(false)
			return true
		end
		return nil
	end
	-- The tree's room beside the file (none over it).
	box[#box + 1] = ui.div {
		shrink = 0,
		h_full = true,
		w = dock_w,
	}
	-- (The toggle and the tree first, as they stand left of the file: Tab
	-- reaches them before it. Drawn over it by `z`, the click-catcher
	-- between.)
	box[#box + 1] = toggle_button
	box[#box + 1] = tree
	box[#box + 1] = FileViewer(viewer_props)
	box[#box + 1] = catcher
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
