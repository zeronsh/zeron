-- A reply streaming in, parsed as the transcript parses it (ds/markdown_find
-- with the reply's key), with nothing drawn: what tests/markdown_streaming
-- checks against the whole text's parse, and tests/streaming_cost.rs times
-- a chunk at a time. Each check leaves "ok" or what went wrong in
-- `check.result`.

local Source = require("@ds/math_source")
local MarkdownFind = require("@ds/markdown_find")

-- The same numbers on every run (a seed's own sequence, not the clock's).
local function random(seed: number): (number) -> number
	local state = seed
	return function(n: number): number
		state = (state * 1103515245 + 12345) % 2147483648
		return state % n + 1
	end
end

local PIECES = {
	"word ", "words here ", "x", " ", " ", "\n", "\n", "\n\n", "\n\n", "\n\n",
	"$", "$", "$$", "$5", "$10 ", "$x^2$", "$a_{1}$ ", "\\$3", "\\", "$$\n\\frac{a}{b}\n$$",
	"`", "``", "`code`", "```", "```rust\nfn a() {}\n```\n", "~~~\nx $y$\n~~~\n", "\n```\n", "\n~~~~\n",
	"- ", "- item ", "* ", "1. ", "2. ", "> ", "    ", "  ", "# Head ", "## Sub\n",
	"| a | b |\n|---|---|\n| $c$ | d |\n", "|---|",
	"**", "*", "_", "~~", "[link](http://a.b)", "[$a](zeron-invoke:x)", "[x", "]", "(", ")",
	"![p](u.png)", "![q](v.png)", "\n![r](w.png)\n", "<!--", "-->", "<div>", "[ref]: http://r", "[ref]",
	"Word", "Zed", "é", "\t", "{", "}", "3",
}

-- A random text of markdown's pieces.
local function doc(rand: (number) -> number): string
	local out = {}
	for _ = 1, rand(70) + 5 do
		table.insert(out, PIECES[rand(#PIECES)])
	end
	return table.concat(out)
end

-- Where to cut `text` into chunks of 1 to `most` bytes.
local function cuts(rand: (number) -> number, text: string, most: number): { number }
	local at, out = 0, {}
	while at < #text do
		at = math.min(#text, at + rand(most))
		-- (On a character's end: the host's text is whole characters.)
		while at < #text and bit32.band(string.byte(text, at + 1), 0xC0) == 0x80 do
			at += 1
		end
		table.insert(out, at)
	end
	return out
end

local function alike(a: any, b: any): boolean
	if a == b then
		return true
	end
	if type(a) ~= "table" or type(b) ~= "table" then
		return false
	end
	for k, v in a do
		if not alike(v, b[k]) then
			return false
		end
	end
	for k in b do
		if a[k] == nil then
			return false
		end
	end
	return true
end

local result = ui.shared("check.result", "")
-- The seeds a check runs (a few at a time: a command has a time budget).
local seeds = ui.shared("check.seeds", { 1, 50 })

-- The math read as a text grows against the math read whole.
local function check_math(): string
	local range = seeds:peek()
	for seed = range[1], range[2] do
		local rand = random(seed)
		local text = doc(rand)
		local st = Source.scan()
		for _, at in cuts(rand, text, if seed % 3 == 0 then 3 else 24) do
			local now = string.sub(text, 1, at)
			Source.feed(st, now)
			local got, want = Source.since(st, 0), Source.prepare(now)
			if got ~= want then
				return `seed {seed} at {at}:\n{now}\n-- streamed --\n{got}\n-- whole --\n{want}`
			end
		end
	end
	return "ok"
end

-- A block, written out (what a failed check shows).
local function show(v: any): string
	if type(v) ~= "table" then
		return tostring(v)
	end
	local parts = {}
	for k, x in v do
		table.insert(parts, `{k}={show(x)}`)
	end
	table.sort(parts)
	return "{" .. table.concat(parts, ",") .. "}"
end

-- How many texts ui.markdown's own parse as it grows (the whole text under
-- one key, as the transcript parsed before) left otherwise than the whole
-- text parsed afresh: not this parse's to match, so the rest of such a text
-- isn't checked.
local own = ui.shared("check.own", 0)

-- The blocks parsed as a text grows against the text parsed whole (under a
-- key of its own each time, so its end shows as a streaming text's does,
-- open markers closed), and the math read as it grows against the math
-- read whole on the way.
local function check_parse(): string
	local fresh = 0
	local range = seeds:peek()
	for seed = range[1], range[2] do
		local rand = random(seed + 1000)
		local text = doc(rand)
		local breaks = seed % 5 == 0
		local key = `stream-check:{seed}`
		local plain = `stream-plain:{seed}`
		for _, at in cuts(rand, text, if seed % 3 == 0 then 4 else 30) do
			local now = string.sub(text, 1, at)
			local prepared = Source.prepare(now)
			local before = ui.markdown(prepared, { key = plain, breaks = breaks })
			local got = MarkdownFind.parse(now, { key = key, breaks = breaks })
			fresh += 1
			local want = MarkdownFind.together(ui.markdown(prepared, { key = `stream-whole:{fresh}`, breaks = breaks }))
			if not alike(got, want) then
				if not alike(MarkdownFind.together(before), want) then
					own:set(own:peek() + 1)
					break
				end
				for i = 1, math.max(#got, #want) do
					if not alike(got[i], want[i]) then
						return `seed {seed} at {at}, block {i}:\n{now}\n-- got --\n{show(got[i])}\n-- whole --\n{show(want[i])}`
					end
				end
			end
		end
	end
	return "ok"
end

-- A long reply's settled blocks stay the same tables as more arrives: all
-- but the last at 1,200 bytes are still there, as they were, at the end.
local function check_kept(): string
	local text = string.rep("A paragraph about $5 and $x^2$, with `code`.\n\n$$\nx\n$$\n\n| a | b |\n|---|---|\n| $c$ | d |\n\n", 40)
	local first: { MarkdownBlock } = {}
	local blocks: { MarkdownBlock } = {}
	for at = 40, #text, 40 do
		blocks = MarkdownFind.parse(string.sub(text, 1, at), { key = "stream-check:kept" })
		if at == 1200 then
			first = table.clone(blocks)
		end
	end
	if #first < 10 then
		return `only {#first} blocks at 1,200 bytes`
	end
	for i = 1, #first - 1 do
		if blocks[i] ~= first[i] then
			return `block {i} was made again`
		end
	end
	return "ok"
end

ui.command { id = "check_math", title = "Check the math read as it grows", run = function() result:set(check_math()) end }
ui.command { id = "check_parse", title = "Check the parse as it grows", run = function() result:set(check_parse()) end }
ui.command { id = "check_kept", title = "Check settled blocks are kept", run = function() result:set(check_kept()) end }

-- The reply so far (`bench.text`, the host's to set), parsed as the
-- transcript parses it.
local text = ui.shared("bench.text", "")
ui.command {
	id = "parse",
	title = "Parse the reply so far",
	run = function()
		MarkdownFind.parse(text:peek(), { key = "reply:bench" })
	end,
}

return ui.div {}
