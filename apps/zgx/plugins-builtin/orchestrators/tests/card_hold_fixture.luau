-- The chat page as ui/chat_fixture.luau has it, with the open chat a
-- thread of a card the person paused: its messages held
-- (screens/chat/card_hold). Orchestrators', so `card_paused` turns it on
-- first (off until then, as on a fresh install: nothing of it loaded).
-- The board's ops are noted in `orch_calls` (`orch:<op>:<id>:<state>`) and
-- played as the engine would (resumed: into Now); `orch_refuse` refuses
-- them, as the bridge refuses an op it doesn't know.
-- For tests/chat_card_hold.test.luau.

local node = require("@app/chat_fixture")
local plugins = require("@account/plugins")
local store = require("@app/screens/chat/store")

local refuse = ui.state("orch_refuse", false)
local said: { string } = {}
local calls = ui.state("orch_calls", "")
local function note(what: string)
	table.insert(said, what)
	calls:set(table.concat(said, ","))
end

-- Its record's ops, played here.
local function play(orchestration: any)
	orchestration.op = function(op: any): any
		note(`orch:{op.op}:{op.id or ""}:{op.state or ""}`)
		if refuse:peek() then
			return { after = function(_: any, _done: any, failed: any) if failed then failed("not yours") end end }
		end
		if op.op == "resumeItem" then
			local now = table.clone(orchestration.now:peek())
			local items = {}
			for _, it in now.items or {} do
				if it.id == op.id then
					it = table.clone(it)
					it.state = op.state or "active"
					it.paused_at = nil
				end
				table.insert(items, it)
			end
			now.items = items
			orchestration.now:set(now)
		end
		return { after = function(_: any, done: (any) -> ()) done(nil) end }
	end
end

-- (Once it's loaded: a frame after it's on.)
local function when_on(f: (orch: any) -> ())
	local orch = plugins.module("orchestrators")
	if orch then
		f(orch)
	else
		ui.after(0, function()
			when_on(f)
		end)
	end
end

ui.command {
	id = "card_paused",
	title = "The open chat's card is paused",
	run = function()
		-- (The chat open: chat_fixture's session.)
		local session = store.session:peek() :: any
		local chat = session and session.id
		if not chat then
			return
		end
		plugins.set({ orchestrators = true })
		when_on(function(orch: any)
			local orchestration = orch.orchestration
			play(orchestration)
			orchestration.now:set({
				state = "ready",
				orchestrators = {},
				crew = {},
				asks = {},
				dumps = {},
				items = { { id = "it-held", title = "Fix the flaky test", state = "blocked", paused_at = os.time() - 600, threads = { chat } } },
			})
		end)
	end,
}

return node
