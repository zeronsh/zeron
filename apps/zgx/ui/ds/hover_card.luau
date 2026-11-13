-- What shows as the pointer rests on something (a node `tooltip`): a card,
-- or a tip of words, framed once, in the menus' own surface: the glass the
-- pickers' panels wear (ds/glass `menu`: the menus' tint over a blur of
-- what's behind, their edge, their shadow, their corner), so a hover card
-- and a menu are one material. Every node tooltip in the app draws through
-- here (tests/hover_cards.rs fails on one that doesn't).
--
-- zgx shows a node tooltip in a popup window of its own, its panel zgx's
-- clear layer hugging the card. Since zgx b75d8e74f (#1390) the popup finds
-- the card through that layer: a card with its own shadow turns the window
-- server's off (no second rim round it), and a translucent card gets
-- AppKit's material under it, cut to its corners, as a menu's panel does
-- (zgx-asks/popup-layer-shadow, adopted).
--
--   tooltip = HoverCard.Card { w = 248, p = "$space.1", … },
--   tooltip_placement = { side = "top", align = "end", offset = 8 },
--
-- `Card` and `Tip` (the small one words take: ds/tooltip) are the same
-- surface; each takes a column's props and children. `frame(props)` adds
-- it to props of the caller's own.

local glass = require("@self/glass")

local M = {}

export type Kind = "card" | "tip"

-- Adds the frame to `props` (changed in place, and returned): the menus'
-- glass, once; what it holds paints none of its own round it.
function M.frame(props: { [any]: any }, _kind: Kind?): { [any]: any }
	props.class = if props.class then `{props.class} hover-card-frame` else "hover-card-frame"
	return glass.style(props, "menu")
end

export type Props = Merge<ContainerProps, { [number]: Child }>
export type CardProps = Merge<ContainerProps, { [number]: Child, bare: boolean? }>

-- A card: a column in the frame. `bare = true`: the column alone, for a
-- panel that wears the frame itself (`Anchored`'s pinned card).
function M.Card(props: CardProps): Node
	local p: any = table.clone(props)
	local bare = p.bare
	p.bare = nil
	if bare then
		return ui.column(p)
	end
	return ui.column(M.frame(p, "card"))
end

-- A tip of words: a column in the frame.
function M.Tip(props: Props): Node
	return ui.column(M.frame(table.clone(props), "tip"))
end

-- A hover card with actions in it (a way in, a button), on the thing it is
-- about, as a tooltip with actions behaves everywhere:
--
-- - It hangs off the anchor's box (below it, lined up with its start, by
--   default), flipping above or clamped to stay in the window: never at the
--   pointer, so the pointer can always reach it.
-- - It stays while the pointer is on the anchor or the card, and as it
--   crosses the gap between them (zgx's interactive tooltip, with its
--   moment's grace). Off both, it goes after that grace; Escape closes it.
-- - `pinned` (the caller's state; a click on the anchor toggles it): the
--   same card stays open, in a popover on the anchor, until a click outside
--   it, Escape, or a click on the anchor again. One card is pinned at a
--   time, and while one is, the pointer on another anchor shows nothing
--   over it. Pinned, its first control has focus: its actions are in reach
--   of the keys (Tab, Enter) too.
-- - An action that leaves (a way in) calls `close`: the card goes, pinned
--   or not, until the pointer comes back to the anchor.
--
--   HoverCard.Anchored {
--     anchor = node,                     -- the chip, the row
--     card = function(c) return HoverCard.Card { bare = c.bare, w = 480, … } end,
--     w = 480,                           -- the card's width (its panel's, pinned)
--     pinned = state,                    -- optional: click-to-pin
--     placement = { side = "bottom", align = "start", offset = 6 },
--     delay = 300,                       -- ms resting before it shows
--   }
--
-- `card` is called with `{ bare, close }`: `bare` is true in the pinned
-- popover, whose panel wears the frame, so the card leaves its own off.

export type CardContext = { bare: boolean, close: () -> () }
export type Placement = { side: string?, align: string?, offset: number? }
export type AnchoredProps = {
	anchor: Node,
	card: (c: CardContext) -> Node,
	w: number,
	pinned: State<boolean>?,
	placement: Placement?,
	delay: number?,
	class: string?,
	-- The anchor's box: as wide as where it's put (a line).
	w_full: boolean?,
	-- The pointer coming onto the card, or leaving it.
	on_card_hover: ((inside: boolean) -> ())?,
}

-- The card pinned now, by its anchor's key ("" for none); tests read it as
-- `hover_card_pinned`.
local pinned_now = ui.state("hover_card_pinned", "")
local next_key = 0

-- Under the anchor, from its start.
M.PLACEMENT = { side = "bottom", align = "start", offset = 6 }

M.Anchored = ui.component(function(props: AnchoredProps): Node
	next_key += 1
	local key = `hc{next_key}`
	local placement = props.placement or M.PLACEMENT
	local pinned = props.pinned
	-- Closed by an action in it: nothing over the anchor till the pointer
	-- comes back to it.
	local dismissed = ui.use_state(false)
	local function close()
		dismissed:set(true)
		if pinned and pinned:peek() then
			pinned:set(false)
		end
	end

	if pinned then
		local p = pinned :: State<boolean>
		-- This one pinned: the one pinned (another let go).
		ui.effect(function()
			if p:get() then
				dismissed:set(false)
				if pinned_now:peek() ~= key then
					pinned_now:set(key)
				end
			elseif pinned_now:peek() == key then
				pinned_now:set("")
			end
		end)
		-- Another pinned: this one isn't.
		ui.effect(function()
			if pinned_now:get() ~= key and p:peek() then
				p:set(false)
			end
		end)
		ui.on_cleanup(function()
			if pinned_now:peek() == key then
				pinned_now:set("")
			end
		end)
	end

	-- On hover: only while no card is pinned (this one's own shows in its
	-- popover then) and none was just closed by its action.
	local hover = ui.column {
		class = "hover-card-hover",
		on_hover = props.on_card_hover,
		ui.show(function(): boolean
			return pinned_now:get() == "" and not dismissed:get()
		end, props.card({ bare = false, close = close })),
	}

	local holder = ui.row {
		class = props.class or "hover-card-anchor",
		shrink = 0,
		w_full = props.w_full,
		min_w = 0,
		items = "center",
		tooltip = hover,
		tooltip_interactive = true,
		tooltip_placement = placement :: any,
		tooltip_delay = props.delay or 300,
		on_hover = function(inside: boolean)
			if inside and dismissed:peek() then
				dismissed:set(false)
			end
		end,
		props.anchor,
	}
	if not pinned then
		return holder
	end
	return ui.popover(M.frame({
		class = "hover-card-pinned",
		open = pinned,
		anchor = holder,
		placement = placement :: any,
		w = props.w,
		p = 0,
		build = function(): Node
			return props.card({ bare = true, close = close })
		end,
	}, "card"))
end)
return M
