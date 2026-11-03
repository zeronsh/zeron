-- What a message carries, as the composer shows it over the draft: a strip
-- of chips, one a thing attached, Zeron's tile strip (composer.rs, the
-- attachment tiles) with each chip saying what it holds. A file is its
-- icon beside its name and what it is ("Rust · 48 KB"); text pasted in
-- whole is a chip too, saying how much ("412 lines · 18 KB"); an element
-- picked on a page is an annotation, a compact chip: its number (the one
-- its mark wears on the page) beside its element, or the note typed for
-- it, on one line; a passage quoted from a conversation is one too
-- (ds/quote), who said it over a short excerpt. An image is its own thumbnail:
-- the chip's height, wide as a window is, so a screenshot shows whole.
--
-- Every chip is one height, whatever its state. One still uploading wears
-- a ring where its glyph was (over the thumbnail, on a scrim) and says how
-- far; one that failed says so where its detail was, in the danger's ink,
-- with Retry at its end. Under the pointer (or with the keys) an × hangs
-- at the chip's top-right corner, over its edge (Zeron's tiles'): laid
-- over the chip, taking no room of its own, so no chip moves. The strip
-- keeps room above its first row and the pill's inset keeps room at its
-- end, so a corner's × is never clipped.
--
-- The chip under the pointer is `hovered` (its id), for whatever shows
-- what a chip stands for elsewhere (the page's mark firms).
--
-- Chips share the draft's left edge and wrap. Past eight, the strip shows
-- six and "+N more", a quiet word that opens the rest; opened, "Show less"
-- stands in its place. A chip arriving scales in; one leaving fades as the
-- others close up.
--
-- `Strip { items = cargo, on_remove = …, on_retry = …, on_open = … }`.

local Icon = require("@self/icon")
local HoverCard = require("@self/hover_card")
local Mark = require("@self/mark")
local FileIcon = require("@self/file_icons")
local FadeText = require("@self/fade_text")
local control = require("@self/control")
local locale = require("@self/locale")

local M = {}

-- ("comment": a review comment left on a file.)
export type Kind = "image" | "file" | "text" | "annotation" | "comment" | "appshot"
export type Item = {
	id: string,
	kind: Kind,
	name: string,
	-- An image: its picture.
	src: string?,
	-- What it is, under its name ("Rust · 24 KB", "412 lines · 18 KB").
	meta: string?,
	-- Still uploading: how far, 0 to 1.
	progress: number?,
	-- Failed: why, in a few words ("Couldn't upload").
	failed: string?,
	-- An annotation: the element's selector ("button#save.btn.primary"),
	-- the page it is on, its box, and its markup (a few lines).
	element: string?,
	page: string?,
	box: { number }?,
	html: string?,
	-- A quoted passage, whole: its card shows it.
	quote: string?,
	-- An Appshot (screens/chat/appshots): its picture is `src`, its app's
	-- icon `icon`; what the host writes of it rides in `appshot`.
	icon: string?,
	appshot: any?,
	bytes: number?,
	-- An annotation's number, the note typed for it, and whether its name
	-- is code (a tag and class) rather than words.
	index: number?,
	note: string?,
	code: boolean?,
}
export type Props = {
	items: State<{ Item }>,
	on_remove: ((id: string) -> ())?,
	on_retry: ((id: string) -> ())?,
	on_open: ((id: string) -> ())?,
	-- An image chip as the lightbox's thumbnail (ds/lightbox `source`): the
	-- props that let its picture grow from the chip and go back into it.
	source: ((item: Item) -> { [string]: any }?)?,
}

-- How many chips show before the rest fold into "+N more", and how many
-- then.
local FOLD_PAST, FOLD_TO = 8, 6
-- The glyph's slot, and the ring in it.
local SLOT, RING = 18, 18
-- An Appshot's picture in its slot.
local APPSHOT_W, APPSHOT_H = 36, 26
-- The ×: its disc, and how far it hangs past the chip's corner.
local CROSS, CROSS_OUT = 18, 5
-- How the × comes and goes.
local SWAP: TweenOptions = { duration = 140 }

-- The chip under the pointer, by id (nil: none).
M.hovered = ui.state("composer_cargo_hovered", nil :: string?)

local function over(item: Item, inside: boolean)
	if inside then
		M.hovered:set(item.id)
	elseif M.hovered:peek() == item.id then
		M.hovered:set(nil)
	end
end

local function n(token: string, fallback: number): number
	local v = ui.theme.get(token)
	return if type(v) == "number" then v else fallback
end

-- The hollow round a chip's ×: the chip cleared in a disc as wide as the
-- button and the hollow's width round it, edged over half a point either
-- side (no fade past that), so what is behind the strip (the pill's
-- glass, or its fill) shows in the gap: a cut, not a painted ring, as
-- ds/avatar cuts a stack. The × stands in the cut, outside the chip's
-- layer, so its fill lies on what is behind the chip, one even disc
-- whether over the chip or past its corner. It is the chip's whole layer,
-- so a picture is cut as a file is.
-- params[0]: the ×'s centre (in from the box's right edge, down from its
-- top), the cut's radius, and how far it is open (0 to 1, with the ×).
local HOLLOW = ui.layer_shader([[
fn layer(input:LayerInput)->vec4<f32> {
 let c=vec2(input.size.x-input.params[0].x,input.params[0].y);
 let d=distance(input.p,c);
 let cut=(1.-smoothstep(-.5,.5,d-input.params[0].z))*clamp(input.params[0].w,0.,1.);
 return vec4(input.color.rgb,input.color.a*(1.-cut));
}
]])

