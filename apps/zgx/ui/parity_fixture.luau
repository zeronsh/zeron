-- The chat page and a side chat's tab side by side, the same width, the
-- same conversation, for tests/chat_parity.test.luau: their transcripts and
-- composers should read the same (the send's runway, the last row's room
-- above the composer, the working row, the top inset, the jump-to-latest
-- pill). Each sits under a 38-point band: the chat page under the window's
-- title bar, the side chat under the panel's tab strip. No engine: a send
-- adds the prompt and a working reply to whichever it went to.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
require("./cloud_stub")
local store = require("@app/screens/chat/store")
local route = require("@app/screens/shell/route")
local chat = require("@app/screens/chat")
local lanes = require("@app/screens/chat/lanes")
local LaneTab = require("@app/screens/chat/lane_tab")
local sessions = require("@account/sessions")

-- The width each stands in, and the band over each.
local W = 640
local BAND = 38

local NOW = os.time()
local function conversation(): { any }
	local out = {}
	for i = 1, 12 do
		table.insert(out, { id = `u{i}`, role = "user", text = `Question {i}: how does part {i} work?` })
		table.insert(out, {
			id = `a{i}`,
			role = "assistant",
			text = `Part {i} reads its input, checks it, and hands it on. It keeps a small cache so a second read is cheap, and it says why when it can't.`,
		})
	end
	return out
end

store.catalog:set {
	state = "ready",
	harnesses = { { id = "claude-code", label = "Claude", mark = "claude" } },
	models = { { value = "claude-code/opus", label = "Opus", harness = "claude-code" } },
}
sessions.now:set({
	state = "ready",
	here = "d",
	devices = { { id = "d", name = "Mac", platform = "macos", here = true } },
	projects = { { id = "p1", name = "atlas", device = "d", path = "/proj", git = true } },
	chats = { { id = "chat-1", title = "Parent", project = "p1", device = "d", branch = "main", cwd = "/proj", updated = NOW, created = NOW - 3600 } },
	archived = {},
	children = { ["chat-1"] = { { id = "side-1", parent = "chat-1", title = "Side", device = "d", updated = NOW, created = NOW - 60 } } },
	places = { pinned = {}, sections = {} },
	places_ready = true,
} :: any)

local RECORD = { state = "ready", harness = "claude-code", model = "claude-code/opus", effort = "high", access = "full", project = "p1", device = "d", branch = "main", cwd = "/proj", running = false }
local function record(id: string): any
	local r = table.clone(RECORD)
	r.id = id
	return r
end

-- The chat page's session.
local stub = store :: any
stub.open = function(id: string?)
	if id == "chat-1" and not (store.session:peek() and (store.session:peek() :: any).id == "chat-1") then
		store.messages:set(conversation())
		store.session:set(record("chat-1"))
	end
end
-- The side chat's, as the host watches it beside the open one.
local stub_lanes = lanes :: any
stub_lanes.watch = function(slot: string, id: string?)
	if id then
		local v = lanes.view(slot)
		v.messages:set(conversation())
		v.session:set(record(id))
	end
end

-- A send: the prompt, and a reply at work under it, where it went.
local function settled(): any
	return {
		after = function(_self: any, ok: ((any) -> ())?)
			if ok then
				ok(nil)
			end
		end,
	}
end
stub.send = function(req: any): any
	local list = if req.session == "chat-1" then store.messages else lanes.view("tab1").messages
	-- (As the host: the sender's own prompt row kept, by its id.)
	local now = table.clone(list:peek())
	local n = #now
	local here = false
	for i, m in now do
		if req.message_id and m.id == req.message_id then
			here = true
			-- (The host's copy: delivered.)
			local delivered = table.clone(m)
			delivered.waiting = nil
			now[i] = delivered
		end
	end
	if not here then
		table.insert(now, { id = req.message_id or `p{n}`, role = "user", text = req.text })
	end
	table.insert(now, { id = `w{n}`, role = "assistant", text = "", waiting = "working", since = ui.now() / 1000 })
	list:set(now)
	return settled()
end
stub.interrupt = function() end
stub.load_catalog = function() end
stub.catalog_on = function() end
stub.configure = function() end
stub.refs = function(): any
	return {
		after = function(_self: any, ok: ((any) -> ())?)
			if ok then
				ok({ { name = "main", current = true } })
			end
		end,
	}
end

route.open_session("chat-1")

return ui.row {
	w_full = true,
	h_full = true,
	bg = "$zeron.window",
	-- The chat page: its session, and the window's composer over it.
	ui.overlay {
		id = "parity-page",
		w = W,
		h_full = true,
		shrink = 0,
		ui.column {
			w_full = true,
			h_full = true,
			pt = ui.raw(BAND, "the title bar's height"),
			chat.Session("chat-1"),
		},
		chat.Composer(),
	},
	-- A side chat's tab, under the panel's strip.
	ui.column {
		id = "parity-lane",
		w = W,
		h_full = true,
		shrink = 0,
		pt = ui.raw(BAND, "the panel's tab strip"),
		LaneTab.Tab { tab = { id = "tab1", kind = "side_chat", chat = "side-1", title = "Side" } },
	},
}
