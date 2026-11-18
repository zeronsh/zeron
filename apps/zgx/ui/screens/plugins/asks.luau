--!strict
-- What a plugin asks for, drawn: each ask's glyph, its line in plain words
-- and the line under it, the risky ones in the danger's ink, and what it
-- names exactly as it names it (selectors, programs, hosts, its agents'
-- tools). The consent sheet's (screens/plugins/consent) and a store
-- plugin's page's (screens/plugins/listing) alike: one drawing, one
-- wording (screens/plugins/words).
--
--   asks.line(l)                 one ask (a words.Line)
--   asks.section(heading, lines) asks under a heading
--   asks.literal(list)           what it names, as written, a line each

local ds = require("@ds")
local words = require("@screens/plugins/words")

local M = {}

-- What it names, as written: each on a line of its own, in the code face.
function M.literal(list: { string }?): Node?
	if not list or #list == 0 then
		return nil
	end
	local rows: { Node } = {}
	for _, v in list do
		table.insert(rows, ui.text {
			v,
			font = "$font.mono",
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			color = "$text.secondary",
			wrap = true,
		})
	end
	return ui.column { w_full = true, gap = "$zeron.space.hair", table.unpack(rows) }
end

-- Named things with their own words: a tool's name, and what it tells
-- your agents.
function M.named(list: { words.Item }?): Node?
	if not list or #list == 0 then
		return nil
	end
	local rows: { Node } = {}
	for _, t in list do
		table.insert(rows, ui.column {
			w_full = true,
			gap = "$zeron.space.hair",
			ui.text { t.name, font = "$font.mono", text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.primary", wrap = true },
			if t.description
				then ui.text { t.description, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.secondary", wrap = true }
				else nil,
		})
	end
	return ui.column { w_full = true, gap = "$space.2", table.unpack(rows) }
end

-- One ask: its glyph, its line, the line under it, and what it names.
function M.line(l: words.Line): Node
	return ui.row {
		w_full = true,
		gap = "$space.3",
		items = "start",
		ui.div {
			pt = "$zeron.space.hair",
			color = if l.risky then "$zeron.tone.danger.ink" else "$text.secondary",
			ds.Icon { name = l.icon :: any, size = 16 },
		},
		ui.column {
			grow = 1,
			min_w = 0,
			gap = "$zeron.space.hair",
			-- (Read as it's drawn: words.Line's words follow the locale.)
			ui.text {
				function(): string
					return l.title
				end,
				text_size = "$font.size.md",
				line_height = "$font.line.md",
				color = if l.risky then "$zeron.tone.danger.ink" else "$text.primary",
				wrap = true,
			},
			if l.detail
				then ui.text {
					function(): string
						return l.detail or ""
					end,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = "$text.secondary",
					wrap = true,
				}
				else nil,
			M.literal(l.literal),
			M.named(l.items),
		},
	}
end

-- A section of asks under its heading.
function M.section(heading: string | Message, lines: { words.Line }): Node
	local list: { Node } = {}
	for _, l in lines do
		table.insert(list, M.line(l))
	end
	return ui.column {
		w_full = true,
		gap = "$space.3",
		ui.text { heading, text_size = "$font.size.sm", weight = "$font.weight.medium", color = "$text.secondary" },
		table.unpack(list),
	}
end

return M
