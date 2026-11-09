-- A reply's prose as zgx's markdown view draws it (`ui.markdown_view`):
-- the style, link handling and text that make its blocks what ds/markdown
-- draws, pixel for pixel (tests/prose_pixels.rs holds them to it). The
-- view parses and draws in Rust, a reply streaming in re-spanning its last
-- paragraph in place, so a chunk costs no script but what reads its text.
--
-- So far its paragraphs: their words, bold, italic, struck, inline code
-- (math too: `$x$` is read into code before the view parses, as
-- ds/markdown reads it), links and bare web addresses. Not yet: chips
-- (mentions, invocations, attachments), a find's marks, the streaming
-- veil; nor headings, lists, quotes or the complex kinds.
--
--     ui.markdown_view {
--         Prose.prepared(text),
--         style = Prose.style(),
--         on_link = Prose.on_link(open),
--         link_menu = Prose.link_menu(open, link_menu),
--         autolink = true,
--     }

local look = require("@self/look")
local Links = require("@ds/links")
local Source = require("@self/math_source")

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
