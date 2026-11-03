-- A main chat's notes (screens/panel/notes) on the panel's home and in their
-- tab, on ui/panel_home_fixture.luau, whose notes engine compares and sets
-- on the text's hash as the engine does (crates/doc notes.rs).

local function eventually(app: any, ok: () -> boolean, ms: number?): boolean
	local tries = math.ceil((ms or 15000) / 50)
	for _ = 1, tries do
		if ok() then
			return true
		end
		app:wait('"never shown, only waited past"', { timeout_ms = 50 })
	end
	return ok()
end

local function home(): any
	local app = mount("panel_home_fixture.luau")
	app:resize(1000, 900)
	app:frames(20)
	expect(app:error()).to_be_nil()
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	app:frames(10)
	return app
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

local function kinds(app: any, id: string?): string
	local s = ((app:state("panel_sessions") :: any) or {})[`chat:{id or "chat-1"}`]
	local out: { string } = {}
	for _, t in (if s then s.tabs else {}) do
		table.insert(out, t.kind)
	end
	return table.concat(out, ",")
end

local function active_kind(app: any): string?
	local s = ((app:state("panel_sessions") :: any) or {})["chat:chat-1"]
	if not s then
		return nil
	end
	for _, t in s.tabs do
		if t.id == s.active then
			return t.kind
		end
	end
	return nil
end

local EDITOR = "#panel-notes-editor"

local function open_notes(app: any)
	app:click("#panel-open-notes")
	expect(app:wait(EDITOR, { timeout_ms = 15000 })).to_be(true)
	app:frames(5)
end

local function back_home(app: any)
	app:click('role:button "New Tab"')
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	app:frames(5)
end

-- The engine's text once a write has gone out (after the quiet).
local function engine_text(app: any): string
	return tostring(app:state("notes_text") or "")
end

test("a main chat offers Notes, with N, after Fork chat", function()
	local app = home()
	expect(app:has('#panel-open-notes >> "Notes"')).to_be(true)
	expect(app:has('#panel-open-notes >> "N"')).to_be(true)
end)

test("a side chat's panel and a subagent's have no Notes, and N opens nothing there", function()
	local app = home()
	for _, id in { "side-a", "chat-1--sub--1" } do
		app:set_state("test_home_session", { id = id, cwd = "/proj" })
		app:frames(20)
		expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
		expect(app:has("#panel-open-notes")).to_be(false)
		app:click("#outside")
		app:key("n")
		app:frames(10)
		expect(kinds(app, id)).to_be("")
		expect(app:has(EDITOR)).to_be(false)
	end
end)

test("Notes opens one tab with a blank page; opening it again comes back to it", function()
	local app = home()
	open_notes(app)
	expect(kinds(app)).to_be("notes")
	expect(app:has('#panel-strip >> role:tab "Notes"')).to_be(true)
	expect(app:text(EDITOR)).to_be("")
	expect(app:has('"Notes for this session. Markdown works here."')).to_be(true)
	back_home(app)
	app:click("#panel-open-terminal")
	expect(eventually(app, function(): boolean
		return kinds(app) == "notes,terminal"
	end)).to_be(true)
	back_home(app)
	open_notes(app)
	expect(kinds(app)).to_be("notes,terminal")
	expect(active_kind(app)).to_be("notes")
end)

test("N opens the notes from the home, the keyboard in them", function()
	local app = home()
	app:click("#outside")
	app:key("n")
	expect(eventually(app, function(): boolean
		return kinds(app) == "notes" and app:has(EDITOR)
	end)).to_be(true)
	app:frames(10)
	-- (The keyboard is in the editor.)
	expect(app:tree(EDITOR)).to_contain("focused")
end)

