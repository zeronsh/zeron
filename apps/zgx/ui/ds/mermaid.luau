-- A ```mermaid fence drawn as its diagram, as Zeron's transcript draws it
-- (crates/ui markdown/render.rs `render_code_block`, transcript.rs
-- `diagram_media`): the host draws it (src/mermaid.rs, `mermaid.render`,
-- the shipping app's renderer in its style) in the theme's colours, off the
-- window's thread; until it has, and while the fence is still being
-- written, the fence shows as the code it is. One that can't be drawn stays
-- code, with why under it.
--
-- Drawn, it sits in the code block's own frame: its own size, never wider
-- than the column (scaled down to it, never up), Show source / Show
-- diagram and Copy in the strip, and a click looks at it whole in the
-- lightbox, on the fence's fill.
--
--   Mermaid.Diagram { text = b.text, lang = b.lang, code = function() return CodeBlock { … } end,
--     writing = function() return Mermaid.writing(source(), b.text) end }

local control = require("@self/control")
local Icon = require("@self/icon")
local IconButton = require("@self/icon_button").IconButton
local Lightbox = require("@self/lightbox")
local locale = require("@self/locale")

local M = {}

-- What the host drew: the diagram as SVG on a clear ground, the same on
-- the fence's fill (the lightbox's), both drawn by `ui.svg` at the size and
-- pixel scale they show at, so sharp at any zoom; the fill's as a PNG file
-- (the lightbox's size, Copy and Reveal); and the diagram's own size in
-- points.
export type Drawn = { svg: string, plate_svg: string, plate: string, w: number, h: number }
type Result = Drawn | { error: string }

-- The host's drawing, a promise of a `Drawn` (a test's host has none and
-- puts its own here).
M.render = function(arg: { [string]: any }): any
	return ui.call("mermaid.render", arg)
end

-- Whether a fence holding `code` is still being written at the end of
-- `source` (a reply streaming in): its last line is the code's last, or
-- the fence's opening line, and no fence has closed it. Only the text's
-- end is read, so asking as each word arrives costs nothing.
function M.writing(source: string, code: string): boolean
	local last = string.match(string.sub(source, -512), "([^\n]*)%s*$") or ""
	last = string.match(last, "^%s*(.-)%s*$") or ""
	if string.match(last, "^```+$") or string.match(last, "^~~~+$") then
		return false
	end
	local mine = string.match(string.sub(code, -512), "([^\n]*)%s*$") or ""
	mine = string.match(mine, "^%s*(.-)%s*$") or ""
	if mine ~= "" and last == mine then
		return true
	end
	-- (Nothing in it yet: its opening line is the text's last.)
	return mine == "" and (string.match(last, "^```+%s*[%w_-]+") ~= nil or string.match(last, "^~~~+%s*[%w_-]+") ~= nil)
end

-- A token's colour now (tracked), as `#rrggbbaa`.
local function color(token: string): string?
	local v = ui.theme.get(token)
	return if type(v) == "string" then v else nil
end

-- Light ink is a dark theme's (a template's palette included).
local function is_dark(ink: string): boolean
	local r, g, b = string.match(ink, "^#(%x%x)(%x%x)(%x%x)")
	if not r then
		return true
	end
	local l = 0.2126 * tonumber(r, 16) + 0.7152 * tonumber(g :: string, 16) + 0.0722 * tonumber(b :: string, 16)
	return l > 128
end

-- What a diagram is drawn in: the theme now (read tracked, so a change of
-- appearance or palette draws it again), and the key it's kept by.
local function style(): ({ [string]: any }, string)
	ui.theme.current:get()
	local text = color("text.primary") or "#ededefff"
	local colors = {
		background = color("background"),
		code = color("zeron.prose.code.bg"),
		text = text,
		secondary = color("text.secondary"),
		tertiary = color("text.tertiary"),
		border = color("border"),
		strong = color("border.strong"),
		accent = color("accent"),
	}
	local font = ui.theme.get("font.sans")
	local dark = is_dark(text)
	local key = table.concat({
		if dark then "d" else "l",
		if type(font) == "string" then font else "",
		colors.background or "",
		colors.code or "",
		text,
		colors.secondary or "",
		colors.tertiary or "",
		colors.border or "",
		colors.strong or "",
		colors.accent or "",
	}, "|")
	return { dark = dark, font = if type(font) == "string" then font else "", colors = colors }, key
