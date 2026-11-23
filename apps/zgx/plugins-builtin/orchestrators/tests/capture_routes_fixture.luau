-- The Control room's Capture with a dump whose parts share their words:
-- one sentence routed to two places (both its colours, both chips lighting
-- it), and two parts that overlap in part. The page alone
-- (screens/control/page), as in control_page_fixture.luau, its newest
-- dump this one.

ui.theme.strict("warn")
-- (Orchestrators on, as these are its: off is the default everywhere.)
require("@account/plugins").set({ orchestrators = true })
local ds = require("@ds")
ds.setup()
require("@app/screens/shell/route").launched:set(true)
require("./control_world")()
local orchestration = require("@orchestrators/account/orchestration")
local page = require("@orchestrators/screens/control/page")

local TEXT = "in cloud tab in settings i switched cloud machines from small to large, so you can try use them for testing and building now, should work way better.\n\ni think it's an issue that cloud machine size is not selectable in the chat page, and not selectable with zeron mcp when you create one."
local now = table.clone(orchestration.now:peek())
local dumps = table.clone(now.dumps or {})
table.insert(dumps, 1, {
	id = "dp-shared",
	text = TEXT,
	state = "routed",
	device = "dev-desk",
	created = os.time() - 30,
	routes = {
		{ id = "rt-a", excerpt = string.sub(TEXT, 1, 149), span = { from = 0, to = 149 },
			target = { kind = "orchestrator", id = "orc-orch" }, intent = "info", message = "The person moved Cloud machines from Small to Large.", state = "delivered", candidates = {} },
		{ id = "rt-b", excerpt = string.sub(TEXT, 1, 149), span = { from = 0, to = 149 },
			target = { kind = "orchestrator", id = "orc-vpn" }, intent = "info", message = "Cloud machines are Large now: use them for builds.", state = "delivered", candidates = {} },
		{ id = "rt-c", excerpt = string.sub(TEXT, 152, 231), span = { from = 151, to = 231 },
			target = { kind = "orchestrator", id = "orc-ci" }, intent = "task", message = "A machine size per chat, in the chat page.", state = "delivered", candidates = {} },
		{ id = "rt-d", excerpt = string.sub(TEXT, 201, 287), span = { from = 200, to = 287 },
			target = { kind = "orchestrator", id = "orc-vpn" }, intent = "task", message = "A machine size per chat, in Zeron's MCP create.", state = "delivered", candidates = {} },
	},
})
now.dumps = dumps
orchestration.now:set(now)

return ui.column {
	w_full = true,
	h_full = true,
	bg = "$background",
	page.Page(),
}