test("typing is written after a quiet, and found again as the tab comes back", function()
	local app = home()
	open_notes(app)
	app:type(EDITOR, "- [ ] ship notes")
	expect(eventually(app, function(): boolean
		return engine_text(app) == "- [ ] ship notes"
	end)).to_be(true)
	-- (One write, after the typing stopped.)
	local _, sets = string.gsub(calls(app), "notes_set:", "")
	expect(sets).to_be(1)
	-- Closed and opened again: the same words.
	app:key("cmd+w")
	app:frames(10)
	expect(eventually(app, function(): boolean
		return kinds(app) == ""
	end)).to_be(true)
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	open_notes(app)
	expect(app:text(EDITOR)).to_be("- [ ] ship notes")
end)

test("what the engine holds shows as the notes open", function()
	local app = home()
	app:set_state("notes_text", "# Plan\n")
	app:command("notes_remote")
	app:frames(5)
	open_notes(app)
	expect(eventually(app, function(): boolean
		return app:text(EDITOR) == "From the laptop\n# Plan\n"
	end)).to_be(true)
end)

test("another computer's change lands in place, the caret staying where it was", function()
	local app = home()
	open_notes(app)
	app:type(EDITOR, "hello world")
	expect(eventually(app, function(): boolean
		return engine_text(app) == "hello world"
	end)).to_be(true)
	local a, b = app:selection(EDITOR)
	expect(a).to_be(11)
	expect(b).to_be(11)
	app:command("notes_remote")
	expect(eventually(app, function(): boolean
		return app:text(EDITOR) == "From the laptop\nhello world"
	end)).to_be(true)
	a, b = app:selection(EDITOR)
	expect(a).to_be(11 + 16)
	expect(b).to_be(11 + 16)
	-- Typing goes on.
	app:type(EDITOR, "!")
	app:frames(3)
	expect(app:text(EDITOR)).to_be("From the laptop\nhello world!")
	expect(eventually(app, function(): boolean
		return engine_text(app) == "From the laptop\nhello world!"
	end)).to_be(true)
end)

test("an edit racing another computer's keeps both", function()
	local app = home()
	open_notes(app)
	app:type(EDITOR, "one")
	expect(eventually(app, function(): boolean
		return engine_text(app) == "one"
	end)).to_be(true)
	-- Typed here, not written yet, as the other computer writes.
	app:type(EDITOR, " two")
	app:command("notes_remote")
	expect(eventually(app, function(): boolean
		return engine_text(app) == "From the laptop\none two" and app:text(EDITOR) == "From the laptop\none two"
	end)).to_be(true)
end)

test("an engine from before notes keeps them here, then hands them over once it has them", function()
	local app = home()
	app:set_state("notes_text", "synced line")
	app:command("notes_old_engine")
	app:frames(3)
	open_notes(app)
	expect(app:wait('#panel-notes-local >> "Syncs once Zeron updates"', { timeout_ms = 15000 })).to_be(true)
	app:type(EDITOR, "kept here")
	app:frames(5)
	local kept = app:state("session_notes_local") :: any
	expect(kept["chat-1"]).to_be("kept here")
	app:command("notes_new_engine")
	expect(eventually(app, function(): boolean
		return engine_text(app) == "synced line\nkept here"
	end)).to_be(true)
	expect(app:text(EDITOR)).to_be("synced line\nkept here")
	expect(app:has("#panel-notes-local")).to_be(false)
	kept = app:state("session_notes_local") :: any
	expect(kept["chat-1"]).to_be_nil()
end)

test("the merge keeps both sides, never splitting a character; the hash is the engine's", function()
	local app = home()
	app:command("notes_selftest")
	local got = app:state("test_notes_selftest") :: any
	expect(got.hash_empty).to_be("cbf29ce484222325")
	expect(got.hash_a).to_be("af63dc4c8601ec8c")
	expect(got.hash_long).to_be(got.hash_long_engine)
	expect(got.both).to_be("aXbcY")
	expect(got.one).to_be("abcY")
	expect(got.overlap).to_be("help!")
	expect(got.empty_base).to_be("theirsmine")
	expect(got.accent).to_be("0,2,è")
end)
