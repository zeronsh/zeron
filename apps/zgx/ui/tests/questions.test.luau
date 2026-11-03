-- What agents ask without stopping for it (@account/questions): cards
-- pinned over the chat's composer, answered by an option or in words, or
-- dismissed; settled, a line; a "? N" pill on the session's row that
-- leaves its state alone; and every chat's open ones on the panel's home.
-- On the shell in ui/shell_fixture.luau (`questions_ask`).

local SIDE = "#shell-sidebar"

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

test("an open question is a card over the composer: its words, its context and its options", function()
	local app = open()
	show_chat(app, "Fix the build")
	expect(app:has("#chat-questions")).to_be(false)
	app:command("questions_ask")
	expect(app:wait("#chat-question-q1", { timeout_ms = 5000 })).to_be(true)
	local card = app:tree("#chat-question-q1")
	expect(card).to_contain("database")
	expect(card).to_contain("The cache is read on every request.")
	expect(app:has('#chat-question-q1 >> role:button "Postgres"')).to_be(true)
	expect(app:has('#chat-question-q1 >> role:button "SQLite"')).to_be(true)
	-- (The other chat's isn't here.)
	expect(app:has("#chat-question-q3")).to_be(false)
	-- Over the composer: its foot above the pill's top.
	expect(app:has("#chat-question-q2")).to_be(true)
end)

test("an option answers with its words, and the card folds to a settled line", function()
	local app = open()
	show_chat(app, "Fix the build")
	app:command("questions_ask")
	expect(app:wait('#chat-question-q1 >> role:button "SQLite"', { timeout_ms = 5000 })).to_be(true)
	app:click('#chat-question-q1 >> role:button "SQLite"')
	app:frames(20)
	expect(calls(app)).to_contain("answer:q1:SQLite")
	expect(app:wait("#chat-questions-settled", { timeout_ms = 5000 })).to_be(true)
	expect(app:tree("#chat-questions-settled")).to_contain("SQLite")
	-- (The open card is gone; the other stays open.)
	expect(app:has('#chat-question-q1 >> role:button "SQLite"')).to_be(false)
	expect(app:has('#chat-question-q2 >> role:textbox "Your answer"')).to_be(true)
end)

test("an answer of your own goes with Enter", function()
	local app = open()
	show_chat(app, "Fix the build")
	app:command("questions_ask")
	expect(app:wait('#chat-question-q2 >> role:textbox "Your answer"', { timeout_ms = 5000 })).to_be(true)
	-- (Two cards are taller than the stack: it scrolls, the composer under it.)
	app:scroll("#chat-questions", 600)
	app:frames(10)
	app:type('#chat-question-q2 >> role:textbox "Your answer"', "Keep it for a release")
	app:key("Enter")
	app:frames(20)
	expect(calls(app)).to_contain("answer:q2:Keep it for a release")
	-- (Never the chat's composer: its draft stays empty.)
	expect(app:text('role:textbox "Message"')).never.to_contain("Keep it")
end)

test("× dismisses it unanswered, and the settled lines can be put away", function()
	local app = open()
	show_chat(app, "Fix the build")
	app:command("questions_ask")
	expect(app:wait("#chat-question-q2", { timeout_ms = 5000 })).to_be(true)
	app:click('#chat-question-q2 >> role:button "Dismiss without answering"')
	app:frames(20)
	expect(calls(app)).to_contain("dismiss:q2")
	expect(app:wait("#chat-questions-settled", { timeout_ms = 5000 })).to_be(true)
	expect(app:tree("#chat-questions-settled")).to_contain("Dismissed")
	app:click('#chat-questions-settled >> role:button "Hide answered questions"')
	app:frames(20)
	expect(app:has("#chat-questions-settled")).to_be(false)
	-- (The open one stays.)
	expect(app:has("#chat-question-q1")).to_be(true)
end)

test("answered on another device, it folds here too", function()
	local app = open()
	show_chat(app, "Fix the build")
	app:command("questions_ask")
	expect(app:wait('#chat-question-q1 >> role:button "Postgres"', { timeout_ms = 5000 })).to_be(true)
	app:command("questions_answered_elsewhere")
	app:frames(20)
	expect(app:has('#chat-question-q1 >> role:button "Postgres"')).to_be(false)
	expect(app:tree("#chat-questions-settled")).to_contain("Postgres")
end)

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
	expect(app:wait('#chat-question-q1 >> role:button "Postgres"', { timeout_ms = 5000 })).to_be(true)
	app:click('#chat-question-q1 >> role:button "Postgres"')
	app:frames(20)
	expect(app:tree(`{SIDE} >> role:button "Fix the build"`)).to_contain("1 open question")
end)

test("the panel's home lists every chat's open questions, this one's first, and answers them there", function()
	local app = open()
	show_chat(app, "Write the docs")
	app:click('#titlebar >> role:button "Panel"')
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	expect(app:has("#panel-home-questions")).to_be(false)
	app:command("questions_ask")
	expect(app:wait("#panel-home-questions", { timeout_ms = 5000 })).to_be(true)
	local part = app:tree("#panel-home-questions")
	expect(part).to_contain("Which docs site theme?")
	expect(part).to_contain("Keep the old flag?")
	-- This chat's first.
	local mine = string.find(part, "panel-question-q3", 1, true)
	local other = string.find(part, "panel-question-q1", 1, true)
	expect(mine ~= nil and other ~= nil and mine < other).to_be(true)
	-- Each names its chat.
	expect(app:tree("#panel-question-q1")).to_contain("Fix the build")
	app:frames(60)
	app:click('#panel-question-q3 >> role:button "Dark"')
	app:frames(20)
	expect(calls(app)).to_contain("answer:q3:Dark")
	expect(app:has("#panel-question-q3")).to_be(false)
end)
