-- A file, open: Zeron's preview (crates/ui/src/files/preview.rs), and a step
-- past it. It lives in a sidebar, usually: a pane 360 to 520 points wide
-- beside the chat. So its header is made for that, and more room only brings
-- controls back out.
--
-- The header says what the file is, on one line (the band a files pane
-- shares with its tree's head, ds/files_pane): its icon, the folders it is
-- in (each a click away from the tree; too long, they fade at their start
-- and give way before the name does), its name, a dot if it has unsaved
-- changes, and its git status as the tree's letter. Two other shapes,
-- `head = "stack"` (the name over its place and status, two lines) and
-- `head = "path"` (the whole path in the code's face, its status letter
-- leading, as the git pages' file cards), stand beside it for comparison.
-- What stays at any width: Save, while there is something to save, Find,
-- the switch a file's kind needs (Preview and Source; an image's zoom,
-- down to its level), and the menu, which holds everything else. At 440
-- points Wrap comes out of the menu, at 640 Copy Path, Reveal and Open.
--
-- Under the header, the file:
--
-- - Code in the design system's code editor (`ds/code_editor`): typed into,
--   folded, undone; ⌘S saves, ⌘F finds, ⌃G goes to a line. What was typed
--   is kept while another file is looked at, until it is saved. If the file
--   changes on disk under unsaved edits, a banner asks which to keep.
-- - Markdown, set for reading (the transcript's markdown), or its source to
--   edit; what is typed shows in the preview.
-- - An image, fitted (never past its own size), over a checkerboard where
--   it's transparent. ⌘+ ⌘− zoom, ⌘0 fits, ⌘1 is its own size; a pinch or
--   ⌘-scroll zooms about the pointer; scrolling pans; a double click
--   toggles between fitted and its own size, there.
-- - Or why it can't be shown: binary, too big, gone, unreadable; each with
--   what can be done instead.
--
-- At the foot of a preview, quietly: its language, lines and size (an
-- image's dimensions and zoom); the editor has a status bar of its own.
-- Opening shows nothing for a beat, then the shape of lines (a quick file
-- never flashes).
--
-- `FileViewer { doc = state, on_save = function(path, text) … end }`: a doc is
-- { path, state, text?, image?, size?, status?, error? } (`state`: "loading",
-- "ready", "binary", "large", "gone" or "error"). The handlers say what the
-- actions do. Without `on_save`, or with `readonly`, nothing can be typed.

local Icon = require("@self/icon")
local FileIcon = require("@self/file_icons")
local Kbd = require("@self/kbd")
local Button = require("@self/button")
local FadeText = require("@self/fade_text")
local IconButtonModule = require("@self/icon_button")
local IconButton = IconButtonModule.IconButton
local ToggleButton = require("@self/toggle_button")
local Segmented = require("@self/segmented")
local Markdown = require("@self/markdown")
local Menu = require("@self/menu")
local banner = require("@self/banner")
local badge = require("@self/badge")

-- The code: the design system's editor, coloured by the highlighter.
local CodeEditor = require("@self/code_editor")
local Syntax = require("@self/syntax")
local SelectionMenu = require("@self/selection_menu")
-- (Zoom eases as the lightbox's does: one system.)
local Motion = require("@self/motion")
local locale = require("@self/locale")

export type Status = "modified" | "added" | "untracked" | "renamed" | "deleted" | "conflict" | "ignored"
export type Head = "line" | "stack" | "path"
export type Doc = {
	path: string,
	-- "loading", "ready", or why it can't show: "binary", "large", "gone",
	-- "error".
	state: string,
	text: string?,
	-- An image's source (a path), and its size in pixels if known.
	image: string?,
	width: number?,
	height: number?,
	size: number?,
	status: Status?,
	error: string?,
	-- Where it is on disk, for a copied path.
	absolute: string?,
	-- Its text is what's on disk after a save found it changed there: the
	-- editor keeps what was typed and asks which to keep, as it does when
	-- the file changes under unsaved edits.
	conflict: boolean?,
	-- Deleted on disk while it had unsaved edits: they're kept, and it
	-- asks whether to save them (making the file again) or let them go.
	deleted: boolean?,
}
export type Props = {
	doc: State<Doc?> | () -> Doc?,
	-- The project's name, leading the trail.
	root: string?,
	-- How big a file opens here (Zeron's 8 MB), for "too big"'s words.
	limit: number?,
	on_crumb: ((folder: string) -> ())?,
	on_reveal: ((path: string) -> ())?,
	on_open_editor: ((path: string) -> ())?,
	-- A file it can't show, opened in the app the system gives it.
	on_open_default: ((path: string) -> ())?,
	-- Whether Reveal and the Opens apply now (the files are on this
	-- machine): followed; without it, they apply whenever their handler is given.
	openable: (() -> boolean)?,
	-- Entries others add to its menu (plugins'), asked for as it opens: a
	-- `label` and `on_select`, or a `heading`.
	more_menu: ((path: string, kind: "file" | "folder") -> { any })?,
	on_retry: ((path: string) -> ())?,
	-- Saving: the file's new text. Without it, nothing can be typed.
	on_save: ((path: string, text: string) -> ())?,
	-- Unsaved edits let go of, for a file deleted on disk (the caller shows
	-- it gone).
	on_discard: ((path: string) -> ())?,
	-- Whether a file has unsaved changes, as that changes (a tree's dot).
	on_dirty: ((path: string, dirty: boolean) -> ())?,
	-- Whether long lines wrap to begin with, and as that changes (Settings →
	-- Files' Word wrap); the header's toggle (⌥Z) still turns it.
	wrap_default: (() -> boolean)?,
	-- Saving on its own: the milliseconds after the typing stops, or nil
	-- while it's off (Settings → Files' Autosave).
	autosave: (() -> number?)?,
	-- The code's size in points, as that changes (Settings' code font
	-- size); nil, the design system's. A change builds the editor again,
	-- the edits and its place kept.
	code_size: (() -> number?)?,
	-- A line for the code's editor to go to (ds/code_editor's `reveal`).
	reveal_line: State<any>?,
	-- How the keyboard leaves the code (ds/code_editor's).
	escape_hint: string?,
	-- A Markdown preview's pictures and links, in the file at `path`: what
	-- a picture (`src` as written) shows from, nil while it can't; and a
	-- link that isn't the web's, followed. Without them, pictures are their
	-- alt text and such links only styled.
	picture: ((path: string, src: string) -> string?)?,
	on_link: ((path: string, target: string) -> ())?,
	readonly: boolean?,
	-- What stands between a code file's lines (`ds/code_editor`'s).
	blocks: any?,
	block_view: CodeEditor.BlockView?,
	on_add_block: ((line: number) -> ())?,
	on_block_moved: ((key: string, line: number) -> ())?,
	-- Unsaved edits by path, if the caller keeps them (the viewer's own by
	-- default): a viewer made again (another folder and back) finds them.
	drafts: State<{ [string]: { text: string, base: string } }>?,
	-- Where each file's editor stood (scroll and selection), if the caller
	-- keeps it: a viewer made again shows each file where it was.
	places: State<{ [string]: CodeEditor.Spot }>?,
	-- Its header's shape (above; `line` by default), and room kept at the
	-- header's start for a control laid over it (a pane's tree toggle), in
	-- points or a function of them.
	head: Head?,
	lead: (number | () -> number)?,
	-- Declared hooks for plugins (zgx's `hook`): the header's actions.
	hooks: { actions: string? }?,
	[any]: any,
}

-- Geometry: the header's height (the pane's band, one line or two), the
-- foot's, and the pane widths where more comes out of the menu: an image's
-- zoom and Markdown's words, then Wrap, then the rest.
local function head_height(head: Head?): number
	return (ui.theme.get(if head == "stack" then "zeron.files.stack" else "zeron.files.head") :: number?) or 40
end
local FOOT = 26
local ROOM_SWITCH, ROOM_TOOLS, ROOM_ALL = 440, 440, 640
-- How long opening shows nothing before the shape of lines.
local QUIET = 140
-- Zoom: the steps ⌘+ and ⌘− walk, and the furthest out and in.
local STEPS = { 0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 1, 1.25, 1.5, 2, 3, 4, 6, 8, 12, 16 }
local MIN_ZOOM, MAX_ZOOM = 0.02, 32
-- The checks behind a transparent image.
local CHECK = 8

-- Each status's name: its message's key.
local SAID: { [string]: string } = {
	modified = "ds.file_viewer.status.modified",
	added = "ds.file_viewer.status.added",
	untracked = "ds.file_viewer.status.untracked",
	renamed = "ds.file_viewer.status.renamed",
	deleted = "ds.file_viewer.status.deleted",
	conflict = "ds.file_viewer.status.conflict",
	ignored = "ds.file_viewer.status.ignored",
}

-- Sizes as people say them.
local function bytes(n: number?): string
	if not n then
		return ""
	end
	if n < 1024 then
		return locale.s("ds.file_viewer.bytes", { n = tostring(n) })
	elseif n < 1024 * 1024 then
		return string.format("%.0f KB", n / 1024)
	elseif n < 1024 * 1024 * 1024 then
		local mb = n / (1024 * 1024)
		return if mb < 10 then string.format("%.1f MB", mb) else string.format("%.0f MB", mb)
	end
	return string.format("%.1f GB", n / (1024 * 1024 * 1024))
end
local function base_name(path: string): string
	return string.match(path, "[^/]+$") or path
end
local function ext(path: string): string
	return string.lower(string.match(base_name(path), "%.([^%.]+)$") or "")
end
-- `text` with its `k`th task (`- [ ]`, `1. [x]`, in a quote too; not in
-- a fenced code block) ticked (`done`) or cleared: what a Markdown
-- preview's box does. Unchanged when there's no such task.
local function tick_task(text: string, k: number, done: boolean): string
	local n = 0
	local fence: string? = nil
	local out: { string } = {}
	local changed = false
	for line, nl in string.gmatch(text, "([^\n]*)(\n?)") do
		if line == "" and nl == "" then
			break
		end
		local f = string.match(line, "^%s*([`~][`~][`~]+)")
		if f then
			if not fence then
				fence = string.sub(f, 1, 1)
			elseif string.sub(f, 1, 1) == fence then
				fence = nil
			end
		elseif not fence and not changed then
			local head, box, rest = string.match(line, "^([%s>]*[-*+]%s+)%[([ xX])%](.*)$")
			if not head then
				head, box, rest = string.match(line, "^([%s>]*%d+[.)]%s+)%[([ xX])%](.*)$")
			end
			if head and box then
				n += 1
				if n == k then
					line = `{head}[{if done then "x" else " "}]{rest}`
					changed = true
				end
			end
		end
		table.insert(out, line .. nl)
	end
	return if changed then table.concat(out) else text
end

local function is_markdown(path: string): boolean
	local e = ext(path)
	return e == "md" or e == "markdown" or e == "mdx"
end
local function lines_in(text: string): number
	local _, n = string.gsub(text, "\n", "")
	if text ~= "" and string.sub(text, -1) ~= "\n" then
		n += 1
	end
	return n
end

-- A header's icon button: quiet until touched, the tree's tools' size
-- (the two heads share the band); its keys in its tooltip, under it
-- (above is the pane's edge).
-- (`key`: the name's message; a tooltip is a string, read as it's built.)
local function tip(key: string, keys: string?): string
	local label = locale.s(key)
	return if keys then `{label}  {ui.shortcut_text(keys)}` else label
end
local function action(key: string, icon: Icon.Name | () -> Icon.Name, on_click: () -> (), keys: string?): Node
	local button: IconButtonModule.Props = { label = locale.t(key), tooltip = tip(key, keys), tooltip_side = "bottom", icon = icon, size = "lg", tone = "quiet", on_click = on_click }
	return IconButton(button)
end
-- A header's toggle (find, wrap): the same, holding the selection's wash
-- while on.
local function toggle(key: string, icon: Icon.Name, on: State<boolean>, keys: string?): Node
	local button: ToggleButton.Props = { label = locale.t(key) :: any, tooltip = tip(key, keys), tooltip_side = "bottom", icon = icon, on = on, size = "lg", tone = "quiet" }
	return ToggleButton(button)
end

-- What shows in place of a file that can't: a glyph, what happened, why,
-- and what can be done instead.
-- (Its words as functions, so they follow the locale; a way's label a
-- message.)
type Way = { label: Label, on_click: () -> (), primary: boolean? }
local function notice(icon: Icon.Name, title: () -> string, body: () -> string, ways: { Way }?): Node
	local buttons: ContainerProps = { gap = "$space.2", mt = "$space.2" }
	local list: { Way } = ways or {}
	for _, w in list do
		table.insert(buttons, ui.button {
			label = w.label,
			h = 28,
			px = "$space.3",
			rounded = "$radius.lg",
			bg = if w.primary then "$zeron.primary.bg" else "$zeron.secondary.bg",
			color = if w.primary then "$zeron.primary.text" else "$text.primary",
			hover = { bg = if w.primary then "$zeron.primary.hover" else "$zeron.secondary.hover" },
			active = { bg = if w.primary then "$zeron.primary.active" else "$zeron.secondary.active" },
			on_click = w.on_click,
			ui.text { w.label, text_size = "$font.size.sm", weight = "$font.weight.medium" },
		})
	end
	return ui.column {
		w_full = true,
		h_full = true,
		items = "center",
		justify = "center",
		px = "$space.6",
		gap = "$space.2",
		ui.div { color = "$text.tertiary", mb = "$space.1", Icon { name = icon, size = 24 } },
		ui.text { title, literal = true, text_size = "$font.size.lg", weight = "$font.weight.medium", color = "$text.primary", align = "center", wrap = true, max_w = 420 },
		ui.text { body, literal = true, text_size = "$font.size.md", color = "$text.secondary", align = "center", wrap = true, max_w = 420 },
		if ways and #ways > 0 then ui.row(buttons) else nil,
	}
end

-- Opening: nothing for a beat, then lines' shapes, breathing.
local WIDTHS = { 0.42, 0.66, 0.58, 0.3, 0, 0.72, 0.5, 0.62, 0.38, 0, 0.55, 0.46, 0.7, 0.34 }
local function loading(): Node
	local bars: ContainerProps = { w_full = true, gap = "$space.2", p = "$space.4", opacity = 0 }
	for i, w in WIDTHS do
		table.insert(bars, ui.div {
			h = 12,
			w = if w > 0 then `{math.floor(w * 100)}%` else 0,
			ml = if i % 3 == 0 then "$space.5" else 0,
			rounded = "$radius.sm",
			bg = "$zeron.secondary.bg",
		})
	end
	bars.on_mount = function(el)
		anime.animate(el, Motion.params({ opacity = { to = 1, duration = 240, delay = QUIET } }))
	end
	return ui.column(bars)
end

-- An image: fitted (never past its own size) or zoomed, over the checks.
local function image_view(doc: Doc, zoom_label: State<string>, commands: { [string]: () -> () }): Node
	local frame = ui.ref()
	-- Its own size: from the doc, else measured from a copy laid out at it.
	local natural = ui.use_state({ w = doc.width or 0, h = doc.height or 0 })
	-- The view: its scale (nil while fitted) and where its middle sits from
	-- the frame's middle.
	local fitted = ui.use_state(true)
	local view = { s = 1, x = 0, y = 0 }
	local tick = ui.use_state(0)
	local motion: any = nil
	local function redraw()
		tick:set(tick:peek() + 1)
	end
	local function fit_scale(): number
		local nat = natural:peek()
		local fw, fh = frame.width or 0, frame.height or 0
		if nat.w <= 0 or nat.h <= 0 or fw <= 0 or fh <= 0 then
			return 1
		end
		return math.min(1, (fw - 48) / nat.w, (fh - 48) / nat.h)
	end
	local function say_zoom()
		zoom_label:set(`{math.floor(view.s * 100 + 0.5)}%`)
	end
	-- Keeps the image from leaving the frame: smaller than it, centred;
	-- larger, its edges never come inside the frame's.
	local function clamp_view()
		local nat = natural:peek()
		local fw, fh = frame.width or 0, frame.height or 0
		local iw, ih = nat.w * view.s, nat.h * view.s
		local mx = math.max(0, (iw - fw) / 2)
		local my = math.max(0, (ih - fh) / 2)
		view.x = math.clamp(view.x, -mx, mx)
		view.y = math.clamp(view.y, -my, my)
	end
	-- Where a zoom easing now ends, while one does.
	local heading: number? = nil
	-- To scale `s`, keeping the point at (px, py) from the frame's middle
	-- where it is; `animate` eases there.
	local function zoom_to(s: number, px: number?, py: number?, animate: boolean?)
		s = math.clamp(s, MIN_ZOOM, MAX_ZOOM)
		local ax, ay = px or 0, py or 0
		local k = s / view.s
		local target = { s = s, x = ax - (ax - view.x) * k, y = ay - (ay - view.y) * k }
		if motion then
			motion:cancel()
			motion = nil
		end
		heading = nil
		if animate then
			local from = { s = view.s, x = view.x, y = view.y }
			local probe = { s = view.s, x = view.x, y = view.y }
			view.s, view.x, view.y = target.s, target.x, target.y
			clamp_view()
			local to = { s = view.s, x = view.x, y = view.y }
			view.s, view.x, view.y = from.s, from.x, from.y
			local params: TableParams = {
				s = to.s,
				x = to.x,
				y = to.y,
				duration = Motion.SNAPPY.zoom,
				ease = Motion.SNAPPY.ease,
				onUpdate = function()
					view.s, view.x, view.y = probe.s, probe.x, probe.y
					redraw()
				end,
				onComplete = function()
					heading = nil
				end,
			}
			heading = to.s
			motion = anime.animate(probe, Motion.params(params))
			view.s = to.s
			say_zoom()
			view.s = from.s
		else
			view.s, view.x, view.y = target.s, target.x, target.y
			clamp_view()
			redraw()
			say_zoom()
		end
	end
	local function fit(animate: boolean?)
		fitted:set(true)
		local s = fit_scale()
		if animate then
			zoom_to(s, 0, 0, true)
			-- (Centred, as fitted is.)
			view.x, view.y = 0, 0
		else
			view.s, view.x, view.y = s, 0, 0
			redraw()
			say_zoom()
		end
	end
	local function step(dir: number)
		fitted:set(false)
		-- (From where a zoom on its way is going, not where it is midway:
		-- quick presses each take a step, none repeated.)
		local s = heading or view.s
		local next_s = if dir > 0 then STEPS[#STEPS] else STEPS[1]
		if dir > 0 then
			for _, v in STEPS do
				if v > s + 0.001 then
					next_s = v
					break
				end
			end
		else
			for i = #STEPS, 1, -1 do
				if STEPS[i] < s - 0.001 then
					next_s = STEPS[i]
					break
				end
			end
		end
		zoom_to(next_s, 0, 0, true)
	end
	commands.zoom_in = function()
		step(1)
	end
	commands.zoom_out = function()
		step(-1)
	end
	commands.fit = function()
		fit(true)
	end
	commands.actual = function()
		fitted:set(false)
		zoom_to(1, 0, 0, true)
	end
	commands.toggle = function()
		if fitted:peek() then
			commands.actual()
		else
			fit(true)
		end
	end
	-- Laid out (or the frame resized) while fitted: fitted again.
	local function relayout()
		if fitted:peek() then
			fit(false)
		else
			clamp_view()
			redraw()
		end
	end

	local function box(): (number, number, number, number)
		tick:get()
		natural:get()
		local nat = natural:peek()
		local fw, fh = frame.width or 0, frame.height or 0
		local w, h = nat.w * view.s, nat.h * view.s
		return (fw - w) / 2 + view.x, (fh - h) / 2 + view.y, w, h
	end
	local measure = if doc.width
		then nil
		else ui.image {
			src = doc.image or "",
			absolute = true,
			left = 0,
			top = 0,
			opacity = 0,
			pointer_events = "none",
			on_resize = function(size)
				if size.w > 0 and size.h > 0 then
					natural:set({ w = size.w, h = size.h })
					ui.after(0, relayout)
				end
			end,
		}
	return ui.div {
		ref = frame,
		w_full = true,
		h_full = true,
		relative = true,
		overflow = "hidden",
		focusable = true,
		label = locale.t("ds.file_viewer.image_named", { name = base_name(doc.path) }),
		focus = { border = 0 },
		cursor = function(): Cursor
			tick:get()
			local nat = natural:get()
			local over = nat.w * view.s > (frame.width or 0) or nat.h * view.s > (frame.height or 0)
			return if over then "grab" else "default"
		end,
		on_resize = function()
			relayout()
		end,
		on_mount = function()
			ui.after(0, relayout)
		end,
		on_press = function(e)
			if e.clicks == 2 then
				local px = e.local_x - (frame.width or 0) / 2
				local py = e.local_y - (frame.height or 0) / 2
				if fitted:peek() then
					fitted:set(false)
					zoom_to(1, px, py, true)
				else
					fit(true)
				end
			end
		end,
		on_wheel = function(e): boolean?
			if e.cmd or e.ctrl then
				fitted:set(false)
				local px = e.local_x - (frame.width or 0) / 2
				local py = e.local_y - (frame.height or 0) / 2
				zoom_to(view.s * math.exp(-e.dy * 0.01), px, py)
				return true
			end
			view.x -= e.dx
			view.y -= e.dy
			clamp_view()
			redraw()
			return true
		end,
		on_pinch = function(e): boolean?
			fitted:set(false)
			local px = e.local_x - (frame.width or 0) / 2
			local py = e.local_y - (frame.height or 0) / 2
			zoom_to(view.s * (1 + e.scale), px, py)
			return true
		end,
		on_key = function(e): boolean?
			if not e.cmd then
				return nil
			end
			if e.key == "=" or e.key == "+" then
				commands.zoom_in()
			elseif e.key == "-" then
				commands.zoom_out()
			elseif e.key == "0" then
				commands.fit()
			elseif e.key == "1" then
				commands.actual()
			else
				return nil
			end
			return true
		end,
		measure,
		-- The checks, under the image's box (only the part in the frame).
		ui.canvas {
			absolute = true,
			left = 0,
			top = 0,
			w_full = true,
			h_full = true,
			pointer_events = "none",
			draw = function(c, fw, fh)
				local x, y, w, h = box()
				local x0, y0 = math.max(0, x), math.max(0, y)
				local x1, y1 = math.min(fw, x + w), math.min(fh, y + h)
				if x1 <= x0 or y1 <= y0 then
					return
				end
				local i0 = math.floor((x0 - x) / CHECK)
				local j0 = math.floor((y0 - y) / CHECK)
				local j = j0
				while y + j * CHECK < y1 do
					local i = i0
					if (i + j) % 2 == 1 then
						i += 1
					end
					local cy = y + j * CHECK
					while x + i * CHECK < x1 do
						local cx = x + i * CHECK
						local l, t = math.max(cx, x0), math.max(cy, y0)
						local r, b = math.min(cx + CHECK, x1), math.min(cy + CHECK, y1)
						if r > l and b > t then
							c:rect(l, t, r - l, b - t, "$zeron.files.checker")
						end
						i += 2
					end
					j += 1
				end
			end,
		},
		ui.image {
			src = doc.image or "",
			label = base_name(doc.path),
			absolute = true,
			-- (Its box is its own shape, zoomed; contained, never stretched.)
			fit = "contain",
			left = function(): number
				local x = box()
				return x
			end,
			top = function(): number
				local _, y = box()
				return y
			end,
			w = function(): number
				local _, _, w = box()
				return math.max(1, w)
			end,
			h = function(): number
				local _, _, _, h = box()
				return math.max(1, h)
			end,
		},
	}
end

local function component(props: Props): Node
	local doc_of = props.doc
	local function doc(): Doc?
		if type(doc_of) == "function" then
			return doc_of()
		end
		return (doc_of :: State<Doc?>):get()
	end
	local function doc_peek(): Doc?
		if type(doc_of) == "function" then
			return doc_of()
		end
		return (doc_of :: State<Doc?>):peek()
	end
	local find_open = ui.use_state(false)
	local wrap = ui.use_state(if props.wrap_default then props.wrap_default() else false)
	-- (The setting changing: wrapping follows it; the toggle's choice stays
	-- until then.)
	if props.wrap_default then
		local default = props.wrap_default
		local said = default()
		ui.effect(function()
			local now = default()
			if now ~= said then
				said = now
				wrap:set(now)
			end
		end)
	end
	local rendered: State<boolean> = ui.use_state(true)
	local zoom_label = ui.use_state("100%")
	local copied = ui.use_state(false)
	local copied_v = 0
	local image_commands: { [string]: () -> () } = {}
	-- What the editor shows; what was last read or saved; whether they
	-- differ; and whether the file moved on disk under them.
	local text = ui.use_state("")
	local base = ui.use_state("")
	local dirty = ui.use_state(false)
	local stale = ui.use_state(false)
	local readonly = props.readonly == true or props.on_save == nil
	local editor_commands: { [string]: () -> () } = {}
	-- Edits kept while another file is looked at: by path, unsaved.
	-- (The caller's, with `drafts`: kept when the viewer is made again, for
	-- another folder and back.)
	type Draft = { text: string, base: string }
	local own_drafts: { [string]: Draft } = {}
	local kept_drafts = props.drafts
	local function draft_of(path: string): Draft?
		if kept_drafts then
			return (kept_drafts:peek() :: any)[path]
		end
		return own_drafts[path]
	end
	local function keep_draft(path: string, d: Draft?)
		if kept_drafts then
			kept_drafts:set_key(path, d)
		else
			own_drafts[path] = d
		end
	end
	local opened: string? = nil
	local focus_ref = ui.ref()
	-- How wide the pane is, for what fits in the header.
	local bar = ui.ref()
	local function room(): number
		return bar.width or ROOM_ALL
	end

	-- What it is now: its kind, from its state and its name.
	local function kind(): string
		local d = doc()
		if not d then
			return "none"
		end
		if d.state ~= "ready" then
			return d.state
		end
		if d.image then
			return "image"
		end
		if is_markdown(d.path) then
			return "markdown"
		end
		return "code"
	end
	-- The editor's text follows the doc. Another file: what was typed here
	-- is kept, and that file's own comes back. The same file read again: it
	-- follows, unless there are edits to lose.
	ui.effect(function()
		local d = doc()
		local path = if d then d.path else nil
		local t = if d and d.state == "ready" then d.text or "" else ""
		if path ~= opened then
			if opened and dirty:peek() then
				keep_draft(opened, { text = text:peek(), base = base:peek() })
			end
			opened = path
			local draft = if path then draft_of(path) else nil
			if path then
				keep_draft(path, nil)
			end
			ui.batch(function()
				base:set(if draft then draft.base else t)
				text:set(if draft then draft.text else t)
				-- (Edits kept from before, the file since changed on disk:
				-- which to keep.)
				-- (Deleted on disk is its own question, not this one.)
				stale:set(draft ~= nil and d ~= nil and d.state == "ready" and not d.deleted and draft.base ~= t)
			end)
		elseif d and d.deleted then
			-- (Gone from disk: what's typed stays, all of it unsaved now.)
			if base:peek() ~= "" then
				base:set("")
			end
		elseif d and d.state == "ready" and t ~= base:peek() then
			if dirty:peek() or d.conflict then
				stale:set(true)
			else
				ui.batch(function()
					base:set(t)
					text:set(t)
				end)
			end
		end
	end)
	ui.effect(function()
		local on = dirty:get()
		if props.on_dirty and opened then
			props.on_dirty(opened, on)
		end
	end)
	-- Edits kept for when it's made again (the screen reloaded, another
	-- folder and back): a beat after typing pauses, and as it goes. So a
	-- reload never loses more than the last moment's, and usually nothing.
	if kept_drafts then
		local function stash()
			if not opened then
				return
			end
			-- (Asked which to keep, the edits are kept too: a save that found
			-- the file changed left them unsaved, if not "dirty".)
			if dirty:peek() or stale:peek() then
				keep_draft(opened, { text = text:peek(), base = base:peek() })
			elseif draft_of(opened) then
				keep_draft(opened, nil)
			end
		end
		ui.effect(function()
			-- (Its length, tracked without copying the text.)
			text:len()
			-- Clean again (saved, reloaded): its draft goes at once, so a
			-- stale one can't come back.
			if not dirty:get() and not stale:get() then
				if opened and draft_of(opened) then
					keep_draft(opened, nil)
				end
				return
			end
			-- (A timer this run owns: the next run, a key later, ends it and
			-- starts its own, so this is kept once typing pauses.)
			ui.after(300, stash)
		end)
		ui.on_cleanup(stash)
		-- A draft that arrives once the file is open (the viewer it was typed
		-- in, made again just now, kept it as it went): taken up.
		ui.effect(function()
			local all = (kept_drafts :: any):get()
			-- (And as the file comes in: a draft that came while it loaded.)
			local now = doc()
			local path = opened
			local d = if path then all[path] else nil
			if not d or dirty:peek() then
				return
			end
			if not now or now.path ~= path or now.state ~= "ready" then
				return
			end
			ui.batch(function()
				base:set(d.base)
				text:set(d.text)
				stale:set(not now.deleted and d.base ~= (now.text or ""))
			end)
		end)
	end
	local function disk(): string
		local d = doc_peek()
		return if d then d.text or "" else ""
	end

	local function copy_path()
		local d = doc_peek()
		if not d then
			return
		end
		ui.clipboard.write(d.absolute or d.path)
		copied_v += 1
		local v = copied_v
		copied:set(true)
		ui.after(1400, function()
			if copied_v == v then
				copied:set(false)
			end
		end)
	end

	-- The folders it is in: the project first, each a click away from the
	-- tree, a slash between them (and after the last, with `trail`, where a
	-- name follows on the same line). Too long on a line with the name, they
	-- give way first (`shrink`, against the name's 1) and fade out at their
	-- end before it; alone, they keep their end and fade at their start, so
	-- the folder it is in always reads.
	-- `size` is their type: the name's beside it, a step down under it.
	-- (`path`: the open file's, read as it changes: each folder's crumb is
	-- its own while the files opened share it.)
	type Crumb = { label: string, folder: string, last: boolean }
	local function folders(path: () -> string, size: string, trail: boolean?, shrink: number?): Node
		-- A crumb's height: its line, with a touch of wash round it.
		local tall = if size == "$font.size.md" then 20 else 16
		local crumbs = ui.computed(function(): { Crumb }
			local segs: { string } = {}
			for seg in string.gmatch(path(), "[^/]+") do
				table.insert(segs, seg)
			end
			local out: { Crumb } = {}
			if props.root then
				table.insert(out, { label = props.root, folder = "", last = #segs <= 1 })
			end
			local acc = ""
			for i = 1, #segs - 1 do
				acc = if acc == "" then segs[i] else acc .. "/" .. segs[i]
				table.insert(out, { label = segs[i], folder = acc, last = i == #segs - 1 })
			end
			return out
		end)
		local function crumb(c: Crumb, _i: number, now: State<Crumb>): Node
			return ui.row {
				shrink = 0,
				gap = 0,
				items = "center",
				ui.button {
					label = locale.t("ds.file_viewer.show_in_tree", { name = c.label }),
					h = tall,
					px = "$zeron.space.hair",
					py = 0,
					shrink = 0,
					rounded = "$radius.sm",
					bg = "#00000000",
					color = "$text.tertiary",
					hover = { bg = "$hover", color = "$text.secondary" },
					active = { bg = "$pressed", color = "$text.secondary" },
					on_click = function()
						if props.on_crumb then
							props.on_crumb(c.folder)
						end
					end,
					ui.text { c.label, literal = true, text_size = size },
				},
				-- A slash after it, but the last (unless a name follows).
				ui.show(function(): boolean
					return trail == true or not now:get().last
				end, ui.text { "/", shrink = 0, text_size = size, color = "$text.disabled" }),
			}
		end
		local row, clip = ui.ref(), ui.ref()
		local function over(): boolean
			return (row.width or 0) > (clip.width or 0) + 0.5
		end
		return ui.row {
			ref = clip,
			grow = if shrink then 0 else 1,
			shrink = shrink or 1,
			min_w = 0,
			h = tall,
			overflow = "hidden",
			items = "center",
			-- On a line with the name, the folders keep their start and fade
			-- out at their end, just before the name, which stays whole
			-- (zgx's own overflow test: it fades only while they run past).
			-- Alone, they keep their end and fade at their start.
			justify = if trail
				then "start"
				else function(): Justify
					return if over() then "end" else "start"
				end,
			fade_x = if trail
				then { 0, 24 }
				else function()
					return { if over() then 24 else 0, 0 }
				end,
			fade_overflow = if trail then true else nil,
			ui.row {
				ref = row,
				shrink = 0,
				gap = 0,
				items = "center",
				ui.each(crumbs, crumb, {
					key = function(c: Crumb): string
						return c.folder
					end,
					direction = "row",
					items = "center",
				}),
			},
		}
	end
	-- A dot after the name while it has unsaved changes.
	local function unsaved(): Node
		return ui.show(dirty, ui.div {
			label = locale.t("ds.file_viewer.unsaved"),
			w = 6,
			h = 6,
			shrink = 0,
			rounded = "$radius.full",
			bg = "$accent",
		})
	end
	-- Its git status: the tree's letter in the status's ink (the git pages'
	-- file rows), its name a tooltip; what git ignores, said quietly.
	local function status_mark(st: string): Node
		if st == "ignored" then
			return ui.text { locale.t("ds.file_viewer.status.ignored"), shrink = 0, text_size = "$font.size.xs", color = "$text.tertiary" }
		end
		return badge.FileStatus { status = st :: any }
	end

	-- What it is. `line`: its icon, the folders it is in, its name, the dot
	-- if it is unsaved, its status: one line, the folders giving way first.
	-- `stack`: the name over its place and status. `path`: its status letter
	-- and the whole path in the code's face, as a git page's file card.
	-- One header while a file is open, each part reading the file as it
	-- stands: another file opened keeps it (the crumbs it shares with the
	-- last stay put), a status a mark of its own kind.
	local function title(): Node
		local function path(): string
			local d = doc()
			return if d then d.path else ""
		end
		local function name(): string
			return base_name(path())
		end
		local function status(): string?
			local d = doc()
			return if d then d.status else nil
		end
		local function nested(): boolean
			return props.root ~= nil or string.find(path(), "/", 1, true) ~= nil
		end
		local head: Head = props.head or "line"
		return ui.presence(function(): string?
			return if doc() then "file" else nil
		end, function(): Node
			if head == "path" then
				return ui.row {
					w_full = true,
					min_w = 0,
					h_full = true,
					gap = "$space.2",
					items = "center",
					ui.presence(function(): string?
						local st = status()
						return if st and st ~= "ignored" then st else nil
					end, function(st: string): Node
						return badge.FileStatus { status = st :: any }
					end),
					FadeText {
						function(): string
							return if props.root then `{props.root}/{path()}` else path()
						end,
						min_w = 0,
						font = "$font.mono",
						text_size = "$font.size.md",
						color = "$text.primary",
					},
					unsaved(),
				}
			elseif head == "stack" then
				return ui.row {
					w_full = true,
					min_w = 0,
					h_full = true,
					gap = "$space.2",
					items = "center",
					ui.div { shrink = 0, FileIcon { name = name, size = 16 } },
					ui.column {
						grow = 1,
						min_w = 0,
						justify = "center",
						ui.row {
							w_full = true,
							min_w = 0,
							gap = "$space.2",
							items = "center",
							FadeText {
								name,
								min_w = 0,
								text_size = "$font.size.md",
								weight = "$font.weight.medium",
								color = "$text.primary",
							},
							unsaved(),
						},
						ui.show(
							function(): boolean
								return status() ~= nil or nested()
							end,
							ui.row {
								w_full = true,
								min_w = 0,
								gap = "$space.1",
								items = "center",
								ui.show(nested, folders(path, "$font.size.xs", false, 8)),
								ui.show(function(): boolean
									return status() ~= nil and nested()
								end, ui.text { "·", shrink = 0, text_size = "$font.size.xs", color = "$text.disabled" }),
								ui.presence(status, function(st: string): Node
									return ui.text {
										if SAID[st] then locale.t(SAID[st]) else st,
										shrink = 0,
										text_size = "$font.size.xs",
										color = if st == "ignored" then "$text.tertiary" else `$zeron.git.{st}`,
									}
								end),
							}
						),
					},
				}
			end
			-- The place and the name as one run, the name a crumb's step from
			-- the last slash. Short of room, the folders give way (they fade
			-- out at their end, before the name) and the name keeps its
			-- width; only a name too long for the whole line fades at its end.
			local run, words = ui.ref(), ui.ref()
			local function over(): boolean
				return (words.width or 0) > (run.width or 0) + 0.5
			end
			return ui.row {
				w_full = true,
				min_w = 0,
				h_full = true,
				gap = "$space.2",
				items = "center",
				ui.div { shrink = 0, FileIcon { name = name, size = 16 } },
				ui.row {
					ref = run,
					min_w = 0,
					shrink = 1,
					gap = 0,
					items = "center",
					overflow = "hidden",
					fade_x = function()
						return { 0, if over() then 24 else 0 }
					end,
					ui.show(nested, folders(path, "$font.size.md", true, 8)),
					ui.text {
						name,
						ref = words,
						shrink = 0,
						ml = function(): any
							return if nested() then "$zeron.space.hair" else 0
						end,
						text_size = "$font.size.md",
						weight = "$font.weight.medium",
						color = "$text.primary",
					},
				},
				unsaved(),
				ui.presence(status, status_mark),
			}
		end, { grow = 1, min_w = 0, h_full = true })
	end

	local function shows(kinds: { string }): () -> boolean
		return function(): boolean
			return table.find(kinds, kind()) ~= nil
		end
	end
	local function editing(): boolean
		local k = kind()
		return k == "code" or (k == "markdown" and not rendered:get())
	end
	local function zoom(name: string): () -> ()
		return function()
			if image_commands[name] then
				image_commands[name]()
			end
		end
	end
	local function on_path(run: ((string) -> ())?): (() -> ())?
		if not run then
			return nil
		end
		return function()
			local d = doc_peek()
			if d then
				run(d.absolute or d.path)
			end
		end
	end
	local reveal_file = on_path(props.on_reveal)
	local open_file = on_path(props.on_open_editor)
	local function openable(): boolean
		return props.openable == nil or props.openable()
	end
	local function and_openable(f: () -> boolean): () -> boolean
		return function(): boolean
			return openable() and f()
		end
	end
	local function save()
		if editor_commands.save then
			editor_commands.save()
		end
	end
	-- Autosave: a moment after the typing stops, the edits saved as ⌘S
	-- would. Only for typing seen here: a viewer made again with unsaved
	-- edits (a reload, a file come back to) waits for the next keystroke.
	if props.autosave and not readonly then
		local delay_of = props.autosave
		local function path_now(): string?
			local d = doc()
			return if d then d.path else nil
		end
		local typed, typed_in = text:peek(), path_now()
		local pending: (() -> ())? = nil
		ui.effect(function()
			local now = text:get()
			local here = path_now()
			local ms = delay_of()
			if pending then
				pending()
				pending = nil
			end
			-- (Another file opened is its text arriving, not typing.)
			if here ~= typed_in or now == typed then
				typed, typed_in = now, here
				return
			end
			typed = now
			if ms then
				pending = ui.after(ms, function()
					pending = nil
					if dirty:peek() and not stale:peek() then
						save()
					end
				end)
			end
		end)
	end
	local function zoom_controls(): Node
		return ui.row {
			gap = "$zeron.space.hair",
			items = "center",
			ui.show(function(): boolean
				return room() >= ROOM_SWITCH
			end, action("ds.file_viewer.zoom_out", "minus", zoom("zoom_out"), "cmd+-")),
			-- Its level, as an icon button's text: wider, the same states.
			IconButton {
				label = locale.t("ds.file_viewer.fit_or_actual"),
				tooltip = locale.s("ds.file_viewer.fit_or_actual_tip", { fit = ui.shortcut_text("cmd+0"), actual = ui.shortcut_text("cmd+1") }),
				size = "lg",
				w = 52,
				on_click = zoom("toggle"),
				ui.text { zoom_label, text_size = "$font.size.sm", tabular = true },
			},
			ui.show(function(): boolean
				return room() >= ROOM_SWITCH
			end, action("ds.file_viewer.zoom_in", "plus", zoom("zoom_in"), "cmd+=")),
		}
	end
	-- Preview and Source: their words where there is room, else one glyph
	-- for the way across.
	local function source_switch(): Node
		local either: IconButtonModule.Props = {
			label = locale.t("ds.file_viewer.preview_or_source"),
			tooltip = locale.s("ds.file_viewer.preview_or_source"),
			icon = function(): Icon.Name
				return if rendered:get() then "file-code" else "eye"
			end,
			size = "lg",
			tone = "quiet",
			on_click = function()
				rendered:toggle()
			end,
		}
		return ui.show(function(): boolean
			return room() >= ROOM_SWITCH
		end, Segmented {
			value = rendered,
			label = locale.t("ds.file_viewer.show") :: any,
			options = {
				{ value = true, label = locale.t("ds.file_viewer.preview") :: any },
				{ value = false, label = locale.t("ds.file_viewer.source") :: any },
			},
		}, IconButton(either))
	end

	-- The menu: what isn't out in the header at this width. Its rows are all
	-- made; each shows while it applies.
	local menu_open = ui.use_state(false)
	local menu_lit = ui.use_state(0)
	local menu_els: { any } = {}
	local menu_rows: { Node } = {}
	-- (`label`: its name's message key, read as it's drawn.)
	type Row = {
		label: string,
		icon: Icon.Name?,
		keys: string?,
		when: () -> boolean,
		on_click: (() -> ())?,
		checked: (() -> boolean)?,
	}
	local function row(r: Row)
		local i = #menu_rows + 1
		menu_rows[i] = ui.show(r.when, ui.menu_item(Menu.lights(Menu.look({
			label = locale.t(r.label),
			on_click = r.on_click,
			if r.icon then Menu.glyph(r.icon) else Menu.glyph("check", "#00000000"),
			FadeText { function(): string
				return locale.s(r.label)
			end, grow = 1, text_size = "$font.size.md", color = "$text.primary" },
			if r.checked
				then ui.show(r.checked, ui.div { color = "$text.secondary", shrink = 0, Icon { name = "check", size = 14 } })
				else nil,
			if r.keys then Kbd { keys = r.keys, look = "plain", size = "md", tone = "quiet" } else nil,
		}), i, menu_lit, menu_els)))
	end
	local function narrower(width: number, also: (() -> boolean)?): () -> boolean
		return function(): boolean
			return room() < width and (also == nil or also())
		end
	end
	local function has_file(): boolean
		return kind() ~= "none"
	end
	local function has_folds(): boolean
		return editing()
	end
	row {
		label = "ds.file_viewer.go_to_line",
		keys = "ctrl+g",
		when = editing,
		on_click = function()
			if editor_commands.go_to_line then
				editor_commands.go_to_line()
			end
		end,
	}
	row {
		label = "ds.file_viewer.wrap_lines",
		icon = "wrap-text",
		keys = "alt+z",
		when = narrower(ROOM_TOOLS, editing),
		checked = function(): boolean
			return wrap:get()
		end,
		on_click = function()
			wrap:toggle()
		end,
	}
	row {
		label = "ds.file_viewer.fold_all",
		keys = "shift+alt+cmd+[",
		when = has_folds,
		on_click = function()
			if editor_commands.fold_all then
				editor_commands.fold_all()
			end
		end,
	}
	row {
		label = "ds.file_viewer.unfold_all",
		keys = "shift+alt+cmd+]",
		when = has_folds,
		on_click = function()
			if editor_commands.unfold_all then
				editor_commands.unfold_all()
			end
		end,
	}
	row { label = "ds.file_viewer.zoom_in", icon = "plus", keys = "cmd+=", when = narrower(ROOM_SWITCH, shows({ "image" })), on_click = zoom("zoom_in") }
	row { label = "ds.file_viewer.zoom_out", icon = "minus", keys = "cmd+-", when = narrower(ROOM_SWITCH, shows({ "image" })), on_click = zoom("zoom_out") }
	row { label = "ds.file_viewer.fit", keys = "cmd+0", when = shows({ "image" }), on_click = zoom("fit") }
	row { label = "ds.file_viewer.actual_size", keys = "cmd+1", when = shows({ "image" }), on_click = zoom("actual") }
	row { label = "ds.file_viewer.copy_path", icon = "copy", keys = "alt+cmd+c", when = narrower(ROOM_ALL, has_file), on_click = copy_path }
	if reveal_file then
		row { label = "ds.file_viewer.reveal", icon = "folder", keys = "alt+cmd+r", when = and_openable(narrower(ROOM_ALL, has_file)), on_click = reveal_file }
	end
	if open_file then
		row { label = "ds.file_viewer.open_in_editor", icon = "arrow-up-right", when = and_openable(narrower(ROOM_ALL, has_file)), on_click = open_file }
	end
	-- What others add (plugins'), asked for each time the menu opens: kept
	-- as words in a state (its rows keyed by them), their handlers here.
	type Extra = { key: string, label: string?, heading: string? }
	local extras = ui.use_state({} :: { Extra })
	local extra_runs: { [string]: () -> () } = {}
	if props.more_menu then
		local more_menu = props.more_menu
		ui.effect(function()
			if not menu_open:get() then
				return
			end
			local d = doc_peek()
			local list: { Extra } = {}
			local runs: { [string]: () -> () } = {}
			if d then
				for i, e in more_menu(d.path, "file") do
					local key = `{i}\0{e.heading or e.label or ""}`
					if e.heading then
						table.insert(list, { key = key, heading = e.heading })
					elseif e.label then
						table.insert(list, { key = key, label = e.label })
						runs[key] = e.on_select
					end
				end
			end
			extra_runs = runs
			extras:set(list)
		end)
	end
	local base_rows = #menu_rows
	-- (As ds/menu lays a menu out: groups of rows, each inset evenly from
	-- the panel's edge, a rule between them edge to edge. Plugins' rows a
	-- group of their own, there only while there are any.)
	local function has_extras(): boolean
		return #extras:get() > 0
	end
	local own_group = Menu.Group(menu_rows :: any)
	local extras_list = ui.each(extras, function(item: Extra, i: number): Node
		if item.heading then
			return Menu.Heading { item.heading }
		end
		local label = item.label or ""
		return ui.menu_item(Menu.lights(Menu.look({
			label = label,
			on_click = function()
				local run = extra_runs[item.key]
				if run then
					run()
				end
			end,
			Menu.glyph("check", "#00000000"),
			FadeText { label, grow = 1, text_size = "$font.size.md", color = "$text.primary" },
		}), base_rows + i, menu_lit, menu_els))
	end, {
		-- (Not keyed: each row's place, `i`, is built in (the keys' light),
		-- so a row moved is a row built again. A few rows, at most.)
		w_full = true,
		gap = "$zeron.list.gap",
	})
	local menu_rows_and_more: { Node } = {
		own_group,
		ui.show(has_extras, Menu.Rule()),
		ui.show(has_extras, Menu.Group { extras_list }),
	}
	local more = IconButton {
		label = locale.t("common.more"),
		tooltip = locale.s("common.more"),
		icon = "more-horizontal",
		size = "lg",
		tone = "quiet",
		tooltip_side = "bottom",
		lit = function(): boolean
			return menu_open:get()
		end,
		on_click = function()
			menu_open:toggle()
		end,
	}

	-- Room kept at the header's start for a control laid over it (a pane's
	-- tree toggle): the header's own inset, or that if it is more.
	local function lead(): number
		local l = props.lead
		return if type(l) == "function" then l() else (l or 0)
	end
	local header = ui.row {
		id = "file-viewer-head",
		ref = bar,
		w_full = true,
		h = head_height(props.head),
		shrink = 0,
		pl = function(): any
			return ui.raw(math.max((ui.theme.get("space.3") :: number?) or 12, lead()), "past the room kept for the pane's control")
		end,
		pr = ui.raw((head_height(props.head) - 28) / 2, "a 28 control as far from the end as from the band's top and foot"),
		gap = "$space.2",
		items = "center",
		border_sides = { 0, 0, 1, 0 },
		border_color = "$border.subtle",
		title(),
		ui.row {
			hook = if props.hooks then props.hooks.actions else nil,
			shrink = 0,
			gap = "$zeron.space.hair",
			items = "center",
			ui.show(function(): boolean
				return dirty:get() and not readonly
			end, ui.div {
				mr = "$space.1",
				Button { locale.t("common.save"), tone = "secondary", size = "sm", on_click = save, tooltip = tip("common.save", "cmd+s") },
			}),
			-- The editor's: Find, at any width (the one thing done most
			-- here), and Wrap with room. Before Preview and Source, so going
			-- from one to the other (these come and go with Source) never
			-- moves the switch under the pointer.
			ui.show(editing, ui.row {
				gap = "$zeron.space.hair",
				mr = function(): any
					return if shows({ "markdown" })() then "$space.1" else 0
				end,
				toggle("ds.file_viewer.find", "search", find_open, "cmd+f"),
				ui.show(function(): boolean
					return room() >= ROOM_TOOLS
				end, toggle("ds.file_viewer.wrap_lines", "wrap-text", wrap, "alt+z")),
			}),
			ui.show(shows({ "markdown" }), source_switch()),
			ui.show(shows({ "image" }), zoom_controls()),
			ui.show(function(): boolean
				return has_file() and room() >= ROOM_ALL
			end, ui.row {
				gap = "$zeron.space.hair",
				items = "center",
				ui.div { w = 1, h = 16, mx = "$space.1", bg = "$border" },
				action(
					"ds.file_viewer.copy_path",
					function(): Icon.Name
						return if copied:get() then "check" else "copy"
					end,
					copy_path,
					"alt+cmd+c"
				),
				if reveal_file then ui.show(openable, action("ds.file_viewer.reveal", "folder", reveal_file, "alt+cmd+r")) else nil,
				if open_file then ui.show(openable, action("ds.file_viewer.open_in_editor", "arrow-up-right", open_file)) else nil,
			}),
			-- (The menu places its anchor, the ⋯ button.)
			ui.show(has_file, ui.menu(Menu.panel {
				open = menu_open,
				anchor = more,
				placement = { side = "bottom", align = "end", offset = 4 },
				w = 264,
				table.unpack(menu_rows_and_more),
			})),
		},
	}

	-- Its file deleted on disk under unsaved edits: save them (the file made
	-- again) or let them go.
	local deleted = ui.show(function(): boolean
		local d = doc()
		return d ~= nil and d.deleted == true
	end, banner.Banner {
		edge = true,
		tone = "warning",
		-- (A title alone, as the conflict's: a pane's width holds no more.)
		title = function(): string
			return locale.s("ds.file_viewer.deleted_on_disk")
		end,
		actions = {
			{
				label = locale.t("common.discard") :: any,
				on_click = function()
					local d = doc_peek()
					if not d then
						return
					end
					ui.batch(function()
						base:set(text:peek())
						stale:set(false)
					end)
					keep_draft(d.path, nil)
					if props.on_discard then
						props.on_discard(d.path)
					end
				end,
			},
			{
				label = locale.t("common.save") :: any,
				primary = true,
				on_click = function()
					if editor_commands.save then
						editor_commands.save()
					end
				end,
			},
		},
	})

	-- Its file moved on disk under unsaved edits: which to keep.
	local conflict = ui.show(stale, banner.Banner {
		edge = true,
		tone = "warning",
		title = function(): string
			return locale.s("ds.file_viewer.changed_on_disk")
		end,
		actions = {
			{
				label = locale.t("ds.file_viewer.keep_mine") :: any,
				on_click = function()
					base:set(disk())
					stale:set(false)
				end,
			},
			{
				label = locale.t("common.reload") :: any,
				primary = true,
				on_click = function()
					local t = disk()
					ui.batch(function()
						base:set(t)
						text:set(t)
						stale:set(false)
					end)
				end,
			},
		},
	})

	-- The code, in the editor.
	local function code(language: string?, path: string): Node
		return CodeEditor {
			value = text,
			saved = base,
			dirty = dirty,
			commands = editor_commands,
			language = language,
			readonly = readonly,
			wrap = wrap,
			find_open = find_open,
			status = true,
			blocks = props.blocks,
			block_view = props.block_view,
			on_add_block = props.on_add_block,
			on_block_moved = props.on_block_moved,
			on_save = function(t: string)
				if props.on_save then
					props.on_save(path, t)
				end
			end,
			place = if props.places
				then {
					get = function(): CodeEditor.Spot?
						return ((props.places :: any):peek() :: any)[path]
					end,
					set = function(spot: CodeEditor.Spot)
						(props.places :: any):set_key(path, spot)
					end,
				}
				else nil,
			label = base_name(path),
			text_size = if props.code_size then props.code_size() else nil,
			reveal = props.reveal_line,
			escape_hint = props.escape_hint,
			w_full = true,
			h_full = true,
		}
	end
	local body = ui.presence(function(): string
		local d = doc()
		-- (Built again for each file, as its kind changes, and at another
		-- code size.)
		local size = if props.code_size then props.code_size() else nil
		return if d then `{kind()}\0{d.path}\0{size or ""}` else "none"
	end, function(key: string): Node
		local d = doc_peek()
		local k = kind()
		if k == "none" or not d then
			return ui.column {
				w_full = true,
				h_full = true,
				items = "center",
				justify = "center",
				gap = "$space.3",
				ui.text { locale.t("ds.file_viewer.choose"), text_size = "$font.size.md", color = "$text.tertiary" },
			}
		end
		local doc_: Doc = d
		local name = base_name(doc_.path)
		if k == "loading" then
			return loading()
		elseif k == "binary" then
			return notice(
				"file-code",
				function(): string
					return locale.s("ds.file_viewer.binary.title", { name = name })
				end,
				function(): string
					return if doc_.size
						then locale.s("ds.file_viewer.binary.body_size", { size = bytes(doc_.size) })
						else locale.s("ds.file_viewer.binary.body")
				end,
				{
					if props.on_open_default and openable()
						then { label = locale.t("ds.file_viewer.binary.open_default"), primary = true, on_click = function()
							(props.on_open_default :: (string) -> ())(doc_.absolute or doc_.path)
						end }
						else nil :: any,
					if props.on_reveal and openable()
						then { label = locale.t("ds.file_viewer.reveal"), on_click = function()
							(props.on_reveal :: (string) -> ())(doc_.absolute or doc_.path)
						end }
						else nil :: any,
				}
			)
		elseif k == "large" then
			return notice(
				"info-circle",
				function(): string
					return locale.s("ds.file_viewer.large.title", { name = name })
				end,
				function(): string
					return locale.s("ds.file_viewer.large.body", { size = bytes(doc_.size), limit = bytes(props.limit or 8 * 1024 * 1024) })
				end,
				{
					if props.on_open_editor and openable()
						then { label = locale.t("ds.file_viewer.open_in_editor"), primary = true, on_click = function()
							(props.on_open_editor :: (string) -> ())(doc_.absolute or doc_.path)
						end }
						else nil :: any,
				}
			)
		elseif k == "gone" then
			return notice(
				"trash-bin-minimalistic",
				function(): string
					return locale.s("ds.file_viewer.gone.title", { name = name })
				end,
				function(): string
					return locale.s("ds.file_viewer.gone.body")
				end
			)
		elseif k == "error" then
			return notice(
				"danger-triangle",
				function(): string
					return locale.s("ds.file_viewer.error.title", { name = name })
				end,
				function(): string
					return doc_.error or locale.s("ds.file_viewer.error.body")
				end,
				{
					if props.on_retry
						then { label = locale.t("common.try_again"), on_click = function()
							(props.on_retry :: (string) -> ())(doc_.path)
						end }
						else nil :: any,
				}
			)
		elseif k == "image" then
			return image_view(doc_, zoom_label, image_commands)
		end
		local language = Syntax.language(doc_.path)
		if readonly and (doc_.text or "") == "" then
			return notice("document", function(): string
				return locale.s("ds.file_viewer.empty.title", { name = name })
			end, function(): string
				return locale.s("ds.file_viewer.empty.body")
			end)
		end
		if k == "markdown" then
			return ui.presence(rendered, function(on: boolean): Node
				if on then
					return SelectionMenu { w_full = true, h_full = true, ui.scroll {
						w_full = true,
						h_full = true,
						scrollbar = true,
						items = "center",
						ui.column {
							w_full = true,
							-- (A theme number: max_w takes points.)
							max_w = ui.theme.get("zeron.prose.measure") :: number?,
							px = "$space.6",
							py = "$space.5",
							Markdown {
								text = function(): string
									return text:get()
								end,
								image = if props.picture
									then function(src: string): string?
										return (props.picture :: any)(doc_.path, src)
									end
									else nil,
								open = if props.on_link
									then function(target: string)
										(props.on_link :: any)(doc_.path, target)
									end
									else nil,
								-- A task's box: the text's own `[ ]` ticked, an edit
								-- like any (saved as one is). Not in a file that
								-- can't be saved.
								task = if readonly
									then nil
									else function(k: number, done: boolean)
										text:set(tick_task(text:peek(), k, done))
									end,
							},
						},
					} }
				end
				return code("markdown", doc_.path)
			end, {
				w_full = true,
				h_full = true,
				enter = Motion.transition({ opacity = { 0, 1 }, duration = 140 }),
				exit = Motion.transition({ opacity = 0, duration = 80 }),
				mode = "wait",
			})
		end
		return code(language, doc_.path)
	end, {
		w_full = true,
		grow = 1,
		h = 0,
		enter = Motion.transition({ opacity = { 0, 1 }, duration = 120 }),
		exit = Motion.transition({ opacity = 0, duration = 60 }),
		mode = "wait",
	})

	-- The foot: what it is, quietly; and the keys that matter here.
	local foot = ui.row {
		w_full = true,
		h = FOOT,
		shrink = 0,
		px = "$space.3",
		gap = "$space.3",
		items = "center",
		border_sides = { 1, 0, 0, 0 },
		border_color = "$border.subtle",
		ui.text {
			function(): string
				local d = doc()
				local k = kind()
				if not d then
					return ""
				end
				local parts: { string } = {}
				if k == "image" then
					zoom_label:get()
					if d.width and d.height then
						table.insert(parts, `{d.width} × {d.height}`)
					end
					table.insert(parts, string.upper(ext(d.path)))
				elseif k == "code" or k == "markdown" then
					local lang = Syntax.name(Syntax.language(d.path))
					if lang ~= "" then
						table.insert(parts, lang)
					end
					local n = lines_in(d.text or "")
					table.insert(parts, locale.s("ds.file_viewer.lines", { n = n }))
				end
				if d.size then
					table.insert(parts, bytes(d.size))
				end
				return table.concat(parts, "  ·  ")
			end,
			literal = true,
			grow = 1,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
			tabular = true,
		},
		ui.show(function(): boolean
			return kind() == "image" and room() >= ROOM_SWITCH
		end, ui.text { locale.t("ds.file_viewer.toggle_fit"), text_size = "$font.size.xs", color = "$text.tertiary" }),
	}

	local box: { [any]: any } = {}
	for k, v in props do
		if k ~= "doc" and k ~= "root" and k ~= "limit" and k ~= "readonly" and k ~= "blocks" and k ~= "block_view" and k ~= "head" and k ~= "lead" and k ~= "hooks" and k ~= "drafts" and k ~= "places" and k ~= "openable" and k ~= "more_menu" and k ~= "wrap_default" and k ~= "autosave" and k ~= "code_size" and k ~= "picture" and k ~= "reveal_line" and k ~= "escape_hint" and string.sub(tostring(k), 1, 3) ~= "on_" and k ~= "children" then
			box[k] = v
		end
	end
	box.ref = focus_ref
	box[#box + 1] = header
	box[#box + 1] = deleted
	box[#box + 1] = conflict
	box[#box + 1] = body
	box[#box + 1] = ui.show(function(): boolean
		local k = kind()
		return k == "image" or (k == "markdown" and rendered:get())
	end, foot)
	return ui.column(box)
end

return ui.component(component)
