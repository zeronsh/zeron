-- A plugin or template waiting for you to allow it has a notice in the
-- window's corner, with a Review that opens its consent window, until it's
-- dealt with or put away (ds/toast, screens/plugins/waiting). Across a load
-- of the scripts, and a launch, in src/waiting_toast_tests.rs: this kit can't
-- reload. On ui/waiting_toasts_fixture.luau.

local CUA = "Cua is waiting for you to allow it"
local RANDOM = "Random Number is waiting for you to allow it"
local REVIEW = 'role:status >> role:button "Review"'

local function open(): any
	local app = mount("waiting_toasts_fixture.luau")
	app:resize(1200, 800)
	app:frames(20)
	expect(app:error()).to_be_nil()
	return app
end

local function run(app: any, command: string)
	app:command(command)
	app:frames(30)
	expect(app:error()).to_be_nil()
end

local function shows(app: any, words: string): boolean
	return app:has(`role:status >> "{words}"`)
end

local function calls(app: any): string
	return table.concat(app:state("waiting_calls") or {}, ",")
end

-- In the card that says `words`, the first node of `kind` called `name`: the
-- tree lists a card's words, then its buttons.
local function in_card(app: any, words: string, kind: string, name: string): string
	local seen = false
	for line in string.gmatch(app:tree(), "[^\n]+") do
		if string.find(line, `"{words}"`, 1, true) then
			seen = true
		elseif seen and string.find(line, ` {kind} "{name}"`, 1, true) then
			return "@" .. (string.match(line, "@(%d+)") :: string)
		end
	end
	error(`no {name} in the card saying {words}:\n{app:tree()}`)
end

-- (After a click: what it set off has had its frames.)
local function press(app: any, target: string)
	app:click(target)
	app:frames(30)
	expect(app:error()).to_be_nil()
end

test("waiting: each waiting plugin's notice has a Review that opens that plugin's window", function()
	local app = open()
	expect(app:count(REVIEW)).to_be(0)
	run(app, "waiting.two")
	expect(shows(app, CUA)).to_be(true)
	expect(shows(app, RANDOM)).to_be(true)
	expect(app:count(REVIEW)).to_be(2)
	press(app, in_card(app, CUA, "button", "Review"))
	expect(calls(app)).to_be("review:cua")
	press(app, in_card(app, RANDOM, "button", "Review"))
	expect(calls(app)).to_be("review:cua,review:random-number")
end)

test("waiting: a notice stays: past an info toast's time, and after Review, until it's decided", function()
	local app = open()
	run(app, "waiting.two")
	-- (Over twice an info toast's 4 seconds, at 60 frames a second.)
	app:frames(500)
	expect(shows(app, CUA)).to_be(true)
	expect(shows(app, RANDOM)).to_be(true)
	-- Review opens the window; nothing's decided yet.
	press(app, in_card(app, CUA, "button", "Review"))
	expect(shows(app, CUA)).to_be(true)
	-- Allowed (wherever: the host's list says so): its notice goes, the other stays.
	run(app, "waiting.allow_cua")
	expect(shows(app, CUA)).to_be(false)
	expect(shows(app, RANDOM)).to_be(true)
end)

test("waiting: declined, a notice goes; the plugin's still reviewable on its page", function()
	local app = open()
	run(app, "waiting.two")
	run(app, "waiting.decline_random")
	expect(shows(app, RANDOM)).to_be(false)
	expect(shows(app, CUA)).to_be(true)
	-- (Cua's card, and each row on the page.)
	run(app, "waiting.plugins_page")
	expect(app:count('role:button "Review"')).to_be(3)
end)

test("waiting: the × is always there, and what it puts away stays away while Plugins keeps its Review", function()
	local app = open()
	run(app, "waiting.two")
	press(app, in_card(app, CUA, "button", "Dismiss"))
	expect(shows(app, CUA)).to_be(false)
	expect(shows(app, RANDOM)).to_be(true)
	-- The list published again changes nothing.
	run(app, "waiting.two")
	expect(shows(app, CUA)).to_be(false)
	-- Still findable: its own row and Review on the Plugins page.
	run(app, "waiting.plugins_page")
	expect(app:has('"Cua"')).to_be(true)
	expect(app:count('role:button "Review"')).to_be(3)
	-- A changed one speaks again.
	run(app, "waiting.change_cua")
	expect(shows(app, CUA)).to_be(true)
end)

test("waiting: of five, the stack shows its four and the fifth waits its turn", function()
	local app = open()
	run(app, "waiting.five")
	expect(app:count(REVIEW)).to_be(4)
	-- (The newest four; the first comes as one's dealt with.)
	expect(shows(app, CUA)).to_be(false)
	run(app, "waiting.five_less")
	expect(shows(app, RANDOM)).to_be(false)
	expect(shows(app, CUA)).to_be(true)
	expect(app:count(REVIEW)).to_be(4)
	-- Every one has its row and Review on the page all the same.
	run(app, "waiting.five")
	run(app, "waiting.plugins_page")
	expect(app:count('role:button "Review"')).to_be(9)
end)

test("waiting: two plugins with one name have a notice each", function()
	local app = open()
	run(app, "waiting.twins")
	expect(app:count(REVIEW)).to_be(2)
end)

test("waiting: while the list isn't known, what's up stays up", function()
	local app = open()
	run(app, "waiting.two")
	run(app, "waiting.loading")
	expect(app:count(REVIEW)).to_be(2)
end)

test("waiting: a template waiting has its notice, whose Review opens its window, until it's decided", function()
	local app = open()
	run(app, "waiting.template")
	expect(shows(app, "Dusk is waiting for you to allow it")).to_be(true)
	expect(shows(app, "A template from someone else.")).to_be(true)
	press(app, in_card(app, "A template from someone else.", "button", "Review"))
	expect(calls(app)).to_be("template:dusk-1")
	expect(shows(app, "Dusk is waiting for you to allow it")).to_be(true)
	run(app, "waiting.template_done")
	expect(shows(app, "Dusk is waiting for you to allow it")).to_be(false)
end)

test("waiting: a template whose window opened as it came has no notice as well", function()
	local app = open()
	run(app, "waiting.template_here")
	expect(shows(app, "Dusk is waiting for you to allow it")).to_be(false)
end)

test("waiting: an action no module declared shows no toast; a function's button does what it says", function()
	local app = open()
	run(app, "waiting.unknown_action")
	expect(app:has('"Nowhere"')).to_be(false)
	run(app, "waiting.function_toast")
	press(app, in_card(app, "Branch deleted", "button", "Undo"))
	expect(calls(app)).to_be("undo")
	-- (Pressed, it's done: this one goes.)
	expect(app:has('"Branch deleted"')).to_be(false)
end)
