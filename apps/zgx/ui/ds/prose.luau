-- A reply's prose as zgx's markdown view draws it (`ui.markdown_view`):
-- the style, link handling and text that make its blocks what ds/markdown
-- draws, pixel for pixel (tests/prose_pixels.rs holds them to it). The
-- view parses and draws in Rust, a reply streaming in re-spanning its last
-- paragraph in place, so a chunk costs no script but what reads its text.
--
-- So far its paragraphs: their words, bold, italic, struck, inline code
-- (math too: `$x$` is read into code before the view parses, as
-- ds/markdown reads it), links and bare web addresses, chips (mentions,
-- invocations, attachments, picks), and the streaming veil. Not yet: a
-- find's marks; nor headings, lists, quotes or the complex kinds.
--
--     ui.markdown_view {
--         Prose.prepared(text),
--         style = Prose.style(),
--         on_link = Prose.on_link(open),
--         link_menu = Prose.link_menu(open, link_menu),
--         autolink = true,
--     }

local look = require("@self/look")
local FileIcon = require("@self/file_icons")
local Links = require("@ds/links")
local Source = require("@self/math_source")
-- (Loaded for its block hook: ds/markdown sets it as it loads.)
require("@self/markdown")
local MarkdownBlocks = require("@self/markdown_blocks")

local M = {}

