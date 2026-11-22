--!strict
-- A list that "n more" opened, kept to a height: laid out as it is while
-- folded (it holds the first few), and at most `max_h` once opened, in a
-- scroll for the rest. What the Control room's lists that grow with the
-- work sit in, so opening one never makes the page as long as the list.
--
--   FoldScroll {
--     open = function() return all:get() end,
--     max_h = 720,                       -- or a function, read as it lays out
--     class = "…",                       -- the scroll's, while open
--     list = function() return ui.each(…) end,
--   }
--
-- (Built again as it opens and folds: a list sized to its own content
-- while folded would chase its own height.)

export type Props = {
	open: () -> boolean,
	max_h: number | () -> number,
	class: string?,
	list: () -> Node,
}

return ui.component(function(props: Props): Node
	local content = ui.use_state(0)
	local function max_h(): number
		local m = props.max_h
		return if type(m) == "function" then m() else m
	end
	return ui.presence(function(): string
		return if props.open() then "open" else "folded"
	end, function(key: string): Node
		if key == "folded" then
			return ui.column { w_full = true, props.list() }
		end
		return ui.scroll {
			class = props.class,
			w_full = true,
			h = function(): any
				return ui.raw(math.min(content:get(), max_h()), "at most max_h, then it scrolls")
			end,
			ui.column {
				w_full = true,
				on_resize = function(size: { w: number, h: number })
					if math.abs(size.h - content:peek()) >= 1 then
						content:set(size.h)
					end
				end,
				props.list(),
			},
		}
	end, { w_full = true })
end)
