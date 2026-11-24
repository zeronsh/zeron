-- The composer's markdown as it is typed (ds/markdown_edit): the spans a
-- line is drawn in, the room it takes and the shapes beside it, in the
-- theme's sizes (zeron.composer.markdown). The draft is never rewritten:
-- every marker stays in the text. Run with `zgx test ui`.

local markdown = require("@ds/markdown_edit")
local mentions = require("@ds/mention_tokens")
local Draft = require("@ds/draft_text")

-- The look as the composer reads it, the theme set up (ui/composer_markdown_fixture).
local look: markdown.Look = mount("composer_markdown_fixture.luau"):state("composer_markdown_look")
local md = markdown.make(look)

-- A size of the look's, by the token it's read from.
local function token(name: string): number
	local by: { [string]: number? } = {
		["zeron.composer.text"] = look.text,
		["zeron.composer.line"] = look.line,
		["zeron.composer.markdown.h1.size"] = look.heading[1].size,
		["zeron.composer.markdown.h1.line"] = look.heading[1].line,
		["zeron.composer.markdown.code.size"] = look.code,
		["zeron.composer.markdown.code.inset"] = look.code_inset,
		["zeron.composer.markdown.quote.inset"] = look.quote_inset,
		["zeron.composer.markdown.quote.rail_w"] = look.quote_rail,
		["zeron.composer.markdown.rule.h"] = look.rule,
		["zeron.composer.markdown.list.marker"] = look.list_marker,
		["zeron.composer.markdown.list.indent"] = look.list_indent,
	}
	local v = by[name]
	assert(type(v) == "number", `the theme gives no {name}`)
	return v
end

-- The span over byte `at` (the first that covers it).
local function at(spans: { { any } }, i: number): any
	for _, s in spans do
		if s[1] <= i and s[2] >= i then
			return s
		end
	end
	return nil
end

test("a heading is drawn at its level's size, its #s greyed and its words bold", function()
	local spans, state = md.highlight("# Plan for today", 0)
	expect(state).to_be(0)
	local hash, word = at(spans, 1), at(spans, 4)
	expect(hash[3]).to_be("$text.tertiary")
	expect(hash.size).to_be(token("zeron.composer.markdown.h1.size"))
	expect(word.weight).to_be(600)
	expect(word.size).to_be(token("zeron.composer.markdown.h1.size"))
	expect(word.line_height).to_be(token("zeron.composer.markdown.h1.line"))
	-- Each of the three steps smaller than the last, and past the draft's.
	local h2 = at((md.highlight("## Two", 0)), 5)
	local h3 = at((md.highlight("### Three", 0)), 6)
	expect(h2.size < word.size).to_be(true)
	expect(h3.size < h2.size).to_be(true)
	expect(h3.size > token("zeron.composer.text")).to_be(true)
	-- Deeper: bold body text.
	local h4 = at((md.highlight("#### Four", 0)), 7)
	expect(h4.weight).to_be(600)
	expect(h4.size).to_be_nil()
	-- No room or shapes of its own.
	expect(md.line_insets("# Plan", 0)).to_be_nil()
	expect(md.line_marks("# Plan", 0)).to_be_nil()
end)

test("inline code is in the mono face, washed, its backticks greyed", function()
	local spans = md.highlight("run `cargo test` now", 0)
	local tick, code, after = at(spans, 5), at(spans, 7), at(spans, 18)
	expect(tick[3]).to_be("$text.tertiary")
	expect(tick.font).to_be("mono")
	expect(code.font).to_be("mono")
	expect(code.size).to_be(token("zeron.composer.markdown.code.size"))
	expect(code.bg).to_be("$zeron.code.wash")
	expect(after).to_be_nil()
	-- Bold and italic keep their faces, their markers greyed.
	local faces = md.highlight("**bold** and *it*", 0)
	expect(at(faces, 1)[3]).to_be("$text.tertiary")
	expect(at(faces, 3).weight).to_be(600)
	expect(at(faces, 15).italic).to_be(true)
end)