export type StyleOptions = {
	-- A step smaller (a thought's words): the prose's small size and line,
	-- blocks a little closer (ds/markdown's `small`).
	small: boolean?,
	-- Its words' ink, where not the primary (ds/markdown's `ink`).
	ink: string?,
}

-- The view's style: each kind of run as ds/markdown's `add_runs` makes its
-- span, a paragraph as its `paragraph_props`, the blocks its column's gap.
-- Tokens resolve through the window's theme as ds/markdown's do; inline
-- code's and links' ink follows "Accent colour throughout" (ds/look).
function M.style(opts: StyleOptions?): { [string]: any }
	local small = opts ~= nil and opts.small == true
	local ink = if opts then opts.ink else nil
	local size = if small then "$zeron.prose.small.size" else "$zeron.prose.size"
	local line = if small then "$zeron.prose.small.line" else "$zeron.prose.line"
	-- A heading of a level (ds/markdown's): semibold, three steps of size
	-- (set small, the prose's), the largest a touch tight; more room above
	-- than the blocks keep, less below: it belongs to what follows it.
	local function heading(level: number): { [string]: any }
		local key = `zeron.prose.h{level}`
		return {
			weight = "$font.weight.semibold",
			text_size = if small then size else `${key}.size`,
			line_height = if small then line else `${key}.line`,
			letter_spacing = if level == 1 then -0.3 else nil,
			color = ink,
			pt = if level < 3 then "$space.3" else "$space.1",
			mb = ui.raw(-6, "a heading sits closer to what it heads"),
		}
	end
	return {
		gap = if small then "$space.3" else "$space.4",
		paragraph = {
			wrap = true,
			text_size = if small then "$zeron.prose.small.size" else "$zeron.prose.size",
			line_height = if small then "$zeron.prose.small.line" else "$zeron.prose.line",
			color = ink or "$text.primary",
		},
		-- (A number: a span's `weight` takes no token.)
		strong = { weight = ui.raw(600, "semibold; a span's weight takes no token") },
		em = { italic = true },
		strike = { strike = true },
		code = {
			-- In code's font (Settings → Appearance), as code blocks are.
			font = look.code_family,
			bg = "$zeron.prose.code_wash",
			color = look.accented_ink(nil),
			padding = { 4, 1 },
			-- Zeron's inline code corner (markdown/render.rs's 4.5), each
			-- line's piece of a wrapped span rounded on its own.
			radius = ui.raw(4.5, "Zeron's inline code corner"),
			text_size = "$zeron.prose.code.inline",
		},
		h1 = heading(1),
		h2 = heading(2),
		h3 = heading(3),
		-- A list: items closer than paragraphs, their markers hanging in a
		-- gutter of their own (a small dot, or the number in figures one
		-- width), so the items' text lines up on its left edge.
		list = { w_full = true, gap = "$zeron.space.tight" },
		item = { w_full = true, items = "start" },
		item_body = { grow = 1, min_w = 0, gap = "$space.2" },
		number = {
			w = 24,
			shrink = 0,
			tabular = true,
			text_size = size,
			line_height = line,
			color = "$zeron.prose.marker",
		},
		bullet = { w = 24, h = line, shrink = 0, items = "center", pl = "$space.1" },
		bullet_dot = { w = 5, h = 5, rounded = "$radius.full", bg = "$zeron.prose.marker" },
		-- A quote: set back by a soft bar, in the secondary ink.
		quote = { w_full = true, gap = "$space.4" },
		quote_rail = { w = 2, shrink = 0, rounded = "$radius.full", bg = "$zeron.prose.quote_rail" },
		quote_body = { grow = 1, min_w = 0, gap = "$space.3" },
		quote_ink = "$text.secondary",
		rule = { w_full = true, h = 1, my = "$space.2", bg = "$border" },
		-- A heading's pull toward what it heads, where it sits in a box of
		-- its own (a find's), which its own margin gives it only as the
		-- column's child.
		pull = { mb = ui.raw(-6, "a heading sits closer to what it heads") },
		-- A find's matches, as the editor's find marks them (ds/markdown's).
		mark = { bg = "$zeron.editor.match" },
		mark_current = { bg = "$zeron.editor.match_current" },
		link = {
			color = look.accented_ink(nil),
			underline = true,
			underline_offset = 4,
			hover = { underline = true, color = "$accent" },
		},
	}
end

-- A web link opens; anything else (a path, a link still arriving) is only
-- styled as one (ds/markdown's `opens`).
local function opens(a: string): boolean
	return string.match(a, "^https?://") ~= nil or string.match(a, "^mailto:") ~= nil
end

-- A link's click, as ds/markdown's: a web link the app's opener takes
-- (true: its address stays shut), any other the document's `open`.
function M.on_link(open: ((target: string) -> ())?): (target: string, e: ClickEvent) -> boolean?
	return function(target: string, _e: ClickEvent): boolean?
		if opens(target) then
			return Links.take(target)
		end
		if open then
			open(target)
			return true
		end
		return false
	end
end

-- A link's context menu, as ds/markdown's: its kind's (ds/links), the
-- page's own entries first where it has them (`asked`: a file's, a chat's).
function M.link_menu(
	open: ((target: string) -> ())?,
	asked: ((target: string) -> { Links.Entry }?)?
): (target: string, e: PointerEvent) -> { Node }?
	return function(target: string, e: PointerEvent): { Node }?
		if not (opens(target) or open or asked) then
			return nil
		end
		return Links.menu_for(target, function(): { Links.Entry }?
			local own = if asked then asked(target) else nil
			if own ~= nil or opens(target) then
				return own
			end
			local opener = open
			return Links.other(target, if opener
				then function()
					opener(target)
				end
				else nil)
		end)(e)
	end
end

-- A chip's look, as Zeron's shipping app paints it (composer/chip.rs
-- `paint_chip`; ds/markdown's): its pill hugs its name, 2 points above and
-- below, corner 5; its icon (14) on a well of its own 2 points round it,
-- corner 4 (`zeron.chip.*`), a point in from the pill's start and 2 before
-- the name; 7 points after the name.
local CHIP_PADDING = { 1, 7, 2 }
local CHIP_RADIUS = ui.raw(5, "Zeron's inline chip corner (chip.rs)")
local function chip_icon(icon: { [string]: any }): { [string]: any }
	icon.size = 14
	icon.gap = 2
	icon.bg = "$zeron.chip.well"
	icon.radius = 4
	icon.pad = 2
	return icon
end
-- A chip's pill, its name and icon: in the body's ink, not underlined, with
-- a hover of its own that changes nothing (without one, a link's pointer
-- draws the focus ring round its words).
local function pill(name: string, icon: { [string]: any }, link: boolean?): { [any]: any }
	return {
		name,
		underline = false,
		hover = {},
		bg = "$zeron.chip.bg",
		padding = CHIP_PADDING,
		radius = CHIP_RADIUS,
		icon = chip_icon(icon),
		link = link,
	}
end
local function scheme(): string
	return if string.find(ui.theme.kind(), "dark", 1, true) then "dark" else "light"
end
-- A mention (`[name](zeron-file:path)`, a chat's `zeron-chat:`, a link to
-- one of the chat's files): its file's icon (a chat's glyph), its name; it
-- opens as a link does.
local function mention(target: string, name: string): { [any]: any }
	local chat = string.match(target, "^zeron%-chat:") ~= nil
	local dir = string.sub(target, -1) == "/"
	local file = string.match(string.match(target, "^[^#]*") or target, "([^/]+)/?$") or name
	file = string.match(file, "^(.-):%d") or file
	if chat then
		return pill(name, { src = "assets/icons/ui/side-chat.svg", tint = "$zeron.chip.glyph" }, true)
	end
	local of = if string.find(file, ".", 1, true) or dir then file else name
	return pill(name, { src = `assets/icons/files/{scheme()}/{FileIcon.of(of, dir)}.svg` }, true)
end
-- A command or skill picked (`zeron-invoke:`): its glyph, its name (a
-- skill's without its `$`).
local function invocation(name: string): { [any]: any }
	local glyph = if string.sub(name, 1, 1) == "/" then "command" else "magic-stick"
	if string.sub(name, 1, 1) == "$" then
		name = string.sub(name, 2)
	end
	return pill(name, { src = `assets/icons/ui/{glyph}.svg`, tint = "$zeron.chip.glyph" })
end
-- An item the message carries, named: its kind's icon (a file's by its
-- name; a chat's, a picture's, a quote's glyph), its name.
local function attachment(name: string, item: { kind: string, icon: string? }): { [any]: any }
	local glyph = if item.kind == "chat" then "side-chat" elseif item.kind == "image" then "gallery" else item.icon
	if glyph then
		return pill(name, { src = `assets/icons/ui/{glyph}.svg`, tint = "$zeron.chip.glyph" })
	end
	return pill(name, { src = `assets/icons/files/{scheme()}/{FileIcon.of(name, false)}.svg` })
end

export type ChipOptions = {
	-- What the page says a link is: one of the chat's files, or one of its
	-- agent's skills (ds/markdown's `link_chip`). Read as the runs are made:
	-- a chip known later makes them again.
	link_chip: ((target: string) -> { file: boolean?, skill: string? }?)?,
	-- What the message carries, by name (ds/markdown's `attached`).
	attached: { [string]: { kind: string, icon: string? } }?,
}

-- The view's `chip`: what ds/markdown draws as a chip, drawn as it does —
-- an item carried, named; a command or skill picked; a picture or file by
-- its number; a browser pick; a link to a skill; a mention (a file's, a
-- chat's). Nil for words and links.
function M.chip(opts: ChipOptions?): (target: string, text: string) -> { [any]: any }?
	local link_chip = if opts then opts.link_chip else nil
	local attached = if opts then opts.attached else nil
	return function(target: string, text: string): { [any]: any }?
		if string.sub(target, 1, 11) == "attachment:" then
			local name = string.sub(target, 12)
			local item = if attached then attached[name] else nil
			return if item then attachment(name, item) else nil
		end
		local chip = if link_chip and not opens(target) then link_chip(target) else nil
		if string.match(target, "^zeron%-invoke:") then
			return invocation(text)
		elseif string.match(target, "^zeron%-image:%d+$") or string.match(target, "^zeron%-attachment:%d+$") then
			return attachment(text, { kind = if string.match(target, "^zeron%-image:") then "image" else "file" })
		elseif string.match(target, "^zeron%-annotation:") then
			return attachment(text, { kind = "file", icon = "cursor" })
		elseif chip and chip.skill then
			return invocation("$" .. chip.skill)
		elseif string.match(target, "^zeron%-file:") or string.match(target, "^zeron%-chat:") or (chip and chip.file) then
			return mention(target, text)
		end
		return nil
	end
end

export type FindAt = { key: string, block: number, k: number }
export type Find = {
	key: string,
	query: () -> string,
	current: () -> FindAt?,
	reveal: ((block: Ref) -> ())?,
}

-- The view's `find`, from ds/markdown's (a chat's find over this text, by
-- its key): the current match where it is in this text, and the block
-- holding it handed to `reveal` as ds/markdown hands it (a ref to it).
function M.find(f: Find?): { [string]: any }?
	if not f then
		return nil
	end
	local reveal = f.reveal
	return {
		query = f.query,
		current = function(): (number?, number?)
			local c = f.current()
			if c and c.key == f.key then
				return c.block, c.k
			end
			return nil, nil
		end,
		reveal = if reveal
			then function(block: any)
				reveal({
					peek = function()
						return block
					end,
					get = function()
						return block
					end,
				} :: any)
			end
			else nil,
	}
end

-- The view's `block`: what it doesn't draw itself (code, display math,
-- mermaid, tables, pictures) drawn by ds/markdown, exactly as it draws
-- them, from ds/markdown's props for the text (its veil, find, pictures,
-- links…).
function M.block(props: { [string]: any }): (b: MarkdownBlock, at: any) -> Node
	return MarkdownBlocks.hook(props)
end

-- The names `attached` carries, for the view's `attached`.
function M.attached(attached: { [string]: any }?): { [string]: boolean }?
	if not attached then
		return nil
	end
	local names: { [string]: boolean } = {}
	for name in attached do
		names[name] = true
	end
	return names
end

-- The text the view parses: `text`'s with its math read into code
-- (ds/math_source, as ds/markdown_find reads it before parsing), followed as
-- it grows: a reply streaming in is read on from where the last read could
-- go on, not from its start.
function M.prepared(text: () -> string): () -> string
	local scan = Source.scan()
	return function(): string
		Source.feed(scan, text())
		return Source.since(scan, 0)
	end
end

return M
