-- The chat fixture (ui/chat_fixture.luau) with a reply streaming in at a
-- reader's pace, for measuring a real window: `stream_bench` opens a chat
-- and writes tests/streaming_cost.rs's 42 KB reply (`reply(80)`: headings,
-- prices, inline and display math, a table, code, a list, eighty times)
-- into the draft 24 bytes at a time, 40 chunks a second, as the host does
-- (`part_text`, a text block's). Run it with `zgx run --inspect`, then
-- `zgx inspect command go_s2`, `… start_block_reply` and `… stream_bench`.

local page = require("./chat_fixture")
local store = require("@app/screens/chat/store")

-- tests/streaming_cost.rs's `reply(parts)`, byte for byte.
local function reply(parts: number): string
	local out = {}
	for i = 1, parts do
		table.insert(
			out,
			`## Part {i}: what it costs\n\n`
				.. `The plan costs $5 and $10 a seat, or \\$3 with the discount; the bound is $O(n \\log n)$ for part {i}, `
				.. `and **the rest** follows from it. A [link](https://example.com/{i}) and \`inline code\` too.\n\n`
				.. `$$\n\\sum_\{k=1}^\{n} k = \\frac\{n(n+1)}\{2}\n$$\n\n`
				.. `| Item | Price | Note |\n|---|---|---|\n| seat {i} | $12 | monthly |\n| team | $99 | yearly, $x^2$ |\n\n`
				.. `\`\`\`rust\nfn part_{i}(x: u32) -> u32 \{\n    // costs $$ nothing, \`really\`\n    x * {i}\n}\n\`\`\`\n\n`
				.. `- first point, with \`a $b$ c\` and $x_{i}^2$\n- second point, about $20 even\n\n`
		)
	end
	return table.concat(out)
end

local TEXT = reply(80)
local CHUNK, EVERY_MS = 24, 25
local streaming: any = nil

ui.command {
	id = "stream_bench",
	title = "Stream a 42 KB reply at 40 chunks a second",
	run = function()
		if streaming then
			streaming()
		end
		local shown = 0
		streaming = ui.every(EVERY_MS, function(): boolean
			shown = math.min(#TEXT, shown + CHUNK)
			-- (On a character's end, as the host's text is.)
			while shown < #TEXT and bit32.band(string.byte(TEXT, shown + 1), 0xC0) == 0x80 do
				shown += 1
			end
			local now = string.sub(TEXT, 1, shown)
			store.draft:set { id = "live", text = now, part = "t", part_text = now }
			return shown < #TEXT
		end)
	end,
}

return page
