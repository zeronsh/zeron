-- LaTeX math, set as the shipping app sets it (zatex, over its bundled math
-- faces): the host lays a formula out once in ems (src/math.rs,
-- `math.layout`), and this draws it on a canvas, scaled to its text, in the
-- text's ink, so it follows the theme. Screen readers hear the TeX.
--
--   Math { tex = "\\frac{a}{b}", display = true, size = 15 }
--   math.lay_out(tex, display)   the host's answer (or nil where there's
--                                no host: a test's), for a caller that
--                                needs to know before it draws
--
-- How formulas look is one setting for the whole app, kept across launches
-- (Settings → Appearance writes them; as ds/material keeps `frosted`):
--   math.render   whether to set math at all (off: its TeX shows as code)
--   math.face     "fira" | "stix" | "computer-modern" | "libertinus" |
--                 "pagella" | "euler"
--   math.size     percent of the text around a formula (90 to 150)

local M = {}

M.render = ui.persist("math_render", true)
M.face = ui.persist("math_face", "fira")
M.size = ui.persist("math_size", 100)

-- The faces, as Settings lists them: the shipping app's names and styles
-- (a style read as a string in the locale shown, through its message, as
-- it's asked for).
local locale = require("@self/locale")
local function face(value: string, label: string, style_key: string): { value: string, label: string, style: string }
	return setmetatable({ value = value, label = label }, {
		__index = function(_, field: string): string?
			return if field == "style" then locale.s(style_key) else nil
		end,
	}) :: any
end
M.FACES = {
	face("fira", "Fira Math", "ds.math.style.fira"),
	face("stix", "STIX Two", "ds.math.style.stix"),
	face("computer-modern", "Computer Modern", "ds.math.style.computer_modern"),
	face("libertinus", "Libertinus", "ds.math.style.libertinus"),
	face("pagella", "TeX Gyre Pagella", "ds.math.style.pagella"),
	face("euler", "Euler", "ds.math.style.euler"),
}
-- And its sizes, in percent.
M.SIZES = { 90, 100, 110, 120, 135, 150 }

export type Layout = { w: number, ascent: number, descent: number, left: number, path: { number } }

-- The host's layout of `tex` in ems, in the face chosen (read tracked:
-- inside a computed, a new face lays it out again), or `{ error, at }`; nil
-- with no host to ask (a test of a page with math in it). A test may stand
-- in for it.
function M.lay_out(tex: string, display: boolean?): any
	local ok, laid = pcall(ui.call_sync, "math.layout", { tex = tex, display = display == true, face = M.face:get() })
	return if ok then laid else nil
end

-- `laid.path` (opcodes and numbers, in ems) as a path at `scale` points an em.
local function path_of(laid: Layout, scale: number): any
	local p = ui.path()
	local n = laid.path
	local i = 1
	while i <= #n do
		local op = n[i]
		if op == 0 then
			p = p:move(n[i + 1] * scale, n[i + 2] * scale)
			i += 3
		elseif op == 1 then
			p = p:line(n[i + 1] * scale, n[i + 2] * scale)
			i += 3
		elseif op == 2 then
			p = p:quad(n[i + 1] * scale, n[i + 2] * scale, n[i + 3] * scale, n[i + 4] * scale)
			i += 5
		elseif op == 3 then
			p = p:cubic(n[i + 1] * scale, n[i + 2] * scale, n[i + 3] * scale, n[i + 4] * scale, n[i + 5] * scale, n[i + 6] * scale)
			i += 7
		else
			p = p:close()
			i += 1
		end
	end
	return p
end

export type Props = {
	tex: string,
	display: boolean?,
	-- The text's size around it, in points (the prose size by default); the
	-- formula is set at that times `math.size`.
	size: number?,
	-- Its ink: the text's (`$text.primary`) by default.
	ink: string?,
	-- Its layout, when the caller already asked for it.
	laid: Layout?,
	[any]: any,
}

