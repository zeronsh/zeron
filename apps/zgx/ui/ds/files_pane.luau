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
-- With no file open and too little room for one beside it, the tree takes the
-- whole pane, until a file is chosen.
--
-- `FilesPane { shown = state, tree = { FileTree's props }, viewer = { FileViewer's props }, head = "line" }`.

local FileTree = require("@self/file_tree")
local FileViewer = require("@self/file_viewer")
local SidebarIcon = require("@self/sidebar_icon")
local IconButton = require("@self/icon_button").IconButton
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
	-- The pane's last width, if the caller keeps it: a pane made again starts
	-- exactly as it was.
	room: State<number>?,
	-- Filled with `toggle`, for a command bound outside.
	commands: { [string]: () -> () }?,
	[any]: any,
}

local OWN = { shown = true, tree = true, viewer = true, head = true, width = true, commands = true, room = true, children = true }

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
	local tree_w = props.width or num("zeron.files.tree", 288)
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
	-- No file open, and no room for one beside the tree: the tree has it all.
	local doc = props.viewer.doc :: any
	local function whole(): boolean
		local open = if type(doc) == "function" then doc() else doc:get()
		local w = room:get()
		return shown:get() and open == nil and w > 0 and w < tree_w + FILE_LEAST
	end
	-- The tree's room beside the file, eased as it comes and goes by the tree's own
	-- slide.
	local function dock_target(): number
		return if whole() then room:get() elseif shown:get() then tree_w else 0
	end
	local dock_w = ui.use_state(dock_target())
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
		if not settled or follows then
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
		local to = if on then 1 else 0
		if settled then
			fading = anime.animate(ink, motion.params({ value = to, duration = INK.duration }))
		else
			ink:set(to)
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
		return math.max(tree_w, dock_w:get())
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
			return pad + square - (dock_w:get() - tree_now())
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
			x = function(): number
				return dock_w:get() - tree_now()
			end,
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
	local tree = ui.div { absolute = true, left = 0, top = 0, right = 0, bottom = 0, pointer_events = "none", beside() }

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