end

-- What's been drawn (or failed), by source and style: a diagram scrolled
-- back, or drawn again in a reply rebuilt, shows at once. (Bounded: the
-- host keeps every one on disk anyway.)
local kept: { [string]: Result } = {}
local kept_count = 0
local function keep(key: string, r: Result)
	if kept[key] == nil then
		kept_count += 1
		if kept_count > 256 then
			kept, kept_count = {}, 1
		end
	end
	kept[key] = r
end

-- A rejection's words (the host's error, its message as a string; or what
-- a stub gave).
local function reason(err: any): string
	local why = if type(err) == "table" then err.message or err.error elseif err ~= nil then tostring(err) else nil
	return if type(why) == "string" and why ~= "" then why else "Diagram could not be rendered"
end

local serial = 0

export type Props = {
	text: string,
	lang: string?,
	-- The fence as code (markdown's code block): while it's written, while
	-- it's drawn, and when it can't be.
	code: () -> Node,
	-- Whether the fence is still being written (see `writing`).
	writing: (() -> boolean)?,
}

-- The strip's buttons: ds/markdown's code block's, concentric with its frame.
local function strip_button(props: { [string]: any }): Node
	props.rounded = ui.raw(control.nest("$radius.xl", 1, 5), "concentric with the code block, as its Copy")
	props.size = "sm"
	return IconButton(props :: any)
end

M.Diagram = ui.component(function(props: Props): Node
	local text = string.gsub(string.gsub(props.text, "\n$", ""), "\t", "    ")
	local lang = if props.lang and props.lang ~= "" then props.lang else "mermaid"
	serial += 1
	-- The thumbnails' group the lightbox grows from (one each).
	local group = `mermaid:{serial}`
	local result = ui.use_state(nil :: Result?)
	local source = ui.use_state(false)
	local copied = ui.use_state(false)
	-- What to draw, by its key ("" while the fence is being written): its
	-- request kept beside it.
	local asks: { [string]: { [string]: any } } = {}
	-- (Once its fence has closed it stays closed: the text is no longer
	-- read as it grows.)
	local closed = props.writing == nil
	local want = ui.computed(function(): string
		if not closed then
			if (props.writing :: () -> boolean)() then
				return ""
			end
			closed = true
		end
		local look, key = style()
		local k = `{key}\0{text}`
		if asks[k] == nil then
			asks[k] = { source = text, dark = look.dark, font = look.font, colors = look.colors }
		end
		return k
	end)
	local alive = true
	ui.on_cleanup(function()
		alive = false
	end)
	ui.effect(function()
		local k = want:get()
		if k == "" then
			return
		end
		local had = kept[k]
		if had then
			result:set(had)
			return
		end
		-- (Until it's drawn, the last drawing stays: another appearance's,
		-- swapped in place; the first time, the code.)
		local ok, promise = pcall(M.render, asks[k])
		if not ok or promise == nil then
			local r = { error = reason(promise) }
			keep(k, r)
			result:set(r)
			return
		end
		promise:after(function(d: any)
			local r: Result = if type(d) == "table" and type(d.svg) == "string" and type(d.plate) == "string" and (d.w or 0) > 0 and (d.h or 0) > 0
				then d
				else { error = "Diagram could not be rendered" }
			keep(k, r)
			if alive and want:peek() == k then
				result:set(r)
			end
		end, function(err: any)
			local r = { error = reason(err) }
			keep(k, r)
			if alive and want:peek() == k then
				result:set(r)
			end
		end)
	end)

	local stop: (() -> ())? = nil
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
	local function drawn(): Drawn?
		local r = result:get()
		return if r and (r :: any).svg then r :: Drawn else nil
	end
	local function open()
		local d = drawn()
		if d then
			Lightbox.open({ { src = d.plate, path = d.plate, svg = d.plate_svg, name = locale.s("ds.mermaid.name") } }, 1, group)
		end
	end
	-- The diagram as the thumbnail the lightbox grows from and goes back to.
	local function thumb(p: { [any]: any }): { [any]: any }
		for k, v in Lightbox.source(group, 1, 0) do
			p[k] = v
		end
		return p
	end

	-- The frame, as ds/markdown's code block's: its strip (the fence's tag,
	-- the toggle, Copy) over the diagram or its source.
	local function frame(showing_source: boolean): Node
		return ui.column {
			class = `md-code lang-mermaid md-mermaid`,
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
				pr = ui.raw(5, "even with Copy's room in the 34 strip"),
				items = "center",
				gap = "$space.1",
				selectable = false,
				focus_group = "horizontal",
				ui.text {
					lang,
					literal = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
					grow = 1,
					min_w = 0,
				},
				strip_button {
					class = "mermaid-toggle",
					label = locale.t(if showing_source then "ds.mermaid.show_diagram" else "ds.mermaid.show_source"),
					icon = if showing_source then "eye" else "file-code",
					on_click = function()
						source:set(not source:peek())
					end,
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
			if showing_source
				then ui.scroll_x {
					w_full = true,
					fade_x = { 16, 32 },
					fade_overflow = true,
					ui.text {
						text,
						font = "$font.mono",
						text_size = "$zeron.prose.code.size",
						line_height = "$zeron.prose.code.line",
						color = "$text.primary",
						literal = true,
						pl = "$space.4",
						pr = "$space.5",
						pb = "$space.3",
					},
				}
				else ui.column {
					w_full = true,
					min_w = 0,
					items = "center",
					px = "$space.4",
					pb = "$space.4",
					-- Its own size, never wider than the column: scaled down
					-- to it, never up.
					ui.div(thumb({
						class = "mermaid-diagram",
						w_full = true,
						max_w = function(): number
							local d = drawn()
							return if d then d.w else 1
						end,
						aspect = function(): number
							local d = drawn()
							return if d and d.h > 0 then d.w / d.h else 1
						end,
						cursor = "zoom_in",
						role = "button",
						label = locale.t("ds.mermaid.view"),
						focusable = true,
						on_click = open,
						on_key = function(e: KeyEvent): boolean?
							if (e.key == "Enter" or e.key == " " or e.key == "Space") and not (e.cmd or e.ctrl or e.alt) then
								open()
								return true
							end
							return nil
						end,
						-- (Vectors: drawn at the size and pixel scale it
						-- shows at, so a zoomed window or a dense display
						-- keeps it sharp.)
						ui.svg {
							src = function(): string
								local d = drawn()
								return if d then "code:" .. d.svg else ""
							end,
							aria_hidden = true,
							w_full = true,
							h_full = true,
						},
					})),
				},
		}
	end

	-- One of: the code (being written, or not drawn yet), the code with why
	-- it can't be drawn, the diagram, or its source in the diagram's frame.
	return ui.presence(function(): string
		if want:get() == "" then
			return "code"
		end
		local r = result:get()
		if r == nil then
			return "code"
		elseif (r :: any).error then
			return "failed"
		end
		return if source:get() then "source" else "diagram"
	end, function(kind: string): Node
		if kind == "code" then
			return props.code()
		elseif kind == "failed" then
			return ui.column {
				w_full = true,
				min_w = 0,
				gap = "$space.2",
				props.code(),
				ui.row {
					class = "mermaid-error",
					w_full = true,
					min_w = 0,
					gap = "$space.2",
					items = "center",
					px = "$space.1",
					color = "$text.tertiary",
					Icon { name = "danger-triangle", size = 12 },
					ui.text {
						function(): string
							local r = result:get()
							return locale.s("ds.mermaid.failed", { reason = if r then (r :: any).error or "" else "" })
						end,
						text_size = "$font.size.xs",
						color = "$text.tertiary",
						grow = 1,
						min_w = 0,
					},
				},
			}
		end
		return frame(kind == "source")
	end, { w_full = true, min_w = 0 })
end)

return M