-- A formula. Nil when it can't be set (its TeX doesn't parse, or there's
-- no host): the caller shows its source instead. It follows the face and
-- size chosen in place (its box and drawing read them).
function M.Math(props: Props): Node?
	local first = props.laid or M.lay_out(props.tex, props.display)
	if first == nil or first.error ~= nil then
		return nil
	end
	local tex, display = props.tex, props.display
	-- Laid out again only when the face changes (the host keeps each).
	local laid = ui.computed(function(): any
		local now = M.lay_out(tex, display)
		return if now and now.error == nil then now else first
	end)
	local text = props.size or (ui.theme.get("zeron.prose.size") :: number?) or 15
	local function scale(): number
		return text * M.size:get() / 100
	end
	-- The path for the layout and size it was built for (a path can't be
	-- kept in a state).
	local shape: any, shaped_for: any, shaped_at = nil, nil, 0
	local function current_shape(): any
		local l, at = laid:get(), scale()
		if l ~= shaped_for or at ~= shaped_at then
			shape, shaped_for, shaped_at = path_of(l, at), l, at
		end
		return shape
	end
	local ink = props.ink or "$text.primary"
	local node: { [any]: any } = {
		w = function(): number
			return math.ceil(laid:get().w * scale())
		end,
		h = function(): number
			local l = laid:get()
			return math.ceil((l.ascent + l.descent) * scale())
		end,
		shrink = 0,
		role = "img",
		label = tex,
		draw = function(c: any)
			c:fill(current_shape(), ink)
		end,
	}
	for k, v in props do
		if k ~= "tex" and k ~= "display" and k ~= "size" and k ~= "ink" and k ~= "laid" then
			node[k] = v
		end
	end
	return ui.canvas(node)
end

-- Markdown's math, before markdown reads it (ds/math_source).
M.prepare = require("@self/math_source").prepare

-- A ```math block as a reply shows it: the formula on its own line (when
-- math is on and its TeX can be set), else `fallback()`, the code block
-- it is. Selecting it and copying gives `$$tex$$` back, from its source laid
-- unseen over the formula.
function M.Display(tex: string, fallback: () -> Node): Node
	-- (A real swap: math turned on or off in Settings.)
	return ui.presence(function(): string
		return if M.render:get() then "math" else "code"
	end, function(kind: string): Node
		local box = ui.ref()
		local formula = if kind == "math" then M.Math { tex = tex, display = true, ref = box } else nil
		if not formula then
			return fallback()
		end
		-- Its box, as laid out (the formula's size follows the settings).
		local function w(): number
			return box.width or 0
		end
		local function h(): number
			return box.height or 0
		end
		-- The source's size: in mono (about 0.6 em a character), wider than
		-- the formula, so its selection fills the formula's width (the box
		-- clips the rest).
		local source = `$${tex}$$`
		local function size(): number
			return math.max(1, w() / (0.6 * math.max(1, #source)) * 1.3)
		end
		return ui.div {
			-- What plugins place by (`.md-math`; ds/markdown gives `tex`
			-- above it).
			class = "md-math",
			relative = true,
			w_full = true,
			min_w = 0,
			overflow = "hidden",
			-- Room above and below, as the shipping app keeps (6 at the text's
			-- size), with the blocks' gap around it.
			py = ui.raw(6, "a display formula's clear space, as markdown/math.rs's DISPLAY_PAD_Y"),
			-- Its source, unseen, under the formula and as large: selecting
			-- across it washes the formula's box behind its ink, as a picture
			-- selects, and copying gives `$$tex$$`. (First, so it paints
			-- under the formula.)
			ui.text {
				source,
				literal = true,
				aria_hidden = true,
				font = "$font.mono",
				text_size = size,
				absolute = true,
				left = 0,
				top = ui.raw(6, "the formula's top, past the clear space above it"),
				w = w,
				h = h,
				line_height = h,
				wrap = false,
				overflow = "hidden",
				color = ui.raw("#00000000", "unseen: only selection and copy read it"),
			},
			formula,
		}
	end, { w_full = true })
end

return M
