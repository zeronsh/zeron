-- A Cursor card with a login whose key Cursor rejected beside the one in
-- use: the step tests/provider_rejected.test.luau drives. Mounted on its
-- own (no engine in a test host); what the card asks of the engine is
-- noted in `calls`.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
ds.PrivateText.hidden:set(false)

local said: { string } = {}
local calls = ui.state("calls", "")
local function note(what: string)
	table.insert(said, what)
	calls:set(table.concat(said, ","))
end

local accounts = ui.state("accounts", {
	{
		id = "dead",
		email = "ada@example.com",
		plan = "Key expires Feb 1",
		session = 0,
		week = 0,
		active = false,
		switchable = false,
		invalid = true,
		invalid_reason = "Key rejected — sign in again",
	},
	{ id = "live", email = "bo@example.com", plan = "Key expires Mar 1", session = 0.2, week = 0.4, active = true, switchable = true },
	{ id = "spare", email = "cy@example.com", plan = "Key expires Mar 2", session = 0.1, week = 0.1, active = false, switchable = true },
} :: { any })

return ds.ProviderCard.Card {
	provider = { id = "cursor", name = "Cursor", mark = "cursor" },
	accounts = function()
		return accounts:get() :: any
	end,
	foot = "idle",
	enabled = ui.state("cursor_on", true),
	active = function()
		return "bo@example.com"
	end,
	settle = function() end,
	on_switch = function(acc)
		note(`switch:{acc.id}`)
	end,
	on_remove = function(acc)
		note(`remove:{acc.id}`)
	end,
}
