-- A reply's markdown, set for reading, from what Zeron's transcript draws
-- (crates/ui/src/markdown/render.rs) and a step past it.
--
-- `Markdown { text }`: `text` a string, or a function of the text as it
-- grows. With `key`, a reply streaming in parses only its new end
-- (`ui.markdown`'s `key`): its earlier blocks stay the very tables they
-- were, so only the block being written is built again, and a marker left
-- open (`**bold`, a link half there) shows closed until it closes, so the
-- text doesn't restyle and reflow as it arrives. `breaks`: single newlines
-- break lines (what a person typed).
--
-- The rhythm:
-- - Blocks sit a paragraph's gap apart (16, two thirds of a line).
-- - A heading has more room above it than below: it belongs to what it
--   heads. Three steps of size, all semibold, the largest a touch tight.
-- - List items sit closer than paragraphs; their markers hang in a gutter
--   of their own (a small dot, or the number in figures one width), so the
--   items' text lines up on its left edge, wrapped lines too.
-- - Inline code is a chip of the code wash in mono at the prose's x-height;
--   links are the text's own ink, underlined clear of descenders.
-- - A code block is a quiet panel: its language and Copy in a strip that
--   shares its fill (no rule under it), comfortable padding, lines that
--   never wrap (a long one scrolls sideways, fading at the edge it runs
--   past).
-- - A table sits in a hairline frame, its header quieter than its cells,
--   its columns as wide as their widest cell; wide, it scrolls sideways.
-- - A quote is set back by a soft bar, in the secondary ink.
--
-- The text is selectable through whatever holds it (a transcript puts
-- `selectable` on its list): this draws it, nothing more.

local control = require("@self/control")
local token = require("@self/token")
local Icon = require("@self/icon")
local Lightbox = require("@self/lightbox")
local ReplyPicture = require("@self/reply_picture")
local Checkbox = require("@self/checkbox")
local ToggleButton = require("@self/toggle_button")
local Syntax = require("@self/syntax")
-- (By the alias the shell uses too: a module is one instance per resolved
-- path, and the opener the shell sets must be the one markdown reads.)
local Links = require("@ds/links")
local FileIcon = require("@self/file_icons")
-- LaTeX math: `$$…$$` and `$…$` read before markdown is (ds/math).
local Math = require("@self/math")
local MarkdownFind = require("@self/markdown_find")
local locale = require("@self/locale")

-- A link's context menu entry (ds/links, ds/context_menu's).
export type LinkEntry = Links.Entry
export type Props = {
	text: string | () -> string,
	key: string?,
	breaks: boolean?,
	-- Told each top-level block's box (a paragraph, a list, a code block)
	-- as it is made, and given back what lets it go: for a reader keeping
	-- its place by the block it is on (a transcript as its width changes).
	track: ((block: Ref) -> () -> ())?,
	-- A document's own (a file's preview): what a picture's `src` shows
	-- from (a file `ui.image` loads), or nil while it can't (its alt text
	-- shows); a picture opens in the lightbox. Without it, pictures are
	-- their alt text (a reply's).
	image: ((src: string) -> string?)?,
	-- The size a picture is asked to show at, in points (an HTML `<img>`'s
	-- width and height, a 2x picture's half): never larger than that, the
	-- column still the limit. Without it, a picture's own size.
	image_size: ((src: string) -> (number?, number?))?,
	-- A link that isn't the web's (a path in the document's folder, `#a`):
	-- what opens it, or nil to leave it styled only.
	open: ((target: string) -> ())?,
	-- A link's context menu where it isn't the web's (a file, a chat): its
	-- entries (ds/links `file`, `chat`), asked as the menu opens; nil for
	-- the kind's own (a web link's: Open, the other place, Copy link; any
	-- other: Open with `open`, Copy link).
	link_menu: ((target: string) -> { Links.Entry }?)?,
	-- A task list's box ticked or cleared: the `k`th task in the text (in
	-- the order they're written, from 1), and whether it's done now. With
	-- it, tasks are boxes; without, their ☐ and ☑.
	task: ((k: number, done: boolean) -> ())?,
	-- Windowed (a long reply in a transcript): each top-level block built
	-- only while it's near the view, held at its last height (or a guess)
	-- while it's away, so a reply of hundreds of blocks costs what shows of
	-- it. `key` names the text (its blocks' heights are kept by it), `view`
	-- is the scroll it shows in, `offset` its scroll: a block above the
	-- view's top that comes in at another height than it was held at moves
	-- the scroll by the difference, so what shows stays put.
	-- (`live`: whether the text is being written, its last run then always
	-- built; `at_end`: whether the view rests at its end, which it then
	-- holds rather than a returning run's place.)
	window: { key: string, view: Ref, offset: State<number>, live: (() -> boolean)?, at_end: State<boolean>? }?,
	-- A reply's own pictures (screens/chat/reply_images): each drawn as
	-- Zeron's reply image row (ds/reply_picture), read as it is asked for;
	-- `group` names the reply, whose pictures open together in the
	-- lightbox (← →).
	pictures: ReplyPicture.Pictures?,
	group: string?,
	-- A find over the text (a chat's): its matches marked, the current one
	-- strongly, as the editor's find marks them. `key` names this text
	-- among the chat's; the current match is the `k`th in its top-level
	-- block (ds/markdown_find counts them the same way). The block holding
	-- it is built (if windowed) and handed to `reveal` once laid out.
	find: Find?,
	-- A step smaller (a thought's words in the work tree): the prose's small
	-- size and line, headings at it too, blocks a little closer.
	small: boolean?,
	-- Its words' ink, where not the primary (a thought's, in the secondary).
	ink: string?,
}
export type FindAt = { key: string, block: number, k: number }
export type Find = {
	key: string,
	query: () -> string,
	current: () -> FindAt?,
	reveal: ((block: Ref) -> ())?,
}

-- What a document's links and pictures resolve by (Props' `image`, `open`).
-- (`tasks`: each task item's place among them, from the latest parse.)
type Ctx = {
	image: ((src: string) -> string?)?,
	image_size: ((src: string) -> (number?, number?))?,
	open: ((target: string) -> ())?,
	link_menu: ((target: string) -> { Links.Entry }?)?,
	task: ((k: number, done: boolean) -> ())?,
	tasks: { [any]: number },
	pictures: ReplyPicture.Pictures?,
	group: string?,
	-- A find marking this block (`q` lower case): the current match's place
	-- in it (0: none), and how many it has counted so far.
	find: { q: string, cur: number, n: number }?,
	-- Set a step smaller (Props' `small`).
	small: boolean?,
}

-- A reply's pictures in the order it wrote them, by its group (each of its
-- blocks a Markdown of its own): the set its lightbox pages through.
local orders: { [string]: { ReplyPicture.Entry } } = {}

-- `b` (a picture) among its reply's, in the order written.
local function enlist(group: string, b: { src: string, alt: string })
	local order = orders[group] or {}
	orders[group] = order
	for _, e in order do
		if e.src == b.src then
			return
		end
	end
	table.insert(order, { src = b.src, alt = b.alt })
end

-- A text parsed as it's drawn: by ds/markdown_find's parse (its math read
-- first, pictures together one block), so a find counts the blocks drawn.
local function parse(text: string, opts: MarkdownOptions): { MarkdownBlock }
	return MarkdownFind.parse(text, opts)
end

-- What ui.markdown makes of a task's `[ ]` and `[x]`: its text's start.
local OPEN_BOX, DONE_BOX = "☐ ", "☑ "

-- Whether a list item is a task, and done: its first paragraph starts with
-- the parser's box.
local function task_of(item: { MarkdownBlock }): boolean?
	local first = item[1]
	if not first or first.t ~= "p" or not first.s[1] then
		return nil
	end
	local x = first.s[1].x
	if string.sub(x, 1, #OPEN_BOX) == OPEN_BOX then
		return false
	elseif string.sub(x, 1, #DONE_BOX) == DONE_BOX then
		return true
	end
	return nil
end

-- The item without its box: the paragraph's first run cut.
local function without_box(item: { MarkdownBlock }): { MarkdownBlock }
	local first = item[1] :: any
	local runs_ = table.clone(first.s)
	local head = table.clone(runs_[1])
	head.x = string.sub(head.x, #OPEN_BOX + 1)
	if head.x == "" then
		table.remove(runs_, 1)
	else
		runs_[1] = head
	end
	local p = table.clone(first)
	p.s = runs_
	local out = table.clone(item)
	out[1] = p
	return out
end

-- Each task item's place among the text's tasks, in the order written.
local function number_tasks(list: { MarkdownBlock }, into: { [any]: number })
	local n = 0
	local function walk(bs: { MarkdownBlock })
		for _, b in bs do
			if b.t == "ul" or b.t == "ol" then
				for _, item in b.items do
					if task_of(item) ~= nil then
						n += 1
						into[item] = n
					end
					walk(item)
				end
			elseif b.t == "quote" then
				walk(b.b)
			end
		end
	end
	walk(list)
end


local SIZE = "$zeron.prose.size"
local LINE = "$zeron.prose.line"
-- The small setting's (`small`).
local SMALL_SIZE = "$zeron.prose.small.size"
local SMALL_LINE = "$zeron.prose.small.line"

-- The text's size and line as the context sets them.
local function size_of(ctx: Ctx?): (string, string)
	if ctx and ctx.small then
		return SMALL_SIZE, SMALL_LINE
	end
	return SIZE, LINE
end

-- A web link opens; anything else (a path, a link still arriving) is only
-- styled as one.
local function opens(a: string): boolean
	return string.match(a, "^https?://") ~= nil or string.match(a, "^mailto:") ~= nil
end

-- A mention in a prompt (Zeron's `[name](zeron-file:path)`, a chat's
-- `zeron-chat:`): a chip in the line, as Zeron's sent bubble draws it
-- (transcript.rs `user_bubble_text`, composer/chip.rs `paint_chip`): its
-- name in the body's ink on the composer chip's wash, corner 5, its file's
-- icon (or a chat's) before it. Its click is the document's `open`.
local function mention_chip(span: { [any]: any }, target: string, name: string)
	local chat = string.match(target, "^zeron%-chat:") ~= nil
	local dir = string.sub(target, -1) == "/"
	local set = if string.find(ui.theme.kind(), "dark", 1, true) then "dark" else "light"
	-- (One pill, zgx d6582b32: icon, gap, name and padding on one wash,
	-- not underlined at rest; its text is the name alone, so a selection
	-- across it copies the name.)
	span.underline = false
	span.underline_offset = nil
	span.hover = nil
	span.bg = "$zeron.composer.chip.bg"
	span[1] = name
	span.padding = { 2, 2 }
	span.radius = ui.raw(5, "Zeron's inline chip corner (chip.rs)")
	span.icon = if chat
		then { src = "assets/icons/ui/chat-round-line.svg", size = 14, gap = 3, tint = "currentColor" }
		else { src = `assets/icons/files/{set}/{FileIcon.of(name, dir)}.svg`, size = 14, gap = 3 }
end

-- A web address written bare in a run's words (not in code, not a link
-- already): a link of its own, as GitHub's autolinks make one, keeping the
-- run's bold, italic and strike. Punctuation that ends the sentence stays
-- out of it, and a closing bracket too unless the address opened one.
local URL = "https?://[%w%-%._~:/%?#%[%]@!%$&'%(%)%*%+,;=%%]+"
local function trimmed(url: string): string
	while true do
		local last = string.sub(url, -1)
		if string.find(".,;:!?'\"", last, 1, true) then
			url = string.sub(url, 1, -2)
		elseif last == ")" and select(2, string.gsub(url, "%(", "")) < select(2, string.gsub(url, "%)", "")) then
			url = string.sub(url, 1, -2)
		else
			return url
		end
	end
end
local function autolinked(list: { MarkdownRun }): { MarkdownRun }
	local out: { MarkdownRun }? = nil
	for i, r in list do
		local pieces: { MarkdownRun }? = nil
		if not r.a and not r.c and not r.img and type(r.x) == "string" and string.find(r.x, "https?://") then
			pieces = {}
			-- (`at`: where the search goes on; `copied`: words before it
			-- already in a piece.)
			local at, copied = 1, 1
			while true do
				local s, e = string.find(r.x, URL, at)
				if not s then
					break
				end
				local url = trimmed(string.sub(r.x, s, e :: number))
				if #url > #"https://" then
					if s > copied then
						local before = table.clone(r)
						before.x = string.sub(r.x, copied, s - 1)
						table.insert(pieces :: any, before)
					end
					local link = table.clone(r)
					link.x = url
					link.a = url
					table.insert(pieces :: any, link)
					copied = s + #url
					at = copied
				else
					at = (e :: number) + 1
				end
			end
			if #(pieces :: any) == 0 then
				pieces = nil
			elseif copied <= #r.x then
				local rest = table.clone(r)
				rest.x = string.sub(r.x, copied)
				table.insert(pieces :: any, rest)
			end
		end
		if pieces and not out then
			out = table.move(list, 1, i - 1, 1, {})
		end
		if out then
			if pieces then
				table.move(pieces, 1, #pieces, #out + 1, out)
			else
				table.insert(out, r)
			end
		end
	end
	return out or list
end

-- A paragraph's runs as one paragraph of spans; `style` sets the
-- paragraph's own (a heading's size, a quote's ink).
local function runs(list: { MarkdownRun }, style: { [string]: any }?, ctx: Ctx?): Node
	list = autolinked(list)
	local size, line = size_of(ctx)
	local props: { [any]: any } = {
		wrap = true,
		text_size = size,
		line_height = line,
		color = "$text.primary",
	}
	if style then
		for k, v in style do
			props[k] = v
		end
	end
	local inline = ui.theme.get("zeron.prose.code.inline")
	-- A find's matches in the paragraph's words, numbered on from the
	-- block's count so far; each span cut where one starts or ends.
	local f = if ctx then ctx.find else nil
	local marks: { { number } } = if f then MarkdownFind.occurrences(MarkdownFind.words(list), f.q) else {}
	local base = if f then f.n else 0
	local cur = if f then f.cur else 0
	if f then
		f.n += #marks
	end
	local pos = 0
	local function emit(span: { [any]: any }, text: string)
		local from, to = pos + 1, pos + #text
		pos = to
		if #marks == 0 or span[1] ~= text then
			table.insert(props, ui.span(span))
			return
		end
		local at = from
		for i, m in marks do
			local s, e = math.max(m[1], from), math.min(m[2], to)
			if s <= e then
				if s > at then
					local before = table.clone(span)
					before[1] = string.sub(text, at - from + 1, s - from)
					table.insert(props, ui.span(before))
				end
				local hit = table.clone(span)
				hit[1] = string.sub(text, s - from + 1, e - from + 1)
				hit.bg = if base + i == cur then "$zeron.editor.match_current" else "$zeron.editor.match"
				table.insert(props, ui.span(hit))
				at = e + 1
			end
		end
		if at <= to then
			local rest = table.clone(span)
			rest[1] = string.sub(text, at - from + 1)
			table.insert(props, ui.span(rest))
		end
	end
	for _, run in list do
		-- A picture among the words, not drawn as one (a reply's that it can
		-- show are: `block`'s paragraph): the link it is, by its alt or its
		-- file's name, as Zeron's markdown keeps a picture the host won't
		-- draw. (One byte to a find: ds/markdown_find `words`.)
		local r = run
		if run.img then
			r = table.clone(run)
			r.x = if run.x ~= "" then run.x else (string.match(run.img, "([^/]+)$") or run.img)
			r.a = run.a or run.img
		end
		local span: { [any]: any } = { r.x, literal = true }
		if r.b then
			-- (A number: a span's `weight` takes no token.)
			span.weight = ui.raw(600, "semibold; a span's weight takes no token")
		end
		if r.i then
			span.italic = true
		end
		if r.s then
			span.strike = true
		end
		if r.c then
			span.font = "mono"
			span.bg = "$zeron.code.wash"
			span.padding = { 4, 1 }
			-- Zeron's inline code corner (markdown/render.rs's 4.5), each
			-- line's piece of a wrapped span rounded on its own.
			span.radius = ui.raw(4.5, "Zeron's inline code corner")
			span.text_size = inline
		end
		if r.a then
			span.underline = true
			span.underline_offset = 4
			span.hover = { underline = true, color = "$accent" }
			if string.match(r.a, "^zeron%-file:") or string.match(r.a, "^zeron%-chat:") then
				mention_chip(span, r.a, r.x)
			end
			local target = r.a
			local open = if ctx then ctx.open else nil
			local asked = if ctx then ctx.link_menu else nil
			if opens(target) then
				-- A link of its own (its cursor, role and menu); the app's
				-- opener takes its click where it takes links (ds/links: its
				-- browser tab), which keeps the href from opening outside.
				span.href = target
				span.on_click = function(): boolean
					return Links.take(target)
				end
			elseif open then
				span.on_click = function()
					open(target)
				end
			end
			-- Its menu, by its kind (ds/links), built as it opens.
			if opens(target) or open or asked then
				span.context_menu = Links.menu_for(target, function(): { Links.Entry }?
					local own = if asked then asked(target) else nil
					if own ~= nil or opens(target) then
						return own
					end
					return Links.other(target, if open
						then function()
							open(target)
						end
						else nil)
				end)
			end
		end
		if r.img then
			pos += #MarkdownFind.PICTURE
			table.insert(props, ui.span(span))
		else
			emit(span, r.x)
		end
	end
	return ui.rich(props)
end

-- A code block's lines, coloured by the design system's highlighter
-- (`ds/syntax`, as Files colours a file) when its fence names a language it
-- knows: one paragraph of runs, a line a break, so it lays out and selects
-- as one text and copies as the code.
-- Code blocks fitted to their width, long lines wrapping, rather than
-- scrolling sideways: one choice for them all, kept (the shipping app's
-- `code_fences_fit_content`, render.rs's Fit content).
local fit = ui.persist("markdown_code_fit", false)

-- (`find`: a find's matches to mark in it, numbered on from `from`, the
-- current one `cur`.)
type CodeFind = { q: string, from: number, cur: number }
local function code_text(text: string, lang: string, wrap: boolean?, find: CodeFind?): Node
	local props: { [any]: any } = {
		wrap = wrap,
		font = "$font.mono",
		text_size = "$zeron.prose.code.size",
		line_height = "$zeron.prose.code.line",
		color = "$text.primary",
		literal = true,
		pl = "$space.4",
		pr = "$space.5",
		pb = "$space.3",
	}
	local marks = if find then MarkdownFind.occurrences(text, find.q) else {}
	local base = if find then find.from else 0
	local cur = if find then find.cur else 0
	if Syntax.highlighter(lang) == nil and #marks == 0 then
		props[1] = text
		return ui.text(props)
	end
	-- Each piece of the text, cut where a match starts or ends.
	local pos = 0
	local function piece(t: string, color: string?)
		local from, to = pos + 1, pos + #t
		pos = to
		local at = from
		for i, m in marks do
			local s, e = math.max(m[1], from), math.min(m[2], to)
			if s <= e then
				if s > at then
					table.insert(props, ui.span { string.sub(t, at - from + 1, s - from), color = color, literal = true })
				end
				table.insert(props, ui.span {
					string.sub(t, s - from + 1, e - from + 1),
					color = color,
					literal = true,
					bg = if base + i == cur then "$zeron.editor.match_current" else "$zeron.editor.match",
				})
				at = e + 1
			end
		end
		if at <= to then
			table.insert(props, ui.span { string.sub(t, at - from + 1), color = color, literal = true })
		end
	end
	if Syntax.highlighter(lang) == nil then
		piece(text)
		return ui.rich(props)
	end
	for i, line in Syntax.lines(text, lang) do
		if i > 1 then
			piece("\n")
		end
		for _, run in line do
			piece(run.text, run.color)
		end
	end
	return ui.rich(props)
end

-- A code block: a strip with its language and Copy over its lines, one
-- quiet panel.
-- (`class` and `provide`: what plugins place it by, and read where they
-- do; `block` gives them.)
local CodeBlock = ui.component(function(props: { lang: string, text: string, class: string?, provide: { [string]: any }?, find: CodeFind? }): Node
	local copied = ui.use_state(false)
	local stop: (() -> ())? = nil
	-- Without its trailing newline (a paste into a terminal shouldn't run
	-- it), tabs as four spaces (a tab's eight read as a gap).
	local text = string.gsub(string.gsub(props.text, "\n$", ""), "\t", "    ")
	local function copy()
		ui.clipboard.write(text)
		copied:set(true)
		if stop then
			stop()
		end
		stop = ui.after(1400, function()
			copied:set(false)
		end)
	end
	return ui.column {
		class = props.class,
		provide = props.provide,
		w_full = true,
		min_w = 0,
		rounded = "$radius.xl",
		bg = "$zeron.prose.code.bg",
		border = 1,
		border_color = "$zeron.prose.code.edge",
		overflow = "hidden",
		ui.row {
			h = 34,
			pl = "$space.4",
			-- Copy as far from the end as from the top and foot.
			pr = ui.raw(5, "even with Copy's room in the 34 strip"),
			items = "center",
			gap = "$space.1",
			selectable = false,
			-- (Fit and Copy one Tab stop, ← → between them: a reply of many
			-- blocks isn't two stops each.)
			focus_group = "horizontal",
			ui.text {
				-- The language by its name ("Rust", "TypeScript"), or the
				-- fence's own tag for one the highlighter doesn't know.
				if Syntax.highlighter(props.lang) then Syntax.name(props.lang) elseif props.lang == "" then "text" else props.lang,
				literal = true,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
				grow = 1,
				min_w = 0,
			},
			ToggleButton {
				on = fit,
				label = locale.t("ds.markdown.fit_content") :: any,
				icon = "wrap-text",
				size = "sm",
				tooltip = locale.s("ds.markdown.fit_content"),
				rounded = ui.raw(control.nest("$radius.xl", 1, 5), "concentric with the code block, as Copy"),
			},
			ui.button {
				label = locale.t("ds.markdown.copy_code"),
				h = 24,
				px = "$space.2",
				gap = "$space.1",
				items = "center",
				rounded = ui.raw(control.nest("$radius.xl", 1, 5), "concentric with the code block: its corner less its edge and inset"),
				bg = "#00000000",
				color = "$text.tertiary",
				text_size = "$font.size.xs",
				hover = { bg = "$hover", color = "$text.primary" },
				active = { bg = "$pressed" },
				focus = { border = 0 },
				on_click = copy,
				Icon {
					name = function()
						return if copied:get() then "check" else "copy"
					end,
					size = 12,
				},
				ui.text(function()
					return if copied:get() then locale.s("ds.markdown.copied") else locale.s("common.copy")
				end),
			},
		},
		ui.presence(function(): boolean
			return fit:get()
		end, function(fitted: boolean): Node
			if fitted then
				return ui.column { w_full = true, min_w = 0, code_text(text, props.lang, true, props.find) }
			end
			return ui.scroll_x {
				w_full = true,
				fade_x = { 16, 32 },
				fade_overflow = true,
				code_text(text, props.lang, nil, props.find),
			}
		end, { w_full = true, min_w = 0 }),
	}
end)

-- A document's picture: its own size, never wider than the column (its
-- height following), and a click looks at it whole in the lightbox. Its
-- size is its file's header's (`ui.image_size`); a picture from the web
-- says it as it loads (`on_load`, a point-sized stand-in), and zgx asks
-- for one that failed again by itself.
local Picture = ui.component(function(props: { src: string, name: string, w: number?, h: number? }): Node
	local ok, iw, ih = pcall(ui.image_size, props.src)
	local header = ok and type(iw) == "number" and type(ih) == "number" and iw > 0 and ih > 0
	local natural = ui.use_state(if header then { w = iw :: number, h = ih :: number } else { w = 0, h = 0 })
	local function known(): boolean
		local n = natural:get()
		return n.w > 0 and n.h > 0
	end
	return ui.div {
		w_full = true,
		relative = true,
		overflow = "hidden",
		ui.show(
			function(): boolean
				return not known()
			end,
			ui.image {
				src = props.src,
				aria_hidden = true,
				absolute = true,
				left = 0,
				top = 0,
				w = 1,
				h = 1,
				opacity = 0,
				pointer_events = "none",
				on_load = function(size: { w: number, h: number })
					if size.w > 0 and size.h > 0 then
						natural:set({ w = size.w, h = size.h })
					end
				end,
			}
		),
		ui.show(
			known,
			ui.div {
				w_full = true,
				max_w = function(): number
					local n = natural:get()
					-- (The width it was asked to show at; or the height's, at
					-- its own proportions.)
					if props.w then
						return props.w
					elseif props.h and n.h > 0 then
						return props.h * n.w / n.h
					end
					return n.w
				end,
				aspect = function(): number
					local n = natural:get()
					return if n.h > 0 then n.w / n.h else 1
				end,
				cursor = "zoom_in",
				-- (A button by the keys too, Enter or Space, and to
				-- assistive technology: what it does, the picture's name in
				-- it. The lightbox gives the keyboard back here.)
				role = "button",
				label = locale.t("ds.markdown.view_named", { name = props.name }),
				focusable = true,
				on_click = function()
					local n = natural:peek()
					Lightbox.open({ { src = props.src, name = props.name, w = n.w, h = n.h } }, 1, `markdown:{props.src}`)
				end,
				on_key = function(e: KeyEvent): boolean?
					if (e.key == "Enter" or e.key == " " or e.key == "Space") and not (e.cmd or e.ctrl or e.alt) then
						local n = natural:peek()
						Lightbox.open({ { src = props.src, name = props.name, w = n.w, h = n.h } }, 1, `markdown:{props.src}`)
						return true
					end
					return nil
				end,
				ui.image { src = props.src, aria_hidden = true, w_full = true, h_full = true, fit = "contain" },
			}
		),
	}
end)

local blocks: (list: { MarkdownBlock }, ink: string?, ctx: Ctx?) -> { Node }

-- A list item's marker, hanging in its gutter, on the first line's middle.
local function marker(ordered: boolean, n: number, ctx: Ctx?): Node
	local size, line = size_of(ctx)
	if ordered then
		return ui.text {
			`{n}.`,
			aria_hidden = true,
			w = 24,
			shrink = 0,
			tabular = true,
			text_size = size,
			line_height = line,
			color = "$text.tertiary",
		}
	end
	return ui.row {
		aria_hidden = true,
		w = 24,
		h = line,
		shrink = 0,
		items = "center",
		pl = "$space.1",
		ui.div { w = 5, h = 5, rounded = "$radius.full", bg = "$text.tertiary" },
	}
end

-- A block ui.markdown hasn't parsed yet (`lazy`: a long text that landed
-- whole is parsed over the next frames, its end first): how tall it will
-- likely be, by what its first line says it is (`hint`), its source lines
-- and its bytes.
local function pending_height(b: any): number
	local lines = math.max(1, b.lines or 1)
	local hint = b.hint
	if hint == "code" then
		return math.max(1, lines - 2) * 19 + 56
	elseif hint == "h" then
		return 34
	elseif hint == "hr" then
		return 24
	elseif hint == "table" then
		return math.max(1, lines - 1) * 36
	elseif hint == "p" then
		local bytes = math.max(1, (b.to or 0) - (b.from or 0) + 1)
		return math.max(lines, math.ceil(bytes / 85)) * 22
	end
	return lines * 22
end

-- One block, by its kind; `ink`, its text's colour where not the primary (a
-- quote's).
local function block(b: MarkdownBlock, ink: string?, ctx: Ctx?): Node
	if (b :: any).t == "pending" then
		-- (Its place, held until it's parsed: a frame or two.)
		return ui.div { w_full = true, h = pending_height(b) }
	end
	if b.t == "p" then
		local style = if ink then { color = ink } else nil
		-- Pictures among a reply's words that it can show: drawn, the words
		-- around them lines of their own above and below, as Zeron's
		-- markdown draws a picture its host takes (render.rs
		-- `text_element`); those with only spaces between on one tray. One
		-- it won't show stays its link (`runs`).
		local pictures = if ctx then ctx.pictures else nil
		if pictures then
			local pieces: { Node } = {}
			local words: { MarkdownRun } = {}
			local shown: { any } = {}
			local function worded(): boolean
				for _, w in words do
					if w.img or string.find(w.x, "%S") then
						return true
					end
				end
				return false
			end
			local function show()
				if #shown == 1 then
					table.insert(pieces, block(shown[1], ink, ctx))
				elseif #shown > 1 then
					table.insert(pieces, block({ t = "pics", list = shown } :: any, ink, ctx))
				end
				shown = {}
			end
			for _, r in b.s do
				if r.img and pictures.resolve(r.img) ~= nil then
					if worded() then
						show()
						table.insert(pieces, runs(words, style, ctx))
					end
					words = {}
					table.insert(shown, { t = "img", src = r.img, alt = r.x })
				else
					table.insert(words, r)
				end
			end
			if #pieces > 0 or #shown > 0 then
				show()
				if worded() then
					table.insert(pieces, runs(words, style, ctx))
				end
				return ui.column {
					w_full = true,
					min_w = 0,
					gap = if ctx and ctx.small then "$space.3" else "$space.4",
					pieces,
				}
			end
		end
		return runs(b.s, style, ctx)
	elseif b.t == "h" then
		local level = math.min(b.l, 3)
		local key = `zeron.prose.h{level}`
		-- (Set small, a heading keeps the prose's size: a thought's heading
		-- is a title in the margin, not a chapter's.)
		local small = ctx ~= nil and ctx.small == true
		return runs(b.s, {
			heading = b.l,
			weight = "$font.weight.semibold",
			text_size = if small then SMALL_SIZE else `${key}.size`,
			line_height = if small then SMALL_LINE else `${key}.line`,
			letter_spacing = if level == 1 then -0.3 else nil,
			color = ink,
			-- More room above than the blocks keep, less below: a heading
			-- belongs to what follows it.
			pt = if level < 3 then "$space.3" else "$space.1",
			mb = ui.raw(-6, "a heading sits closer to what it heads"),
			class = "md-heading",
		}, ctx)
	elseif b.t == "code" and b.lang == "math" then
		-- Display math (`$$…$$`, or a ```math fence): the formula, else its
		-- source as the code block it is.
		-- (What it gives is above it: a plugin replacing `.md-math` reads
		-- `tex` there, where the node it replaces stood; as a token, read
		-- through what the plugin may read, ds/token.)
		return ui.div {
			provide = { tex = token.block(b.text) },
			w_full = true,
			min_w = 0,
			Math.Display(b.text, function(): Node
				return CodeBlock { lang = b.lang, text = b.text, class = "md-math" }
			end),
		}
	elseif b.t == "code" then
		-- Plugins place by `.md-code` and `.md-code.lang-<language>` (a
		-- renderer of its own: `.md-code.lang-mermaid`), reading `code`.
		-- (What it gives is above it, so a plugin replacing the block reads
		-- it where the block stood.)
		local lang = string.gsub(string.lower(b.lang or ""), "[^%w_-]", "-")
		-- (A find's matches in its lines, numbered on from the block's.)
		local f = if ctx then ctx.find else nil
		local find: CodeFind? = nil
		if f then
			find = { q = f.q, from = f.n, cur = f.cur }
			f.n += #MarkdownFind.occurrences(b.text, f.q)
		end
		return ui.div {
			provide = { code = token.block({ lang = b.lang or "", source = b.text }) },
			w_full = true,
			min_w = 0,
			CodeBlock {
				lang = b.lang,
				text = b.text,
				find = find,
				class = if lang ~= "" then `md-code lang-{lang}` else "md-code",
			},
		}
	elseif b.t == "ul" or b.t == "ol" then
		local items: { Node } = {}
		local ordered = b.t == "ol"
		local start = if b.t == "ol" then b.start else 1
		for i, item in b.items do
			-- A task (a document's, where its box can be ticked): the box in
			-- the marker's gutter.
			local done = if ctx and ctx.task then task_of(item) else nil
			local k = if done ~= nil and ctx then ctx.tasks[item] else nil
			local lead = marker(ordered, start + i - 1, ctx)
			local body = item
			if done ~= nil and k and ctx and ctx.task then
				local tick, is_done = ctx.task, done
				body = without_box(item)
				lead = ui.row {
					w = 24,
					h = LINE,
					shrink = 0,
					items = "center",
					Checkbox {
						value = function(): boolean
							return is_done
						end,
						bare = true,
						name = locale.t("ds.markdown.done") :: any,
						on_change = function(on: boolean)
							tick(k, on)
						end,
					},
				}
			end
			table.insert(
				items,
				ui.row {
					role = "listitem",
					w_full = true,
					items = "start",
					lead,
					ui.column { grow = 1, min_w = 0, gap = "$space.2", blocks(body, ink, ctx) },
				}
			)
		end
		return ui.column { role = "list", w_full = true, gap = "$zeron.space.tight", items }
	elseif b.t == "quote" then
		return ui.row {
			class = "md-quote",
			w_full = true,
			gap = "$space.4",
			ui.div { w = 2, shrink = 0, rounded = "$radius.full", bg = "$border.strong" },
			ui.column { grow = 1, min_w = 0, gap = "$space.3", blocks(b.b, "$text.secondary", ctx) },
		}
	elseif b.t == "hr" then
		return ui.div { w_full = true, h = 1, my = "$space.2", bg = "$border" }
	elseif b.t == "table" then
		-- Column by column, so each is as wide as its widest cell; cells keep
		-- to one line (one height a row), and the last column takes what's
		-- left of the frame.
		local box = ui.ref()
		local cell = {
			wrap = false,
			h = 36,
			px = "$space.4",
			text_size = "$zeron.prose.small.size",
			line_height = 36,
		}
		local cols: { Node } = {}
		for c, head in b.head do
			local head_style = table.clone(cell) :: { [string]: any }
			head_style.weight = "$font.weight.medium"
			head_style.color = "$text.secondary"
			head_style.border_sides = { 0, 0, 1, 0 }
			head_style.border_color = "$border"
			local cells: { Node } = { runs(head, head_style, ctx) }
			for r, row in b.rows do
				local style = table.clone(cell) :: { [string]: any }
				style.border_sides = { 0, 0, if r < #b.rows then 1 else 0, 0 }
				style.border_color = "$border.subtle"
				table.insert(cells, runs(row[c] or {}, style, ctx))
			end
			table.insert(cols, ui.column { shrink = 0, grow = if c == #b.head then 1 else 0, cells })
		end
		return ui.scroll_x {
			class = "md-table",
			ref = box,
			w_full = true,
			rounded = "$radius.xl",
			border = 1,
			border_color = "$border",
			fade_x = { 16, 32 },
			fade_overflow = true,
			ui.row {
				min_w = function(): number
					return math.max(0, (box.width or 0) - 2)
				end,
				cols,
			},
		}
	elseif (b.t == "img" or b.t == "pics") and ctx and ctx.pictures then
		-- A reply's pictures (ds/reply_picture): one alone, or several that
		-- follow one another together on a tray. One it won't draw (a URL, a
		-- home path, a GIF) is the link the reply wrote, as Zeron keeps it.
		local pictures = ctx.pictures :: ReplyPicture.Pictures
		local group = ctx.group or "reply"
		local function set(): { ReplyPicture.Entry }
			return orders[group] or {}
		end
		local members: { any } = if b.t == "pics" then (b :: any).list else { b }
		local out: { Node } = {}
		local run: { ReplyPicture.Entry } = {}
		local function flush()
			if #run == 1 then
				table.insert(out, ReplyPicture.Single { pictures = pictures, src = run[1].src, alt = run[1].alt, group = group, set = set, open_link = ctx.open, link_menu = ctx.link_menu })
			elseif #run > 1 then
				local these = run
				table.insert(out, ReplyPicture.Run {
					pictures = pictures,
					items = function(): { ReplyPicture.Entry }
						return these
					end,
					group = group,
					set = set,
					open_link = ctx.open,
					link_menu = ctx.link_menu,
				})
			end
			run = {}
		end
		for _, it in members do
			if pictures.resolve(it.src) ~= nil then
				enlist(group, it)
				table.insert(run, { src = it.src, alt = it.alt })
			else
				flush()
				local words = if it.alt ~= "" then it.alt else (string.match(it.src, "([^/]+)$") or it.src)
				table.insert(out, ui.div { class = "md-image", min_w = 0, runs({ { x = words, a = it.src } :: any }, nil, ctx) })
			end
		end
		flush()
		if #out == 1 then
			return out[1]
		end
		return ui.column { w_full = true, min_w = 0, gap = "$space.3", out }
	elseif b.t == "pics" then
		-- (Pictures together where a text has none of a reply's: each as a
		-- document's, one under another.)
		local out: { Node } = {}
		for _, it in (b :: any).list do
			table.insert(out, block(it, ink, ctx))
		end
		return ui.column { w_full = true, min_w = 0, gap = "$space.3", out }
	elseif b.t == "img" then
		local alt = ui.text { b.alt, literal = true, italic = true, color = "$text.tertiary", text_size = SIZE, class = "md-image" }
		if not (ctx and ctx.image) then
			return alt
		end
		-- A document's picture: at its own size, never past the column; a
		-- click looks at it whole.
		local resolve, wanted = ctx.image, b.src
		return ui.div {
			class = "md-image",
			min_w = 0,
			ui.presence(function(): string
			return resolve(wanted) or ""
		end, function(src: string): Node
			if src == "" then
				return alt
			end
			local name = if b.alt ~= "" then b.alt else (string.match(wanted, "([^/]+)$") or wanted)
			local w, h = nil :: number?, nil :: number?
			if ctx.image_size then
				w, h = ctx.image_size(wanted)
			end
			return Picture { src = src, name = name, w = w, h = h }
		end),
		}
	end
	return ui.div {}
end

function blocks(list: { MarkdownBlock }, ink: string?, ctx: Ctx?): { Node }
	local out: { Node } = {}
	for _, b in list do
		table.insert(out, block(b, ink, ctx))
	end
	return out
end

-- A windowed text's blocks' heights, as last laid out, by the text's key
-- and the block's place in it: a reply scrolled back holds each block at
-- the height it had, not a guess.
-- (Kept in keyed state, a key at a time: a reload keeps them, so a view
-- made again lays out exactly as it was.)
local HEIGHTS = ui.state("markdown_window_heights", {} :: { [string]: number })
-- Whether each run was built (near the view) when last judged: a reload
-- builds the same runs, so nothing moves.
local NEAR = ui.state("markdown_window_near", {} :: { [string]: boolean })
-- How far past the view a windowed block is built ahead of coming in.
local OVERSCAN = 600
-- How many blocks a windowed run holds (a long reply away from the view
-- is a box a run, not a box a block).
local CHUNK = 16

-- How tall a block will likely be, before it has been laid out: its words
-- at a reply's width (about 85 characters a line), a code block by its
-- lines, a list by its items.
local function estimate(b: any): number
	local function chars(runs: any): number
		local n = 0
		for _, r in runs or {} do
			n += #(r.x or "")
		end
		return n
	end
	local t = b.t
	if t == "pending" then
		return pending_height(b)
	elseif t == "p" then
		return math.max(1, math.ceil(chars(b.s) / 85)) * 22
	elseif t == "h" then
		return 34
	elseif t == "code" then
		local lines = 1
		for _ in string.gmatch(b.text or "", "\n") do
			lines += 1
		end
		return lines * 19 + 56
	elseif t == "ul" or t == "ol" then
		local h = 0
		for _, item in b.items or {} do
			for _, inner in item do
				h += estimate(inner)
			end
		end
		return math.max(h, 22)
	elseif t == "quote" then
		local h = 0
		for _, inner in b.b or {} do
			h += estimate(inner)
		end
		return math.max(h, 22)
	elseif t == "table" then
		return (#(b.rows or {}) + 1) * 36
	elseif t == "img" then
		-- (A picture held at its placeholder's height: the column at 16:10.)
		return 412
	elseif t == "pics" then
		return 240 * math.ceil(#((b :: any).list or {}) / 3)
	end
	return 24
end

return function(props: Props): Node
	local text = props.text
	local opts: MarkdownOptions = { key = props.key, breaks = props.breaks }
	local ctx: Ctx? = if props.image or props.open or props.link_menu or props.task or props.pictures or props.small
		then { image = props.image, image_size = props.image_size, open = props.open, link_menu = props.link_menu, task = props.task, tasks = {}, pictures = props.pictures, group = props.group, small = props.small }
		else nil
	-- Its words' ink, where the caller sets it (a thought's secondary).
	local ink = props.ink
	local column: { [any]: any } = { w_full = true, min_w = 0, gap = if props.small then "$space.3" else "$space.4" }
	-- A block, in a box its tracker is told of (when there is one).
	local track = props.track
	local function boxed(node: Node): Node
		if not track then
			return node
		end
		local r = ui.ref()
		ui.on_cleanup(track(r))
		return ui.div { ref = r, w_full = true, min_w = 0, node }
	end
	-- A top-level block, the `n`th: with a find, made again as its matches
	-- change (the query, or the current match coming or going), its marks
	-- numbered from its first part; holding the current match, handed to
	-- `reveal` once laid out.
	local find = props.find
	-- A top-level block, the `n`th, with a find's marks (`q`: the query, ""
	-- for none; `k`: the current match's place in it, 0 if it's elsewhere).
	-- Holding the current match, it's handed to `reveal` once laid out.
	local function marked(b: MarkdownBlock, n: number, q: string?, k: number): Node
		if not find or not q or q == "" then
			return boxed(block(b, ink,ctx))
		end
		local mine: Ctx = if ctx then table.clone(ctx) else { tasks = {} }
		mine.find = { q = string.lower(q), cur = k, n = 0 }
		local node = block(b, ink,mine)
		local reveal = (find :: Find).reveal
		if k == 0 or not reveal then
			return boxed(node)
		end
		local r = ui.ref()
		ui.after(0, function()
			reveal(r)
		end)
		return boxed(ui.div {
			ref = r,
			w_full = true,
			min_w = 0,
			mb = if b.t == "h" then ui.raw(-6, "a heading sits closer to what it heads") else nil,
			node,
		})
	end
	-- A windowed run's find, as its key: "" without one, else
	-- `|<block>:<k>|<query>` (the current match's block and place, if it's in
	-- this run, else 0:0).
	local function run_find(at: number): string
		if not find then
			return ""
		end
		local f = find :: Find
		local q = f.query()
		if q == "" then
			return ""
		end
		local c = f.current()
		if c and c.key == f.key and (c.block - 1) // CHUNK + 1 == at then
			return `|{c.block}:{c.k}|{q}`
		end
		return `|0:0|{q}`
	end
	local function top(b: MarkdownBlock, n: number): Node
		if not find then
			return boxed(block(b, ink,ctx))
		end
		local f = find :: Find
		local r = ui.ref()
		return boxed(ui.div {
			ref = r,
			w_full = true,
			min_w = 0,
			-- (A heading's pull toward what it heads, which its own margin
			-- gives it only as the column's child.)
			mb = if b.t == "h" then ui.raw(-6, "a heading sits closer to what it heads") else nil,
			ui.presence(function(): string
				local q = f.query()
				if q == "" then
					return "-"
				end
				local c = f.current()
				local k = if c and c.key == f.key and c.block == n then c.k else 0
				return `{k}:{q}`
			end, function(sig: string): Node
				local k, q = string.match(sig, "^(%d+):(.*)$")
				local mine: Ctx = if ctx then table.clone(ctx) else { tasks = {} }
				mine.find = if q and q ~= "" then { q = string.lower(q), cur = tonumber(k) or 0, n = 0 } else nil
				if mine.find and mine.find.cur > 0 and f.reveal then
					local reveal = f.reveal
					ui.after(0, function()
						reveal(r)
					end)
				end
				return block(b, ink,mine)
			end, { w_full = true, min_w = 0 }),
		})
	end
	-- Windowed (`window`): the blocks in runs of CHUNK, each run built while
	-- its reach (the run and OVERSCAN either side) touches the view; away, a
	-- box of its last height. Inside a run each block is drawn once and kept
	-- while its table is the same, so the one being written rebuilds alone.
	local window = props.window
	-- Each run's judge, by its place (registered as it's made): a scroll
	-- runs them in order, once, rather than a timer for every run. Each
	-- says whether its box starts past the view's foot (and the overscan),
	-- and whether it's built now. (A run past the foot that isn't built
	-- needs no judging until the view comes near.)
	local judges: { [number]: () -> (boolean, boolean) } = {}
	-- Its runs judged a tick on (once laid out), all at once: as runs mount
	-- and resize, one timer for the text, not one a run.
	local looking = false
	local function look_soon()
		if looking then
			return
		end
		looking = true
		ui.after(0, function()
			looking = false
			for _, j in judges do
				j()
			end
		end)
	end
	local function windowed(run: State<{ MarkdownBlock }>, at: number, after: () -> number): Node
		local w = window :: {
			key: string,
			view: Ref,
			offset: State<number>,
			live: (() -> boolean)?,
			at_end: State<boolean>?,
		}
		local k = `{w.key}:{at}`
		-- Built as it was last judged. One never judged is built if it's
		-- among the text's last runs (where a transcript opens: its end),
		-- else held at a guess and built a tick on if it's near. (Built all
		-- at first, a long reply cost its whole height once, and the memory
		-- it took stayed.)
		local function last(): boolean
			return after() == 0
		end
		local near = ui.use_state(if NEAR:peek()[k] ~= nil then NEAR:peek()[k] else after() < 1)
		local function guess(): number
			local h = 0
			for _, b in run:peek() do
				h += estimate(b) + 16
			end
			return h
		end
		local held = ui.use_state(HEIGHTS:peek()[k] or guess())
		local r = ui.ref()
		-- Built again after being held: its first layout may come in at
		-- another height than it was held at (the width changed meanwhile).
		-- Only then does the scroll make up the difference; a rewrap of a
		-- run already built is the reading place's to keep (ds/transcript).
		local returning = false
		-- Near the view: its box within OVERSCAN of the view's, judged as the
		-- view scrolls (a tick on, once the scroll has laid out) and as it's
		-- laid out. (zgx's visibility event looks again only when layout
		-- changes, not as a scroll alone moves what shows.)
		local function judge(): (boolean, boolean)
			local el, view = r:peek(), w.view:peek()
			if not el or not view or el.y == nil or view.y == nil or el.height == nil or view.height == nil then
				return false, near:peek()
			end
			local y, top = el.y :: number, view.y :: number
			-- (While the text is written its last run is always built: where
			-- the words arrive, and the end the view holds to.)
			local live = w.live
			-- (And the run holding a find's current match, so it can show.)
			local c = if find then find.current() else nil
			local holds = c ~= nil and find ~= nil and c.key == w.key and (c.block - 1) // CHUNK + 1 == at
			local shows = (live ~= nil and live() and last())
				or holds
				or (y < top + (view.height :: number) + OVERSCAN and y + (el.height :: number) > top - OVERSCAN)
			if NEAR:peek()[k] ~= shows then
				NEAR:set_key(k, shows)
			end
			if near:peek() ~= shows then
				returning = shows
				near:set(shows)
			end
			return y >= top + (view.height :: number) + OVERSCAN, shows
		end
		judges[at] = judge
		local look = look_soon
		return ui.div {
			ref = r,
			w_full = true,
			min_w = 0,
			on_mount = look,
			on_resize = function(size: { w: number, h: number })
				look()
				if not near:peek() then
					return
				end
				local was = held:peek()
				local back = returning
				returning = false
				local kept = HEIGHTS:peek()[k]
				if kept == nil or math.abs(size.h - kept) >= 0.5 then
					HEIGHTS:set_key(k, size.h)
				end
				if math.abs(size.h - was) < 0.5 then
					return
				end
				held:set(size.h)
				if not back or (w.at_end and w.at_end:peek()) then
					return
				end
				-- Above the view's top (it came in from there): the view moves
				-- with it, so what shows doesn't.
				local el, view = r:peek(), w.view:peek()
				if el and view and el.y and view.y and (el.y :: number) < (view.y :: number) then
					w.offset:set(math.max(0, w.offset:peek() + size.h - was))
				end
			end,
			-- (Built, it's made again as a find's marks in it change: the
			-- query, or the current match coming into it or leaving.)
			ui.presence(function(): string
				return if near:get() then "built" .. run_find(at) else "held"
			end, function(kind: string): Node
				if string.sub(kind, 1, 5) == "built" then
					local cb, ck, q = string.match(kind, "^built|(%d+):(%d+)|(.*)$")
					local cur_block, cur_k = tonumber(cb) or 0, tonumber(ck) or 0
					return ui.column {
						w_full = true,
						min_w = 0,
						gap = "$space.4",
						ui.each(ui.computed(function(): { MarkdownBlock }
							return run:get()
						end), function(b: MarkdownBlock, i: number)
							local n = (at - 1) * CHUNK + i
							return marked(b, n, q, if n == cur_block then cur_k else 0)
						end),
					}
				end
				return ui.div {
					w_full = true,
					h = function(): number
						return held:get()
					end,
				}
			end),
		}
	end

	-- A windowed text (a reply in a transcript) is parsed lazily: one that
	-- lands whole and long comes back at once, its blocks not parsed yet
	-- held in place (`pending`) and parsed over the next frames, its end
	-- first, so no frame parses all of it. (By a key, which lazy parsing
	-- needs: the caller's, else the window's.) A find's current match in
	-- it has its block parsed first.
	if window and type(text) == "function" then
		opts.key = opts.key or `md:{window.key}`
		opts.lazy = true
	end
	if type(text) == "function" then
		-- A text that changes: its blocks as a state, each drawn once and kept
		-- while its table is the same (all but the one being written).
		local parsed = ui.computed(function(): { MarkdownBlock }
			if window and find then
				local c = find.current()
				opts.first = if c and c.key == window.key then c.block else nil
			end
			local list = parse(text(), opts)
			if ctx and ctx.task then
				ctx.tasks = {}
				number_tasks(list, ctx.tasks)
			end
			return list
		end)
		if window then
			-- The blocks in runs of CHUNK, each run a row of its own.
			local runs = ui.computed(function(): { { at: number, blocks: { MarkdownBlock } } }
				local out = {}
				for n, b in parsed:get() do
					local at = (n - 1) // CHUNK + 1
					if not out[at] then
						out[at] = { at = at, blocks = {} }
					end
					table.insert(out[at].blocks, b)
				end
				return out
			end)
			-- As the view scrolls (a tick on, once the scroll has laid out): the
			-- runs judged from the top, stopping at the first past the view's
			-- foot once none further on is built.
			local w = window :: { offset: State<number> }
			-- All the runs judged, from the top, stopping at the first past the
			-- view's foot once none further on is built.
			local judged_at = -math.huge
			local function judge_all()
				judged_at = ui.now()
				local count = #runs:peek()
				-- (A find's current match further on: its run is judged,
				-- so it's built, however far past the view it is.)
				local c = if find then find.current() else nil
				local target = if c and c.key == (window :: any).key then (c.block - 1) // CHUNK + 1 else 0
				for at = 1, count do
					local j = judges[at]
					if j then
						local below = j()
						if below and at >= target then
							local more = false
							for later = at + 1, count do
								if NEAR:peek()[`{(window :: any).key}:{later}`] ~= false then
									more = true
									break
								end
							end
							if not more then
								break
							end
						end
					end
				end
			end
			ui.effect(function()
				w.offset:get()
				-- (A find stepping to another match: its run may need building.)
				if find then
					find.current()
				end
				-- While the view moves every frame (a wheel's fling, its momentum),
				-- each move runs this again before the tick below comes, and that
				-- tick goes with it: judged only once the view stopped, the runs
				-- coming in stayed held and the transcript showed blank through a
				-- fling. So a pass now as well, a frame apart at most (the boxes as
				-- last laid out, a frame behind: well within OVERSCAN), and the tick
				-- for where the view comes to rest.
				if ui.now() - judged_at >= 8 then
					judge_all()
				end
				ui.after(0, judge_all)
			end)
			column[1] = ui.each(runs, function(run: any, _i: any, row: State<any>)
				return windowed(ui.computed(function(): { MarkdownBlock }
					return row:get().blocks
				end) :: any, run.at, function(): number
					return #runs:peek() - run.at
				end)
			end, {
				key = function(run: any): string
					return tostring(run.at)
				end,
			})
		else
			column[1] = ui.each(parsed, function(b: MarkdownBlock, i: number)
				return top(b, i)
			end)
		end
	elseif track then
		local out: { Node } = {}
		local list = parse(text, opts)
		if ctx and ctx.task then
			number_tasks(list, ctx.tasks)
		end
		for i, b in list do
			table.insert(out, top(b, i))
		end
		column[1] = out
	else
		local list = parse(text, opts)
		if ctx and ctx.task then
			number_tasks(list, ctx.tasks)
		end
		if find then
			local out: { Node } = {}
			for i, b in list do
				table.insert(out, top(b, i))
			end
			column[1] = out
		else
			column[1] = blocks(list, ink, ctx)
		end
	end
	return ui.column(column)
end