-- The hollow's props for a chip whose × is lit by `lit`: the layer and its
-- parameters, the cut opening as the × fades in (and closing as it goes).
local function hollow(lit: () -> boolean): { [string]: any }
	local box = ui.ref()
	local width = n("zeron.composer.chip.hollow", 2)
	ui.effect(function()
		local on = lit()
		local el = box:peek()
		if el then
			anime.animate(el, { layerParam3 = if on then 1 else 0, duration = SWAP.duration })
		end
	end)
	return {
		ref = box,
		layer_shader = HOLLOW,
		layer_params = { { CROSS / 2 - CROSS_OUT, CROSS / 2 - CROSS_OUT, CROSS / 2 + width, 0 } },
	}
end

-- A ring of `ink`, `f` of the way round from the top, over a faint full
-- turn: the upload's progress, drawn where it is.
local function ring(f: () -> number, ink: string, faint: string): Node
	return ui.canvas {
		w = RING,
		h = RING,
		shrink = 0,
		draw = function(c, w, h)
			local r = w / 2 - 2
			c:stroke(ui.path():arc(w / 2, h / 2, r, 0, 360), faint, 2)
			local v = math.clamp(f(), 0, 1)
			if v > 0 then
				c:stroke(ui.path():arc(w / 2, h / 2, r, -90, 360 * v), ink, 2, { cap = "round" })
			end
		end,
	}
end

-- The × that takes a chip off: a disc hung at the chip's top-right corner,
-- over its edge, laid over the chip (no room of its own), shown while
-- `lit` (the chip under the pointer, or the × with the keys).
local function cross(item: Item, lit: () -> boolean, on_remove: (id: string) -> (), on_lit: (boolean) -> (), name: (() -> string)?): Node
	-- (Its chip's name as it is now, when the chip's can change.)
	local function named(): string
		return locale.s("ds.composer_cargo.remove", { name = if name then name() else item.name })
	end
	return ui.button {
		absolute = true,
		top = -CROSS_OUT,
		right = -CROSS_OUT,
		w = CROSS,
		h = CROSS,
		p = 0,
		justify = "center",
		items = "center",
		rounded = "$radius.full",
		bg = "$zeron.composer.chip.cross",
		color = "$text.secondary",
		cursor = "pointer",
		hover = { bg = "$zeron.composer.chip.cross", color = "$text.primary" },
		active = { bg = "$zeron.composer.chip.cross", color = "$text.primary" },
		focus = control.ring,
		label = named,
		tooltip = named(),
		opacity = ui.tween(ui.computed(function(): number
			return if lit() then 1 else 0
		end), SWAP),
		on_focus = function()
			on_lit(true)
		end,
		on_blur = function()
			on_lit(false)
		end,
		on_click = function()
			on_remove(item.id)
		end,
		Icon { name = "close", size = 10 },
	}
