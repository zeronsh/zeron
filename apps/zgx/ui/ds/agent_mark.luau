-- An agent's mark wherever agents are listed (Providers' cards, Add an
-- agent's rows, the model picker, the composer, the sidebar), the one place
-- they're drawn, as ds/agent_look decides:
--
-- - an agent Zeron drives itself: its bundled brand mark (ds.Mark);
-- - an agent from the ACP registry: the registry's own icon for it, in the
--   theme's text colour on a quiet tile, or bare in the text's colour round
--   it;
-- - no icon (yet, or it didn't load): the registry's glyph in the same ink;
--   one of your own, its initial.
--
--   AgentMark { label = "Gemini CLI", seed = "gemini", icon = svg, size = 32 }
--   AgentMark { label = "Claude Code", mark = "claude", size = 32 }
--
-- `seed`: the agent's registry id (or its harness, `acp.<id>`). `src`: a
-- bundled file in place of `icon`. `bare`: the glyph alone, beside text (a
-- picker's row, a sidebar row).

local Mark = require("@self/mark")
local Tile = require("@self/tile")
local Look = require("@self/agent_look")
local RegistryMark = require("@self/registry_mark")

export type Props = {
	label: string,
	mark: string?,
	icon: string?,
	src: string?,
	seed: string?,
	size: number?,
	bare: boolean?,
}

local M = {}

M.look = Look.look

function M.Mark(props: Props): Node
	local size = props.size or 32
	local look = Look.look(props)
	if look.kind == "builtin" then
		return Mark { name = look.mark :: any, size = size, bare = props.bare }
	end
	if look.kind == "initial" then
		return Tile { label = props.label, size = size }
	end
	-- A registry agent with no icon given: found by its harness alone.
	if look.kind == "fallback" and props.seed then
		return RegistryMark { harness = props.seed :: string, size = size, bare = props.bare }
	end
	local glyph = size * (if props.bare then 1 else 0.6)
	local svg = ui.svg {
		code = look.code,
		src = if look.code then nil else look.src,
		tint = look.tint,
		w = glyph,
		h = glyph,
		shrink = 0,
		aria_hidden = true,
	}
	if props.bare then
		return svg
	end
	-- The quiet tile, shaped as a built-in's (its corner a quarter of its
	-- size), so they sit in a row as one.
	return ui.column {
		w = size,
		h = size,
		shrink = 0,
		rounded = ui.raw(size * 0.25, "a mark tile's radius scales with it"),
		items = "center",
		justify = "center",
		bg = look.bg,
		aria_hidden = true,
		svg,
	}
end

-- Called as the component itself (`AgentMark { … }`).
return setmetatable(M, {
	__call = function(_self: any, props: Props): Node
		return M.Mark(props)
	end,
}) :: any
