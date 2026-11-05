-- The composer's mentions (ds/mention_tokens, the words' side of
-- ds/composer_mentions): the token the caret is in, and what a pick puts
-- in its place. Run with `zgx test ui`.

local mentions = require("@ds/mention_tokens")

local FILES: { mentions.Item } = {
	{ value = "crates/ui/src/queue.rs", label = "queue.rs", file = true },
	{ value = "crates/engine/tests/message_queue.rs", label = "message_queue.rs", file = true },
	{ value = "apps/zgx/ui/ds/init.luau", label = "init.luau", file = true },
	{ value = "apps/zgx/ui/screens/onboarding/init.luau", label = "init.luau", file = true },
}
local COMMANDS: { mentions.Item } = {
	{ value = "commit", label = "/commit" },
	{ value = "compact", label = "/compact" },
}
local SKILLS: { mentions.Item } = {
	{ value = "code-review", label = "Code review" },
	{ value = "release", label = "Release" },
}
local SOURCES: mentions.Sources = { ["@"] = FILES, ["/"] = COMMANDS, ["$"] = SKILLS }
local MESSAGE_QUEUE = FILES[2]

-- A pick with the caret at `caret` (bytes from 0) in `text`: the text
-- after it.
local function picked(text: string, caret: number, item: mentions.Item): string
	local from, to, replacement = mentions.pick(text, caret, item, SOURCES)
	assert(from and to and replacement, "no token at the caret")
	return string.sub(text, 1, from) .. replacement .. string.sub(text, to + 1)
end

test("the token runs from its trigger to its end, past the caret", function()
	local text = "Compare @queue.rs with"
	-- The caret after "@que": the query is what was typed, the token all of it.
	local t = mentions.token(text, 12)
	expect(t).to_be_truthy()
	assert(t, "no token at the caret")
	expect(t.trigger).to_be("@")
	expect(t.query).to_be("que")
	expect(t.from).to_be(8)
	expect(t.to).to_be(17)
end)

test("a pick replaces the whole token, caret at its end, middle or start", function()
	local text = "Compare @queue.rs with"
	expect(picked(text, 17, MESSAGE_QUEUE)).to_be("Compare @message_queue.rs with")
	expect(picked(text, 12, MESSAGE_QUEUE)).to_be("Compare @message_queue.rs with")
	expect(picked(text, 9, MESSAGE_QUEUE)).to_be("Compare @message_queue.rs with")
end)

test("a pick adds one space, never a second", function()
	expect(picked("Read @que", 9, MESSAGE_QUEUE)).to_be("Read @message_queue.rs ")
	expect(picked("Read @que then", 9, MESSAGE_QUEUE)).to_be("Read @message_queue.rs then")
end)

test("two tokens side by side: only the one the caret is in changes", function()
	local text = "@queue.rs @init.luau"
	expect(picked(text, 4, MESSAGE_QUEUE)).to_be("@message_queue.rs @init.luau")
	-- The last token, at the draft's end: a space follows the pick.
	expect(picked(text, 14, FILES[1])).to_be("@queue.rs @queue.rs ")
end)

test("commands and skills replace to their end too", function()
	expect(picked("then $code-review it", 9, SKILLS[2])).to_be("then $release it")
	expect(picked("and /commit now", 8, COMMANDS[2])).to_be("and /compact now")
	-- A command's run stops at a slash: `/co` inside a path is no command.
	expect(mentions.token("see /usr/bin", 12)).to_be_nil()
	expect(mentions.token("costs $12", 7)).to_be_nil()
end)

test("a file is named by the shortest end of its path that is its own", function()
	expect(mentions.short(FILES, FILES[1])).to_be("queue.rs")
	expect(mentions.short(FILES, FILES[2])).to_be("message_queue.rs")
	expect(mentions.short(FILES, FILES[3])).to_be("ds/init.luau")
	expect(mentions.short(FILES, FILES[4])).to_be("onboarding/init.luau")
end)

test("the draft's highlight styles a token in type alone", function()
	local spans = mentions.highlight("Read @queue.rs and /commit", 0)
	local glyphs, names = 0, 0
	-- A span is `{ from, to, color }` with its faces as fields too.
	for _, span in spans do
		local s = span :: { [number]: any, weight: number?, bg: string? }
		if s[3] == "$zeron.composer.token.trigger" then
			glyphs += 1
		elseif s.weight == 500 then
			names += 1
			expect(s.bg).to_be_nil()
		end
	end
	expect(glyphs).to_be(2)
	expect(names).to_be(2)
end)