test("a fenced block is a washed block in mono whose lines never wrap and scroll sideways together, each block its own", function()
	local lines = {
		"```lua",
		"local wide = 'a line wider than the composer could ever show in one row'",
		"",
		"```",
		"between",
		"~~~",
		"second block",
		"~~~",
		"after",
	}
	local states: { any } = {}
	local state: any = 0
	for i, line in lines do
		states[i] = state
		local _, next = md.highlight(line, state)
		state = next
	end
	-- The fences and lines inside a block are drawn in mono, the fences greyed.
	local fence = at((md.highlight(lines[1], states[1])), 1)
	expect(fence.font).to_be("mono")
	expect(fence[3]).to_be("$text.tertiary")
	local code = at((md.highlight(lines[2], states[2])), 1)
	expect(code.font).to_be("mono")
	expect(code.size).to_be(token("zeron.composer.markdown.code.size"))
	expect(code.line_height).to_be(token("zeron.composer.line"))
	-- Inside: grouped, unwrapped, set in from the wash; the blank line too.
	for _, i in { 2, 3 } do
		local room = md.line_insets(lines[i], states[i]) :: any
		expect(room.group).to_be(states[2])
		expect(room.nowrap).to_be(true)
		expect(room.left).to_be(token("zeron.composer.markdown.code.inset"))
	end
	-- The fences set in alike, but not scrolled.
	local fence_room = md.line_insets(lines[1], states[1]) :: any
	expect(fence_room.left).to_be(token("zeron.composer.markdown.code.inset"))
	expect(fence_room.group).to_be_nil()
	-- Every line of the block, fences too: the wash edge to edge, still.
	for _, i in { 1, 2, 3, 4 } do
		local marks = md.line_marks(lines[i], states[i]) :: any
		expect(#marks).to_be(1)
		expect(marks[1].fill).to_be("$zeron.code.wash")
		expect(marks[1].w).to_be("fill")
		expect(marks[1].h).to_be("fill")
		expect(marks[1].pinned).to_be(true)
	end
	-- Prose between blocks: none of it.
	expect(markdown.inside(states[5])).to_be(false)
	expect(md.line_insets(lines[5], states[5])).to_be_nil()
	expect(md.line_marks(lines[5], states[5])).to_be_nil()
	-- The second block scrolls on its own.
	local second = md.line_insets(lines[7], states[7]) :: any
	expect(second.group).never.to_be(states[2])
	expect(markdown.inside(states[9])).to_be(false)
	-- A mention typed inside a block is code, not a token.
	local spans = mentions.highlighter(md.highlight)("see @queue.rs", states[2])
	for _, s in spans do
		expect(s[3]).never.to_be("$zeron.composer.token.trigger")
	end
end)

test("a quote is set in past its rail, its > kept and greyed, its words in the quote's ink", function()
	local line = "> what the agent said before"
	local spans = md.highlight(line, 0)
	expect(at(spans, 1)[3]).to_be("$text.tertiary")
	expect(at(spans, 5)[3]).to_be("$zeron.composer.markdown.quote.ink")
	local room = md.line_insets(line, 0) :: any
	expect(room.left).to_be(token("zeron.composer.markdown.quote.inset"))
	local marks = md.line_marks(line, 0) :: any
	expect(#marks).to_be(1)
	expect(marks[1].x).to_be(0)
	expect(marks[1].w).to_be(token("zeron.composer.markdown.quote.rail_w"))
	expect(marks[1].h).to_be("fill")
	expect(marks[1].fill).to_be("$zeron.composer.markdown.quote.rail")
end)

test("a rule is kept greyed and drawn across", function()
	local spans = md.highlight("---", 0)
	expect(spans[1][3]).to_be("$text.tertiary")
	local marks = md.line_marks("---", 0) :: any
	expect(marks[1].w).to_be("fill")
	expect(marks[1].h).to_be(token("zeron.composer.markdown.rule.h"))
	expect(marks[1].fill).to_be("$zeron.composer.markdown.rule.ink")
	-- (A list item of dashes is no rule.)
	expect(md.line_marks("- -", 0)).never.to_be_nil()
end)

test("a list item hangs from its marker: the marker drawn beside it as typed, its text where its wrapped lines start", function()
	local line = "- an item long enough that it wraps onto a second row in a narrow composer"
	local spans = md.highlight(line, 0)
	-- The marker and its space stay in the text, folded to nothing.
	local folded = at(spans, 1)
	expect(folded[2]).to_be(2)
	expect(folded.size).to_be(1)
	expect(folded.line_height).to_be(token("zeron.composer.line"))
	expect(at(spans, 3)).to_be_nil()
	-- Drawn beside it instead: the very marker, greyed, at the line's start.
	local marks = md.line_marks(line, 0) :: any
	expect(#marks).to_be(1)
	expect(marks[1].label).to_be("-")
	expect(marks[1].label_color).to_be("$text.tertiary")
	expect(marks[1].x).to_be(0)
	-- The whole line set in to where the item's text starts, so a wrapped
	-- row lines up under it.
	local room = md.line_insets(line, 0) :: any
	expect(room.left >= token("zeron.composer.markdown.list.marker")).to_be(true)
	expect(room.left).to_be(marks[1].x + marks[1].w)
	-- Nested a level: the marker a step in.
	local sub = md.line_marks("  * nested", 0) :: any
	expect(sub[1].x).to_be(token("zeron.composer.markdown.list.indent"))
	expect(sub[1].label).to_be("*")
	-- Numbered: its number as typed, in figures of one width.
	local numbered = md.line_marks("12. twelfth", 0) :: any
	expect(numbered[1].label).to_be("12.")
	expect(numbered[1].tabular).to_be(true)
	expect((md.line_insets("12. twelfth", 0) :: any).left >= room.left).to_be(true)
	-- A task's box stays in the text, greyed.
	local task = md.highlight("- [ ] write tests", 0)
	expect(at(task, 3)[3]).to_be("$text.tertiary")
	expect(at(task, 3).size).to_be_nil()
	-- Inside a fenced block a dash is code.
	expect(md.line_marks("- not a list", 1)[1].label).to_be_nil()
end)

test("the caret never rests inside a list item's folded marker", function()
	-- Anywhere in it: the item's text's start.
	expect(markdown.marker_caret("- item", 0, nil)).to_be(2)
	expect(markdown.marker_caret("- item", 1, 4)).to_be(2)
	expect(markdown.marker_caret("  1. item", 3, nil)).to_be(5)
	-- At the text, or past it, it stays.
	expect(markdown.marker_caret("- item", 2, nil)).to_be(nil)
	expect(markdown.marker_caret("- item", 5, 2)).to_be(nil)
	-- Stepped back from the text's start: the line before.
	expect(markdown.marker_caret("- item", 1, 2)).to_be(-1)
	-- Not a list, a rule: nothing.
	expect(markdown.marker_caret("plain", 0, nil)).to_be(nil)
	expect(markdown.marker_caret("- - -", 0, nil)).to_be(nil)
	-- A task's box is text: the caret may sit before it.
	expect(markdown.marker_caret("- [ ] x", 1, nil)).to_be(2)
	expect(markdown.marker_caret("- [ ] x", 2, nil)).to_be(nil)
end)

test("a draft's line and its fences are read about the caret", function()
	local head = string.rep("a line of the draft before it\n", 200)
	local text = head .. "```\n- in code\n```\n- item"
	-- A draft's state, read in place as zgx's is (`sub`, `find`, `len`).
	local s = {
		sub = function(_, i: number, j: number?): string
			return string.sub(text, i, j)
		end,
		len = function(): number
			return #text
		end,
		get = function(): string
			error("the whole draft was read")
		end,
		find = function(_, query: string): ({ { number } }, boolean)
			local out, from = {}, 1
			while true do
				local a, b = string.find(text, query, from, true)
				if not a or not b then
					return out, false
				end
				table.insert(out, { a, b })
				from = b + 1
			end
		end,
	}
	local item = #text - #"- item"
	local start, line = Draft.line_at(s, item + 1)
	expect(start).to_be(item)
	expect(line).to_be("- item")
	expect(Draft.fenced(s, item)).to_be(false)
	local code = #head + #"```\n"
	expect(Draft.fenced(s, code)).to_be(true)
	local first, top = Draft.line_at(s, 0)
	expect(first).to_be(0)
	expect(top).to_be("a line of the draft before it")
end)
