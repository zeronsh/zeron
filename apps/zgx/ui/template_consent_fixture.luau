-- The template consent sheet, mounted on its own with packages made up here
-- (no engine in a test host): what tests/templates_consent.test.luau
-- drives. The host's `template_consent` is set here; its one action is
-- answered as the engine would (the package installed or dropped), and
-- `ui.command`s show another package, or who signed it once looked up.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local consent = require("@screens/templates/consent")

local shown = ui.shared("template_consent", { busy = true } :: any)

local PACKAGES: { [string]: any } = {
	ledger = {
		pending = "ledger-1700000000",
		id = "ledger",
		name = "Ledger",
		description = "Dense rows, ledger lines.",
		version = "2.0.0",
		zeron_version = "0.2.131",
		key = "AbCdEfGhIjKlMnOpQrStUv",
		files = 42,
		ignored = { "screens/plugins/page.luau", "account/sessions.luau" },
		hash = "ledger-hash-1",
		source = "ledger.zeron-template",
		author = { kind = "verified", username = "ada" },
	},
	mystery = {
		pending = "mystery-1700000001",
		id = "mystery",
		name = "Mystery",
		version = "",
		key = "ZyXwVuTsRqPoNmLkJiHg",
		files = 1,
		ignored = {},
		hash = "mystery-hash-1",
		author = { kind = "unknown", reason = "Checking who made it…" },
	},
	-- A template in preview, to keep (an agent made it on this device).
	sleek = {
		keep = "sleek",
		id = "sleek",
		name = "Sleek",
		description = "",
		version = "",
		files = 7,
		hash = "sleek-digest-1",
		author = { kind = "unknown", reason = "Made on this device, by you or an agent." },
	},
}

local said: { string } = {}
local calls = ui.state("calls", "")
local function note(what: string)
	table.insert(said, what)
	calls:set(table.concat(said, ","))
end

local function show(id: string?)
	shown:set({ busy = false, item = if id then table.clone(PACKAGES[id]) else nil } :: any)
end

ui.actions {
	["templates.decide"] = function(arg: any)
		if arg.keep then
			note(`keep:{arg.keep}:{tostring(arg.allow)}:{arg.hash}`)
		else
			note(`decide:{arg.pending}:{tostring(arg.allow)}:{arg.hash}`)
		end
		show(nil)
	end,
}

ui.command { id = "ledger", title = "Show Ledger", run = function() show("ledger") end }
ui.command { id = "mystery", title = "Show Mystery", run = function() show("mystery") end }
ui.command { id = "sleek", title = "Keep Sleek", run = function() show("sleek") end }
ui.command {
	id = "found",
	title = "Mystery's signer is looked up",
	run = function()
		PACKAGES.mystery.author = { kind = "verified", username = "grace" }
		show("mystery")
	end,
}
ui.command {
	id = "loading",
	title = "Still fetching",
	run = function()
		shown:set({ busy = true } :: any)
	end,
}

show("ledger")

return ui.column {
	w = 460,
	h = 540,
	consent.View(),
}
