-- A heading's size and the room above it (ui/markdown_headings_fixture):
-- a chat reply's as they've always been, and a document's stepped down in
-- size only (`heading_step`), its room above each its own level's, so its
-- sections group as written.

local function boxes(app: any, column: string): { [string]: { y: number, h: number } }
	local out = {}
	for line in app:tree(column):gmatch("[^\n]+") do
		local name, y, h = line:match('"([^"]+)" [%d.]+,([%d.]+) [%d.]+x([%d.]+)')
		if name then
			out[name] = { y = tonumber(y) or 0, h = tonumber(h) or 0 }
		end
	end
	return out
end

local function mounted(): any
	local app = mount("markdown_headings_fixture.luau")
	app:resize(1000, 700)
	app:frames(10)
	return app
end

test("a chat reply's headings: their sizes and the room above them as ever", function()
	local app = mounted()
	local b = boxes(app, "#md-chat")
	-- (A heading's box holds its room above: 12 over the first two levels,
	-- 4 over the third; then its line: 30, 26, 24.)
	expect(b["One"].h).to_be(42)
	expect(b["Two"].h).to_be(38)
	expect(b["Three"].h).to_be(28)
end)

test("a document's headings step down in size, not in the room above them", function()
	local app = mounted()
	local chat, doc = boxes(app, "#md-chat"), boxes(app, "#md-doc")
	-- `#` at h2's line (26), `##` and `###` at h3's (24); each with its own
	-- level's room above, as the chat reply's.
	expect(doc["One"].h).to_be(26 + 12)
	expect(doc["Two"].h).to_be(24 + 12)
	expect(doc["Three"].h).to_be(24 + 4)
	-- (So the words before each heading end as far above it in both.)
	for _, pair in { { "Words before.", "One" }, { "Words under one.", "Two" }, { "Words under two.", "Three" } } do
		local words, heading = pair[1], pair[2]
		expect(doc[heading].y - (doc[words].y + doc[words].h)).to_be(chat[heading].y - (chat[words].y + chat[words].h))
	end
	expect(app:tree("#md-doc")).to_contain('heading "One"')
end)
