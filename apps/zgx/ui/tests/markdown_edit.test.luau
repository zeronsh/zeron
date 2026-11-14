-- The composer's markdown typing (ds/markdown_edit): what a line break does
-- in a list item. Run with `zgx test ui`.

local markdown = require("@ds/markdown_edit")

test("a break in a list item continues it, however long the draft above", function()
	local head = string.rep("a line of the draft before it\n", 3000)
	local text = head .. "- one"
	local from, to, put = markdown.newline(text, #text, #text)
	expect(from).to_be(#text)
	expect(to).to_be(#text)
	expect(put).to_be("\n- ")
	-- An empty item: undone, its marker gone.
	local empty = text .. "\n- "
	local f, t, p = markdown.newline(empty, #empty, #empty)
	expect(f).to_be(#text + 1)
	expect(t).to_be(#empty)
	expect(p).to_be("")
	-- A plain line, or a selection: a plain break.
	expect(markdown.newline(head .. "plain", #head + 5, #head + 5)).to_be(nil)
	expect(markdown.newline(text, #text - 1, #text)).to_be(nil)
end)

test("a break in a fenced block is plain, and the list goes on after it", function()
	local head = string.rep("a line of the draft before it\n", 3000)
	local fenced = head .. "  ```lua\n- one"
	expect(markdown.newline(fenced, #fenced, #fenced)).to_be(nil)
	local tilde = head .. "~~~\n- one"
	expect(markdown.newline(tilde, #tilde, #tilde)).to_be(nil)
	local closed = fenced .. "\nnot a ``` fence\n```\n- two"
	local _, _, put = markdown.newline(closed, #closed, #closed)
	expect(put).to_be("\n- ")
end)
