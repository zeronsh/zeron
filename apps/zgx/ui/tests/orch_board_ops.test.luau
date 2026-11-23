-- What the board's account calls (account/orchestration) put on the wire,
-- on ui/orch_board_ops_fixture.luau: the `Orchestrate` op each sends, as the
-- engine reads it (crates/proto OrchestrateOp; the bridge's tests check the
-- same tables deserialize). No call sets a card's `state`: the engine works
-- its place out.

local function sent(app: any, command: string): { any }
	app:command(command)
	app:frames(3)
	return app:state("sent") or {}
end

local function same(got: any, want: any, at: string)
	if type(want) ~= "table" then
		expect(`{at} = {tostring(got)}`).to_be(`{at} = {tostring(want)}`)
		return
	end
	expect(`{at} is {type(got)}`).to_be(`{at} is table`)
	for k, v in want do
		same(got[k], v, `{at}.{k}`)
	end
	for k, _ in got do
		expect(`{at}.{k} {if want[k] == nil then "unexpected" else "expected"}`).to_be(`{at}.{k} expected`)
	end
end

local function app(): any
	local a = mount("orch_board_ops_fixture.luau")
	a:frames(3)
	expect(a:error()).to_be_nil()
	return a
end

test("board ops: each of the person's actions sends the engine's op", function()
	same(sent(app(), "actions"), {
		{ op = "closeItem", id = "it-1" },
		{ op = "closeItem", id = "it-1", dropped = true },
		{ op = "reopenItem", id = "it-1" },
		{ op = "holdItem", id = "it-1", held = true },
		{ op = "holdItem", id = "it-1", held = false },
		{ op = "seenItem", id = "it-1" },
		{ op = "pauseItem", id = "it-1" },
		{ op = "resumeItem", id = "it-1", state = "active" },
		{ op = "startItem", id = "it-1" },
		{ op = "removeItem", id = "it-1" },
		{ op = "requestChanges", id = "it-1", text = "tighten it" },
	}, "ops")
end)

test("board ops: Undo of a delete makes the card again with no state, then does what put it where it was", function()
	local a = app()
	local made = {
		op = "upsertItem",
		item = { orchestratorId = "orc-1", title = "T", detail = "D", priority = "normal", threads = { "th-1" }, links = {}, order = 3 },
	}
	same(sent(a, "restore_active"), { made }, "running")
	same(sent(a, "restore_done"), { made, { op = "closeItem", id = "it-new" } }, "done")
	same(sent(a, "restore_dropped"), { made, { op = "closeItem", id = "it-new", dropped = true } }, "dropped")
	same(sent(a, "restore_inbox"), { made, { op = "holdItem", id = "it-new", held = true } }, "held")
	same(sent(a, "restore_paused"), { made, { op = "pauseItem", id = "it-new" } }, "paused")
end)

test("board ops: New task is a dump naming the chosen orchestrator", function()
	same(sent(app(), "new_task"), {
		{ op = "dump", text = "Speed up the sidebar", chat = "bd-chat", refs = { { kind = "orchestrator", id = "orc-ci", label = "CI speed" } } },
		{ op = "dump", text = "Look at the logs", chat = "bd-chat" },
	}, "dumps")
end)
