-- What agents ask without stopping for it (@account/questions): a line
-- over the composer says how many are open, its Answer turns the composer
-- to its question mode (ds/composer_ask, as a harness's questions are
-- asked), one at a time; answered, the next; × or Escape puts them off,
-- still open. The chat isn't waiting: the composer stays the person's to
-- type in. A "? N" pill on the session's row, its state left alone.
-- On the shell in ui/shell_fixture.luau (`questions_ask`: two on Fix the
-- build, one on Write the docs).

local SIDE = "#shell-sidebar"
local COMPOSER = "#chat-composer"

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	return app
end

local function show_chat(app: any, title: string)
	app:click(`{SIDE} >> role:button "{title}"`)
	app:frames(20)
end

local function calls(app: any): string
	return app:state("calls") or ""
end

-- Fix the build with its two questions, the composer asking them.
local function answering(app: any)
	show_chat(app, "Fix the build")
	app:command("questions_ask")
	expect(app:wait(`{COMPOSER} >> role:button "Answer"`, { timeout_ms = 5000 })).to_be(true)
	-- (The line fades in.)
	app:frames(30)
	app:click(`{COMPOSER} >> role:button "Answer"`)
	app:frames(20)
end

test("open questions are a line over the composer, which stays the person's to type in", function()
	local app = open()
	show_chat(app, "Fix the build")
	expect(app:has(`{COMPOSER} >> role:button "Answer"`)).to_be(false)
	app:command("questions_ask")
	expect(app:wait(`{COMPOSER} >> role:button "Answer"`, { timeout_ms = 5000 })).to_be(true)
	expect(app:tree(COMPOSER)).to_contain("The agent asked 2 questions")
	-- Not asking yet: no options, and the draft is a message.
	expect(app:has(`{COMPOSER} >> role:radio "Postgres"`)).to_be(false)
	expect(app:has(`{COMPOSER} >> role:textbox "Message"`)).to_be(true)
	expect(calls(app)).never.to_contain("answer:")
	-- The other chat's own count.
	show_chat(app, "Write the docs")
	expect(app:tree(COMPOSER)).to_contain("The agent asked a question")
end)

test("Answer asks them one at a time, as the agent's questions are asked", function()
	local app = open()
	answering(app)
	local tree = app:tree(COMPOSER)
	-- The first, oldest: its words (plain, without its Markdown), its
	-- context, its options, its place among them.
	expect(tree).to_contain("Which database should the cache use?")
	expect(tree).never.to_contain("**")
	expect(tree).to_contain("The cache is read on every request.")
	expect(tree).to_contain('radio "Postgres"')
	expect(tree).to_contain("1/2")
	-- The line's gone while it asks.
	expect(app:has(`{COMPOSER} >> role:button "Answer"`)).to_be(false)
	-- 2 picks the second option; Next answers and goes on.
	app:key("2")
	app:frames(10)
	app:click(`{COMPOSER} >> role:button "Next"`)
	app:frames(20)
	expect(calls(app)).to_contain("answer:q1:SQLite")
	tree = app:tree(COMPOSER)
	expect(tree).to_contain("Keep the old flag?")
	expect(tree).never.to_contain("Which database")
end)

test("an answer in words goes with Enter, and the last one hands the composer back", function()
	local app = open()
	answering(app)
	-- (The keys go to the composer: 1 picks, Enter answers.)
	app:key("1")
	app:frames(5)
	app:key("Enter")
	app:frames(20)
	expect(calls(app)).to_contain("answer:q1:Postgres")
	-- (No options: an answer of its own.)
	app:type(`{COMPOSER} >> role:textbox`, "Keep it for a release")
	app:key("Enter")
	app:frames(20)
	expect(calls(app)).to_contain("answer:q2:Keep it for a release")
	-- None left: no question, no line.
	expect(app:tree(COMPOSER)).never.to_contain("Keep the old flag?")
	expect(app:has(`{COMPOSER} >> role:button "Answer"`)).to_be(false)
end)

test("Escape puts them off, still open", function()
	local app = open()
	answering(app)
	expect(app:tree(COMPOSER)).to_contain("Which database")
	app:key("Escape")
	app:frames(20)
	expect(app:tree(COMPOSER)).never.to_contain("Which database")
	expect(calls(app)).never.to_contain("dismiss:")
	-- Still open: the line again, both still there.
	expect(app:tree(COMPOSER)).to_contain("The agent asked 2 questions")
	-- Back in: from the first again; Back from the second goes to it.
	app:click(`{COMPOSER} >> role:button "Answer"`)
	app:frames(20)
	expect(app:tree(COMPOSER)).to_contain("Which database")
end)

test("answered on another device, it leaves the composer here too", function()
	local app = open()
	answering(app)
	expect(app:tree(COMPOSER)).to_contain("Which database")
	app:command("questions_answered_elsewhere")
	app:frames(20)
	local tree = app:tree(COMPOSER)
	expect(tree).never.to_contain("Which database")
	expect(tree).to_contain("Keep the old flag?")
end)

-- The open-questions pill's count on the sidebar, from its label.
local function pill(app: any): string?
	for line in app:tree(SIDE):gmatch("[^\n]+") do
		local n = line:match('"(%d+) open questions?"')
		if n then
			return n
		end
	end
	return nil
end

test("a session's row counts its open questions in a pill of their own, its state left as it was", function()
	local app = open()
	expect(pill(app)).to_be(nil)
	local before = app:tree(`{SIDE} >> role:button "Write the docs"`)
	app:command("questions_ask")
	app:frames(20)
	-- Fix the build has two, Write the docs one.
	expect(app:tree(`{SIDE} >> role:button "Fix the build"`)).to_contain("2 open questions")
	expect(app:tree(`{SIDE} >> role:button "Write the docs"`)).to_contain("1 open question")
	-- (Write the docs was idle, and is: no waiting state.)
	expect(app:tree(`{SIDE} >> role:button "Write the docs"`)).never.to_contain("waiting")
	expect(before).never.to_contain("open question")
	-- Answered: one fewer.
	show_chat(app, "Fix the build")
	app:click(`{COMPOSER} >> role:button "Answer"`)
	app:frames(20)
	app:key("1")
	app:click(`{COMPOSER} >> role:button "Next"`)
	app:frames(20)
	expect(app:tree(`{SIDE} >> role:button "Fix the build"`)).to_contain("1 open question")
end)
