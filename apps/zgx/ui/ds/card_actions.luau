-- The ways to share a member card, as one quiet control: Share on X, Copy
-- link, Save image, each its glyph and its words, in one surface (the base
-- surface with a hairline, nothing like a screen's filled way on), a
-- hairline between each. A row of them under a card, or a column beside
-- one (`vertical`, with the width the column takes).
--
-- Each is a ghost pill that starts at its pad, not its middle, as wide as
-- its words at rest, and settles into its done word in place (Shared,
-- Copied: each shorter than the words it replaces), so the control keeps
-- its shape in every state. Which is done is the page's `state`
-- ("shared", "copied", anything else for neither); a click sets it, or
-- sets it back to "ready" (`on_share`, `on_copy`, `on_save` run first,
-- when given: the page's own share, clipboard and picture).
--
-- Its forms (`form`): "row" (under a card), "column" (beside one), and
-- "glyphs" (under a small card, a profile's preview): the glyphs alone in
-- a pill as wide as the card, three equal segments filling it, each glyph
-- centred in its own, the segment its button, the hairlines on the segments' boundaries; each its
-- words as a tooltip and its name to assistive technology, a check in a
-- glyph's place once done.
--
--   CardActions { state = shown }
--   CardActions { state = shown, form = "column", w = 152 }
--   CardActions { state = shown, form = "glyphs", w = 180 }

local Button = require("@self/button")
local Icon = require("@self/icon")
local control = require("@self/control")
local locale = require("@self/locale")

export type Form = "row" | "column" | "glyphs"
export type Props = {
	state: State<string>,
	form: Form?,
	-- (The forms as flags, as the first pages asked for them.)
	vertical: boolean?,
	compact: boolean?,
	-- The column's width (its pills fill it), or the glyph pill's (its
	-- segments share it); a row of words is as wide as its pills.
	w: number?,
	-- The glyph pill's height, its edge included (as tall as the buttons
	-- beside it: setup's footer); its segments take what's inside.
	h: number?,
	on_share: (() -> ())?,
	on_copy: (() -> ())?,
	on_save: (() -> ())?,
}

-- Each way: its glyph (an icon, or a brand's own mark: X's), its words, its
-- done word and the state that means it was done, and its width at rest
-- (measured, with the glyph). The words are their messages' keys.
type Action = { icon: Icon.Name, mark: string?, label: string, done: string?, state: string?, w: number }
local ACTIONS: { Action } = {
	{
		icon = "arrow-up-right",
		mark = "assets/icons/x-mark.svg",
		label = "ds.card_actions.share_x",
		done = "ds.card_actions.shared",
		state = "shared",
		w = 122,
	},
	{ icon = "copy", label = "ds.card_actions.copy_link", done = "ds.card_actions.copied", state = "copied", w = 110 },
	{ icon = "gallery", label = "ds.card_actions.save_image", w = 124 },
}

return function(props: Props): Node
	local shown = props.state
	local form: Form = props.form or (if props.vertical then "column" elseif props.compact then "glyphs" else "row")
	local vertical = form == "column"
	local compact = form == "glyphs"
	-- A segment's width: the pill's, less its edges and inset, and the two
	-- hairlines with their room (zgx's buttons don't grow into a row).
	local hair = (ui.theme.get("zeron.space.hair") :: number?) or 2
	local segment = ((props.w or 180) - 2 - hair * 2 - (1 + hair * 2) * 2) / 3
	-- A segment's height, when the pill's is set: inside its edge and inset.
	local segment_h = if props.h then props.h - 2 - hair * 2 else nil
	local function action(a: Action, run: (() -> ())?): Node
		local function is_done(): boolean
			return a.state ~= nil and shown:get() == a.state
		end
		-- Its glyph at rest: a brand's mark a little under the icons' box
		-- (a mark fills its box; an icon's stroke doesn't), in the ink round it.
		local function rest(): Node
			if a.mark then
				return ui.svg { src = a.mark, tint = "currentColor", w = 14, h = 14 }
			end
			return Icon { name = a.icon, size = 16 }
		end
		local function press()
			if run then
				run()
			end
			if a.state then
				shown:set(if shown:peek() == a.state then "ready" else a.state)
			end
		end
		if compact then
			-- Its segment: an equal share of the pill (no basis of its own, so
			-- the three grow alike), all of it the button, so the pill's one
			-- inset holds on every side of it.
			return ui.div {
				grow = 1,
				basis = 0,
				min_w = 0,
				items = "center",
				justify = "center",
				Button {
					ui.show(is_done, Icon { name = "check", size = 16 }, rest()),
					w = segment,
					h = segment_h,
					px = 0,
					tone = "ghost",
					label = function()
						return locale.s(if is_done() then a.done or a.label else a.label)
					end,
					tooltip = locale.s(a.label),
					on_click = press,
				},
			}
		end
		return Button {
			rest(),
			function()
				return locale.s(if is_done() then a.done or a.label else a.label)
			end,
			tone = "ghost",
			w = if vertical then nil else a.w,
			w_full = vertical,
			shrink = 0,
			justify = "start",
			px = "$space.3",
			on_click = press,
		}
	end
	local runs = { props.on_share, props.on_copy, props.on_save }
	local kids: { Node } = {}
	for i, a in ACTIONS do
		if i > 1 then
			table.insert(kids, ui.div {
				w = if vertical then nil else 1,
				h = if vertical then 1 elseif segment_h then segment_h - 12 else 20,
				w_full = vertical,
				mx = if vertical then nil else "$zeron.space.hair",
				my = if vertical then "$zeron.space.hair" else nil,
				bg = "$border.subtle",
				shrink = 0,
				aria_hidden = true,
			})
		end
		table.insert(kids, action(a, runs[i]))
	end
	local box: ContainerProps = {
		role = "group",
		label = locale.t("ds.card_actions.group"),
		w = props.w,
		items = "center",
		bg = "$surface.base",
		border = 1,
		border_color = "$border.subtle",
		shrink = 0,
		table.unpack(kids),
	}
	-- Its pills take their corner from the box: pills in the pill, and in
	-- the column's rounded box, that corner less the inset.
	control.contain(box, { radius = if vertical then "$radius.lg" else "$radius.full", inset = "$zeron.space.hair", border = 1 })
	return if vertical then ui.column(box) else ui.row(box)
end
