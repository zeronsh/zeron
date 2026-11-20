-- Chips in a chat (ds/markdown): a file, a chat, a skill or a command is
-- drawn as a chip (its icon, then its name, on one wash) in the prompt and
-- in the reply alike; a skill by its name alone, no `$`; a web link stays a
-- link. A chip is one piece: a find in it never cuts it. On the chat page
-- (ui/chat_fixture.luau), its `chip_turn` command.

local function chat(): any
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	app:frames(30)
	app:command("chip_turn")
	app:frames(30)
	return app
end

-- How far the `nth` link labelled `label` in the transcript starts after
-- its paragraph's text (each of the turn's paragraphs starts with its
-- link): a chip's icon (14), gap and padding before its name; a link's,
-- nothing. (The prompt's first, then the reply's.)
local function lead(app: any, label: string, nth: number): number
	local text_x: number? = nil
	local seen = 0
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local tx = line:match("^%s*@%d+ text .- (%-?%d+),%-?%d+ %d+x%d+")
		if tx then
			text_x = tonumber(tx)
		end
		local lx = line:match(`^%s*@%d+ link "{label}" (%-?%d+),`)
		if lx and text_x then
			seen += 1
			if seen == nth then
				return (tonumber(lx) :: number) - text_x
			end
		end
	end
	error(`no link "{label}" #{nth}:\n{app:tree("#transcript")}`)
end

local function chip(app: any, label: string, nth: number): string
	return `{label} #{nth}: {if lead(app, label, nth) >= 17 then "chip" else "link"}`
end

test("a skill's chip shows its name alone, no `$`; a command's keeps its `/`", function()
	local app = chat()
	local tree = app:tree("#transcript")
	-- (A skill's and a command's chips open nothing: the words, not links.)
	expect(tree).to_contain('text "frontend-design and"')
	expect(tree).to_contain('text "frontend-design as well"')
	expect(tree).never.to_contain("$frontend-design")
	expect(tree).to_contain('text "/review last"')
end)

test("a file and a chat are chips in the prompt, and the same in the reply; a web link stays a link", function()
	local app = chat()
	for _, label in { "AGENTS.md", "Long chat" } do
		expect(chip(app, label, 1)).to_be(`{label} #1: chip`)
		expect(chip(app, label, 2)).to_be(`{label} #2: chip`)
	end
	expect(chip(app, "the docs", 1)).to_be("the docs #1: link")
end)

test("a reply's file chip opens the file, as its link did", function()
	local app = chat()
	local calls = app:state("calls")
	app:click('role:link "AGENTS.md"[2]')
	app:frames(10)
	expect(string.sub(app:state("calls"), #calls + 1)).to_contain("open_file:s2:AGENTS.md")
end)

test("a find in a chip's name never cuts it: still one chip", function()
	local app = chat()
	app:command("find_open")
	app:frames(20)
	app:type('role:textbox "Find"', "AGENTS")
	app:frames(30)
	-- (The prompt's and the reply's: each one link, the whole name.)
	local _, n = string.gsub(app:tree("#transcript"), 'link "AGENTS%.md"', "")
	expect(n).to_be(2)
	expect(app:tree("#transcript")).never.to_contain('link "AGENTS"')
	expect(chip(app, "AGENTS.md", 1)).to_be("AGENTS.md #1: chip")
end)

test("a reply's link to one of the agent's skills (its SKILL.md, wherever it's kept) is that skill's chip, by its name; one not listed stays a link", function()
	local app = chat()
	-- (The chat's skills read once, as the link shows.)
	expect(app:wait('~"code-review is one"', { timeout_ms = 2000 })).to_be(true)
	expect(app:state("calls")).to_contain("invocations:claude-code:s2")
	expect(app:has('role:link "the review skill"')).to_be(false)
	expect(app:tree("#transcript")).never.to_contain("$code-review")
	expect(chip(app, "that skill", 1)).to_be("that skill #1: link")
end)

test("a file outside the chat's folder stays a link", function()
	local app = chat()
	expect(chip(app, "hosts", 1)).to_be("hosts #1: link")
end)