end

-- A thumbnail's scrim, with what it holds over the picture.
local function scrim(...: Node): Node
	return ui.div {
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		items = "center",
		justify = "center",
		bg = "$zeron.composer.chip.scrim",
		color = "$zeron.composer.chip.scrim_ink",
		pointer_events = "none",
		...,
	}
end

-- An image: its picture cropped to the chip's height, a hairline over its
-- edge (a light picture on a light pill would lose it), the ring or the
-- failure over it, the × at its corner (outside the picture's clip).
local function image_chip(item: Item, row: State<Item>, props: Props): Node
	local hovered = ui.use_state(false)
	local focused = ui.use_state(false)
	local function lit(): boolean
		return hovered:get() or focused:get()
	end
	local h = n("zeron.composer.chip.h", 40)
	local w = n("zeron.composer.chip.thumb", 56)
	local function uploading(): boolean
		local p = row:get().progress
		return p ~= nil and p < 1 and row:get().failed == nil
	end
	local function failed(): boolean
		return row:get().failed ~= nil
	end
	local remove = props.on_remove
	local cut = hollow(lit)
	local lifted = if props.source then props.source(item) else nil
	return ui.overlay {
		w = w,
		h = h,
		shrink = 0,
		relative = true,
		rounded = "$zeron.composer.chip.radius",
		label = item.name,
		-- (Opening: with the keys too, and the lightbox's thumbnail.)
		focusable = props.on_open ~= nil,
		focus = control.ring,
		on_mount = if lifted then lifted.on_mount else nil,
		on_visible = if lifted then lifted.on_visible else nil,
		opacity = if lifted then lifted.opacity else nil,
		tooltip = if item.meta then `{item.name} · {item.meta}` else item.name,
		cursor = if props.on_open then "pointer" else nil,
		on_click = if props.on_open
			then function()
				(props.on_open :: (string) -> ())(item.id)
			end
			else nil,
		on_hover = function(inside: boolean)
			hovered:set(inside)
			over(item, inside)
		end,
		-- (The picture is the cut layer; the × stands beside it, uncut.)
		ui.overlay {
			ref = cut.ref,
			layer_shader = cut.layer_shader,
			layer_params = cut.layer_params,
			w_full = true,
			h_full = true,
			rounded = "$zeron.composer.chip.radius",
			overflow = "hidden",
			border = 1,
			border_color = "$zeron.composer.chip.edge",
			border_over = true,
			-- (Its file gone since, or not a picture: the gallery's glyph in
			-- its place, `ui.image`'s `fallback`.)
			ui.image {
				src = item.src,
				fit = "cover",
				w_full = true,
				h_full = true,
				fallback = scrim(ui.div { color = "$text.tertiary", Icon { name = "gallery", size = 16 } }),
			},
			ui.show(uploading, scrim(ring(function(): number
				return row:get().progress or 0
			end, "$zeron.composer.chip.scrim_ink", "#ffffff4d"))),
			ui.show(failed, scrim(ui.div { color = "$zeron.tone.danger.ink", Icon { name = "danger-triangle", size = 16 } })),
		},
		if remove
			then cross(item, lit, remove, function(on: boolean)
				focused:set(on)
			end)
			else nil,
	}
end

-- A file, pasted text, or an annotation: its glyph, its name over what it
-- is. Uploading, the glyph's slot holds the ring and the detail says how
-- far; failed, the slot holds the warning, the detail says so, and Retry
-- stands at the end.
-- An element picked on a page in the browser panel is a row chip as a
-- quoted passage is (one anatomy): the bare annotate glyph; its number
-- (as its mark on the page wears it, renumbered with it as others leave)
-- before the page's address, host and path, never cut short, fading at
-- the chip's edge; under it the note typed for it, else what it says of
-- the element. Under the pointer its mark on the page lights, and a card
-- says what the element is (the shipping app's). Nothing to open: no
-- pointer, no click.
-- The markup the card shows under the pointer: its first lines (the
-- shipping app's ANNOTATION_SNIPPET_LINES), an ellipsis past them.
local CARD_LINES = 4
local CARD_W = 360

-- What the element is, under the pointer (the shipping app's annotation
-- card): its tag, a weight up, its id and classes muted, its size at the
-- end; under a rule, the first lines of its markup in the mono face.
local function pick_card(a: any): Node?
	if type(a) ~= "table" or type(a.element) ~= "string" then
		return nil
	end
	local cut = string.find(a.element, "[#.]")
	local tag = if cut then string.sub(a.element, 1, cut - 1) else a.element
	local rest = if cut then string.sub(a.element, cut) else ""
	local rect = if type(a.rect) == "table" then a.rect else {}
	local w, hgt = math.floor((rect[3] or 0) + 0.5), math.floor((rect[4] or 0) + 0.5)
	local lines: { string } = {}
	local more = false
	for line in string.gmatch(if type(a.html) == "string" then a.html else "", "[^\n]+") do
		if string.find(line, "%S") then
			if #lines >= CARD_LINES then
				more = true
				break
			end
			table.insert(lines, line)
		end
	end
	if more then
		table.insert(lines, "…")
	end
	local code: { any } = {}
	for _, line in lines do
		table.insert(code, FadeText { line, literal = true, w_full = true, font = "$font.mono", text_size = "$font.size.xs", line_height = "$font.line.xs", color = "$text.secondary" })
	end
	return HoverCard.Card {
		w = CARD_W,
		p = 0,
		gap = 0,
		ui.row {
			w_full = true,
			h = 30,
			px = "$space.3",
			gap = "$space.2",
			items = "center",
			ui.row {
				grow = 1,
				min_w = 0,
				items = "center",
				ui.text { tag, literal = true, shrink = 0, text_size = "$font.size.sm", weight = "$font.weight.semibold", color = "$text.primary" },
				FadeText { rest, literal = true, grow = 1, text_size = "$font.size.sm", color = "$text.tertiary" },
			},
			if w > 0 or hgt > 0
				then ui.text { `{w} × {hgt}`, literal = true, shrink = 0, tabular = true, text_size = "$font.size.sm", color = "$text.tertiary" }
				else nil,
		},
		if #code > 0
			then ui.column {
				w_full = true,
				px = "$space.3",
				py = "$space.2",
				border_sides = { 1, 0, 0, 0 },
				border_color = "$border.subtle",
				table.unpack(code),
			}
			else nil,
	}
end

-- A review comment's card, as it hovers: who left it (a plugin's), its
-- note, and the code that goes with it, every line of what's sent (the
-- host holds it to 40 lines), so nothing reaches the agent unseen.
local function comment_card(c: any): Node?
	if type(c) ~= "table" or (type(c.snippet) ~= "string" and type(c.plugin) ~= "string") then
		return nil
	end
	local code: { any } = {}
	for line in string.gmatch(if type(c.snippet) == "string" then c.snippet .. "\n" else "", "([^\n]*)\n") do
		table.insert(code, FadeText { line, literal = true, w_full = true, font = "$font.mono", text_size = "$font.size.xs", line_height = "$font.line.xs", color = "$text.secondary" })
	end
	return HoverCard.Card {
		w = CARD_W,
		p = 0,
		gap = 0,
		ui.column {
			w_full = true,
			px = "$space.3",
			py = "$space.2",
			gap = "$zeron.space.hair",
			if type(c.plugin) == "string"
				then ui.text { locale.t("ds.composer_cargo.comment_from", { plugin = c.plugin }), text_size = "$font.size.xs", color = "$text.tertiary" }
				else nil,
			ui.text { c.name or "", literal = true, text_size = "$font.size.sm", weight = "$font.weight.semibold", color = "$text.primary" },
			if type(c.body) == "string" and c.body ~= ""
				then ui.text { c.body, literal = true, wrap = true, text_size = "$font.size.sm", color = "$text.primary" }
				else nil,
		},
		if #code > 0
			then ui.column {
				w_full = true,
				px = "$space.3",
				py = "$space.2",
				border_sides = { 1, 0, 0, 0 },
				border_color = "$border.subtle",
				table.unpack(code),
			}
			else nil,
	}
end

-- A page's address as a chip names it: no scheme, no "www.", no lone
-- trailing slash.
local function page_of(url: any): string?
	if type(url) ~= "string" or url == "" then
		return nil
	end
	local s = string.gsub(url, "^%a[%w+.-]*://", "")
	s = string.gsub(s, "^www%.", "")
	if string.sub(s, -1) == "/" and not string.find(string.sub(s, 1, -2), "/", 1, true) then
		s = string.sub(s, 1, -2)
	end
	return s
end

local function row_chip(item: Item, row: State<Item>, props: Props): Node
	local hovered = ui.use_state(false)
	local focused = ui.use_state(false)
	local function lit(): boolean
		return hovered:get() or focused:get()
	end
	local h = n("zeron.composer.chip.h", 40)
	local glyph_size = n("zeron.composer.chip.glyph", 18)
	local function uploading(): boolean
		local p = row:get().progress
		return p ~= nil and p < 1 and row:get().failed == nil
	end
	local function failed(): boolean
		return row:get().failed ~= nil
	end
	local glyph: Node
	if item.kind == "file" then
		glyph = FileIcon { name = item.name, size = glyph_size }
	elseif item.kind == "annotation" then
		glyph = Icon { name = "cursor", size = glyph_size - 2 }
	elseif item.kind == "comment" then
		glyph = Icon { name = "chat-round-line", size = glyph_size - 2 }
	elseif item.kind == "appshot" then
		-- An Appshot: a small picture of the window, its app's icon on its
		-- corner (the shipping app's tile, at a chip's size).
		glyph = ui.overlay {
			w = APPSHOT_W,
			h = APPSHOT_H,
			relative = true,
			ui.overlay {
				w_full = true,
				h_full = true,
				rounded = ui.raw(control.nest("$zeron.composer.chip.radius", (n("zeron.composer.chip.h", 40) - APPSHOT_H) / 2), "concentric with the chip: its corner less the picture's inset"),
				overflow = "hidden",
				border = 1,
				border_color = "$zeron.composer.chip.edge",
				border_over = true,
				bg = "$hover",
				ui.image { src = item.src, fit = "cover", w_full = true, h_full = true },
			},
			if item.icon
				then ui.image { src = item.icon, absolute = true, right = -4, bottom = -4, w = 14, h = 14, fit = "contain", aria_hidden = true }
				else nil,
		}
	elseif item.kind == "chat" then
		-- Another chat mentioned: its agent's mark, as the sidebar's row.
		glyph = if item.mark then Mark { name = item.mark, size = glyph_size - 2, bare = true } else Icon { name = "chat-round-line", size = glyph_size - 2 }
	else
		glyph = Icon { name = "wrap-text", size = glyph_size - 2 }
	end
	local remove = props.on_remove
	-- An element picked on a page (the browser panel's): its number and
	-- page over its note or words, its card under the pointer, no click.
	local pick = item.kind == "annotation" and item.element ~= nil
	local opens = props.on_open ~= nil and not pick
	-- The slot: the glyph, or the ring, or the warning.
	local slot = ui.div {
		w = if item.kind == "appshot" then APPSHOT_W else SLOT,
		h = if item.kind == "appshot" then APPSHOT_H else SLOT,
		shrink = 0,
		items = "center",
		justify = "center",
		color = "$text.secondary",
		ui.show(uploading, ring(function(): number
			return row:get().progress or 0
		end, "$text.primary", "$border.strong"), ui.show(failed, ui.div { color = "$zeron.tone.danger.ink", Icon { name = "danger-triangle", size = 16 } }, glyph)),
	}
	local function meta(): string
		local now = row:get()
		if pick then
			return if now.note and now.note ~= "" then now.note else (now.name or "")
		end
		if now.failed then
			return now.failed
		end
		if now.progress and now.progress < 1 then
			return locale.s("ds.composer_cargo.uploading", { n = math.floor(now.progress * 100 + 0.5) })
		end
		return now.meta or ""
	end
	local card: Node? = if pick then pick_card(item.annotation) elseif item.kind == "comment" then comment_card(item) else nil
	local function title(): string
		local now = row:get()
		if pick then
			return page_of(now.annotation and now.annotation.url) or now.page or now.name or ""
		end
		return now.name
	end
	-- (A passage from the conversation has no card: its chip's line, who
	-- said it over the words, is all it shows.)
	local cut = hollow(lit)
	-- The chip (the cut layer) and the × beside it, uncut, in a box that
	-- holds both and knows the pointer over either.
	return ui.row {
		-- (In points: `max_w` doesn't take a token.)
		max_w = n("zeron.composer.chip.max", 214),
		shrink = 0,
		relative = true,
		label = function(): string
			if pick then
				return locale.s("ds.composer_cargo.annotation", { n = row:get().index or 0, note = `{title()}: {meta()}` })
			end
			return row:get().name
		end,
		on_hover = function(inside: boolean)
			hovered:set(inside)
			over(item, inside)
		end,
		ui.row {
			ref = cut.ref,
			layer_shader = cut.layer_shader,
			layer_params = cut.layer_params,
			h = h,
			min_w = 0,
			-- (Held to the chip's widest too: a pick's two lines, each wider,
			-- fade at its edge rather than push it wider.)
			max_w = n("zeron.composer.chip.max", 214),
			pl = "$zeron.space.snug",
			pr = "$space.2",
			gap = "$space.2",
			items = "center",
			rounded = "$zeron.composer.chip.radius",
			bg = "$zeron.composer.chip.bg",
			tooltip = card,
			tooltip_placement = if card then { side = "top", align = "start", offset = 8 } else nil,
			cursor = if opens then "pointer" else nil,
			on_click = if opens
				then function()
					(props.on_open :: (string) -> ())(item.id)
				end
				else nil,
			slot,
			ui.column {
				min_w = 0,
				grow = 1,
				gap = 0,
				ui.row {
					-- (Held to the column's width, so the page fades at the
					-- chip's edge rather than pushing it wider.)
					w_full = true,
					min_w = 0,
					gap = "$space.1",
					items = "center",
					-- A pick's number, as its mark on the page wears it.
					if pick
						then ui.text {
							function(): string
								return tostring(row:get().index or 0)
							end,
							literal = true,
							shrink = 0,
							tabular = true,
							text_size = "$font.size.md",
							line_height = "$font.line.md",
							weight = "$font.weight.semibold",
							color = "$zeron.tone.accent.ink",
						}
						else nil,
					FadeText {
						-- (Its row's: a comment carried to another line says so; a
						-- pick's page.)
						title,
						grow = 1,
						literal = true,
						text_size = "$font.size.md",
						line_height = "$font.line.md",
						weight = "$font.weight.medium",
						color = "$text.primary",
					},
				},
				-- (Only with something to say: an empty line would hold the
				-- name off the chip's middle.)
				ui.show(
					function(): boolean
						return meta() ~= ""
					end,
					FadeText {
						meta,
						literal = true,
						text_size = "$font.size.xs",
						line_height = "$font.line.xs",
						color = function()
							return if failed() then "$zeron.tone.danger.ink" else "$text.tertiary"
						end,
					}
				),
			},
			-- Failed: the way back, in the chip.
			ui.show(
				failed,
				ui.button {
					label = locale.t("ds.composer_cargo.retry_named", { name = item.name }),
					h = 24,
					px = "$space.2",
					shrink = 0,
					rounded = ui.raw(control.nest("$zeron.composer.chip.radius", "$space.2"), "concentric with the chip: its corner less the button's inset"),
					bg = "#00000000",
					color = "$text.primary",
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
					cursor = "pointer",
					hover = { bg = "$hover" },
					active = { bg = "$pressed" },
					focus = control.ring,
					on_click = function()
						if props.on_retry then
							props.on_retry(item.id)
						end
					end,
					locale.t("ds.composer_cargo.retry"),
				}
			),
		},
		if remove
			then cross(item, lit, remove, function(on: boolean)
				focused:set(on)
			end, function(): string
				return row:get().name
			end)
			else nil,
	}
end

-- A chip for one thing carried.
function M.Chip(item: Item, row: State<Item>, props: Props): Node
	if item.kind == "image" then
		return image_chip(item, row, props)
	end
	return row_chip(item, row, props)
end

-- The strip: the chips, wrapping; past eight, six and "+N more"; opened,
-- "Show less" where that was.
M.Strip = ui.component(function(props: Props): Node
	local all = props.items
	local opened = ui.use_state(false)
	local FOLD = "\0fold"
	local function folds(): boolean
		return #all:get() > FOLD_PAST
	end
	-- What shows: every chip, or the first few; and the fold's word when
	-- there is anything to fold.
	local shown = ui.computed(function(): { Item }
		local list = all:get()
		if not folds() then
			return list
		end
		local out: { Item } = {}
		for i, it in list do
			if opened:get() or i <= FOLD_TO then
				table.insert(out, it)
			end
		end
		local fold: Item = { id = FOLD, kind = "text", name = FOLD }
		table.insert(out, fold)
		return out
	end)
	local function hidden(): number
		return math.max(0, #all:get() - FOLD_TO)
	end
	-- Too few to fold: back to the start.
	ui.effect(function()
		if not folds() then
			opened:set(false)
		end
	end)
	local h = n("zeron.composer.chip.h", 40)
	-- A chip follows its item (its progress climbing, its failure): zgx's
	-- animated rows get no state of their own, so each finds its item in
	-- the list by its id.
	local function follow(item: Item): State<Item>
		return ui.computed(function(): Item
			for _, it in all:get() do
				if it.id == item.id then
					return it
				end
			end
			return item
		end)
	end
	return ui.row {
		w_full = true,
		flex_wrap = true,
		items = "center",
		gap = "$zeron.composer.chip.gap",
		px = "$zeron.composer.chip.inset",
		pt = "$zeron.composer.chip.top",
		ui.each(shown, function(item: Item): Node
			if item.id == FOLD then
				return ui.button {
					label = function()
						return if opened:get()
							then locale.s("ds.composer_cargo.show_fewer")
							else locale.s("ds.composer_cargo.show_more", { n = hidden() })
					end,
					expanded = opened,
					h = h,
					px = "$space.2",
					shrink = 0,
					rounded = "$zeron.composer.chip.radius",
					bg = "#00000000",
					color = "$text.tertiary",
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
					cursor = "pointer",
					hover = { bg = "$hover", color = "$text.primary" },
					active = { bg = "$pressed", color = "$text.primary" },
					focus = control.ring,
					on_click = function()
						opened:set(not opened:peek())
					end,
					ui.text {
						function()
							return if opened:get() then locale.s("ds.composer_cargo.show_less") else locale.s("ds.composer_cargo.more", { n = hidden() })
						end,
						tabular = true,
					},
				}
			end
			return M.Chip(item, follow(item), props)
		end, {
			key = function(item: Item): string
				return item.id
			end,
			direction = "row",
			flex_wrap = true,
			items = "center",
			gap = "$zeron.composer.chip.gap",
			animate = { duration = 220, ease = "outCubic" },
			enter = { opacity = { 0, 1 }, scale = { 0.9, 1 }, duration = 220, ease = "outCubic" },
			exit = { opacity = 0, scale = 0.9, duration = 140, ease = "outCubic" },
		}),
	}
end)

return M
