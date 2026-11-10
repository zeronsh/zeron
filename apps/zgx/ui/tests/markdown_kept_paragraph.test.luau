-- A reply's paragraph being written stays one paragraph (ds/markdown's
-- `kept`): each chunk makes its runs again in place (zgx's `ui.rich`
-- `spans`), so its node, and the lines its text has laid out, are kept
-- rather than its row built again every chunk. What it shows is what the
-- same text shown whole shows.

local PARAGRAPH = "Words arriving as a reply streams in, alpha beta gamma delta, with a [link](https://example.com/1), "
	.. "a [path](src/chat.rs) the page may draw as a chip, `inline code` and **bold words** going on for a while."

local MIXED = "## A heading\n\n"
	.. PARAGRAPH
	.. "\n\nSecond paragraph, with *italics* and ~~struck~~ words.\n\n"
	.. "- a list item\n- another, with `code`\n\n"
	.. "```lua\nlocal x = 1\nprint(x)\n```\n\n"
	.. "| a | b |\n| --- | --- |\n| 1 | 2 |\n\n"
	.. "The last paragraph, its end."

-- A chat open with a reply starting in blocks (`reduced`: motion reduced, so
-- nothing fades and nothing is cut where a fade starts).
local function reply(reduced: boolean): any
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:frames(2)
	if reduced then
		app:set_state("motion_reduce", true)
	end
	app:command("go_s2")
	app:frames(30)
	app:command("start_block_reply")
	app:frames(5)
	return app
end

local function write(app: any, text: string)
	app:set_state("chat.draft", { id = "live", text = text, part = "t", part_text = text })
	app:frames(1)
end

-- Its node's number in the tree, nil if it isn't there.
local function node_of(app: any, target: string): string?
	if not app:has(target) then
		return nil
	end
	return string.match(app:tree(target), "^%s*@(%d+)")
end

-- The transcript's tree, its node numbers left out.
local function shape(app: any): string
	local tree = string.gsub(app:tree(), "@%d+ ", "")
	local from = string.find(tree, "#transcript scroll", 1, true) or 1
	local to = string.find(tree, "#transcript-rail", 1, true) or #tree
	return string.sub(tree, from, to)
end

test("a paragraph being written keeps its node, chunk by chunk", function()
	local app = reply(false)
	local first: string? = nil
	for n = 24, #PARAGRAPH + 23, 24 do
		write(app, string.sub(PARAGRAPH, 1, math.min(n, #PARAGRAPH)))
		expect(app:error()).to_be(nil)
		local now = node_of(app, '~"Words arriving"')
		if now then
			first = first or now
			-- (Through a link arriving, its target not yet known, and inline code
			-- not yet closed.)
			expect(now).to_be(first)
		end
	end
	expect(first).never.to_be(nil)
	app:frames(30)
	local text = app:text('~"Words arriving"') or ""
	expect(text).to_contain("going on for a while.")
	expect(text).to_contain("inline code")
	expect(node_of(app, '~"Words arriving"')).to_be(first)
end)

test("a streamed reply's tree is the tree of the same text shown whole", function()
	local streamed = reply(true)
	for n = 24, #MIXED + 23, 24 do
		write(streamed, string.sub(MIXED, 1, math.min(n, #MIXED)))
		expect(streamed:error()).to_be(nil)
	end
	streamed:frames(10)
	local whole = reply(true)
	write(whole, MIXED)
	whole:frames(10)
	expect(whole:error()).to_be(nil)
	expect(shape(streamed)).to_be(shape(whole))
	expect(shape(whole)).to_contain('link "link"')
end)

test("a paragraph's words fade in as they come and settle once faded", function()
	local app = reply(false)
	local text = "A sentence with a [link](https://example.com/1) arriving"
	-- The link's text comes in two chunks: two runs, each fading as it came.
	write(app, "A sentence with a [li")
	write(app, "A sentence with a [link](https://example.com/1)")
	expect(app:count("role:link")).to_be(2)
	-- Once both have faded, the next chunk makes the paragraph again with the
	-- link one run, as the whole text shown at once is.
	app:wait("#nothing-by-this-name", { timeout_ms = 700 })
	local id = node_of(app, '~"A sentence with"')
	write(app, text)
	expect(app:error()).to_be(nil)
	expect(app:count("role:link")).to_be(1)
	expect(app:has('role:link "link"')).to_be(true)
	expect(node_of(app, '~"A sentence with"')).to_be(id)
	expect(app:text('~"A sentence with"')).to_contain("arriving")
end)

-- As the host writes a reply now: the draft names the block and which of
-- its words states holds the block's words (`words`), and each chunk grows
-- that state alone (src/chat.rs `write_words`), read by the view in Rust.
local function write_words(app: any, text: string)
	app:set_state("chat.draft", { id = "live", part = "t", words = 1 })
	app:set_state("chat.draft.words.1", text)
	app:frames(1)
end

test("a paragraph written through the host's words state keeps its node", function()
	local app = reply(false)
	local first: string? = nil
	for n = 24, #PARAGRAPH + 23, 24 do
		write_words(app, string.sub(PARAGRAPH, 1, math.min(n, #PARAGRAPH)))
		expect(app:error()).to_be(nil)
		local now = node_of(app, '~"Words arriving"')
		if now then
			first = first or now
			expect(now).to_be(first)
		end
	end
	expect(first).never.to_be(nil)
	app:frames(30)
	local text = app:text('~"Words arriving"') or ""
	expect(text).to_contain("going on for a while.")
end)

test("a reply streamed through the host's words, then done, is the same text shown whole", function()
	local streamed = reply(true)
	for n = 24, #MIXED + 23, 24 do
		write_words(streamed, string.sub(MIXED, 1, math.min(n, #MIXED)))
		expect(streamed:error()).to_be(nil)
	end
	streamed:frames(10)
	local whole = reply(true)
	write(whole, MIXED)
	whole:frames(10)
	expect(shape(streamed)).to_be(shape(whole))
	-- Done: its row holds the words, the draft lets them go; drawn the same.
	streamed:set_state("chat.messages", (function()
		local list = table.clone(streamed:state("chat.messages"))
		local last = table.clone(list[#list])
		last.streaming = false
		last.text = MIXED
		last.blocks = { { id = "t", kind = "text", text = MIXED } }
		list[#list] = last
		return list
	end)())
	streamed:set_state("chat.draft", { id = "", text = "" })
	streamed:frames(10)
	expect(streamed:error()).to_be(nil)
	-- (Its words: the turn's "Working" line under them is the turn's.)
	local function words(app: any): string
		return string.match(shape(app), "^(.-)\"Working\"") or shape(app)
	end
	expect(words(streamed)).to_be(words(whole))
end)
