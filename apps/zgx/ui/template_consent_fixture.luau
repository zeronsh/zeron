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
	-- From the store: verified and signed; unverified, by a deploy token (no
	-- key), an update over what was changed here; and one from a link.
	store = {
		pending = "midnight-1", id = "midnight", name = "Midnight", version = "1.2.0", zeron_version = "0.3.18", key = "AbCdEfGhIjKlMnOpQrStUv", files = 12, ignored = {}, hash = "store-hash-1",
		from = { kind = "store" }, author = { kind = "verified", username = "ada" }, store = { id = "midnight", publisher = "ada", verified = true },
	},
	token = {
		pending = "midnight-2", id = "midnight", name = "Midnight", version = "1.3.0", zeron_version = "0.3.18", files = 12, ignored = {}, hash = "store-hash-2",
		from = { kind = "store" }, author = { kind = "verified", username = "ada" }, store = { id = "midnight", publisher = "ada", verified = false, verified_alternative = { version = "1.2.0" } },
		update_of = "midnight", replaces_edits = true, new_owner = true, also_installed = { { id = "midnight", from = "file" } },
	},
	changed = {
		pending = "midnight-3", id = "midnight", name = "Midnight", version = "1.3.0", zeron_version = "0.3.18", files = 12, ignored = {}, hash = "store-hash-3",
		from = { kind = "store" }, author = { kind = "verified", username = "grace" }, store = { id = "midnight", publisher = "grace", verified = false, verified_alternative = { version = "1.2.0" } },
		update_of = "midnight", replaces_edits = true, new_owner = true, store_changed = { unverified = true, publisher = true }, also_installed = { { id = "midnight", from = "file" } },
	},
	worded = {
		pending = "midnight-5", id = "midnight", name = "Midnight", version = "1.0.0-verified", zeron_version = "0.3.18", files = 12, ignored = {}, hash = "store-hash-5",
		from = { kind = "store" }, author = { kind = "verified", username = "ada" }, store = { id = "midnight", publisher = "ada", verified = false },
	},
	instead = {
		pending = "midnight-6", id = "midnight", name = "Midnight", version = "1.2.0", zeron_version = "0.3.18", key = "AbCdEfGhIjKlMnOpQrStUv", files = 12, ignored = {}, hash = "store-hash-6",
		from = { kind = "store", replaced = { version = "1.3.0", older = true } }, author = { kind = "verified", username = "ada" }, store = { id = "midnight", publisher = "ada", verified = true },
		update_of = "midnight", instead = true,
	},
	instead_unknown = {
		pending = "midnight-7", id = "midnight", name = "Midnight", version = "1.4.0", zeron_version = "0.3.18", key = "AbCdEfGhIjKlMnOpQrStUv", files = 12, ignored = {}, hash = "store-hash-7",
		from = { kind = "store", replaced = { version = "", older = false } }, author = { kind = "verified", username = "ada" }, store = { id = "midnight", publisher = "ada", verified = true },
		update_of = "midnight", instead = true,
	},
	-- (The longest name the host lets through.)
	long = {
		pending = "long-1", id = "midnight", name = string.rep("W", 64), version = "1.3.0", zeron_version = "0.3.18", files = 12, ignored = {}, hash = "long-1",
		from = { kind = "store" }, author = { kind = "verified", username = string.rep("w", 39) }, store = { id = "midnight", publisher = string.rep("w", 39), verified = false, verified_alternative = { version = "1.2.0" } },
		update_of = "midnight", replaces_edits = true, new_owner = true, store_changed = { unverified = true, publisher = true },
	},
	plain_update = {
		pending = "midnight-4", id = "midnight", name = "Midnight", version = "1.3.0", zeron_version = "0.3.18", files = 12, ignored = {}, hash = "store-hash-4",
		from = { kind = "store" }, author = { kind = "verified", username = "ada" }, store = { id = "midnight", publisher = "ada", verified = false },
		update_of = "midnight",
	},
	-- (`store`, as the store came to say of it: the same package.)
	store_said = {
		pending = "midnight-1", id = "midnight", name = "Midnight", version = "1.2.0", zeron_version = "0.3.18", key = "AbCdEfGhIjKlMnOpQrStUv", files = 12, ignored = {}, hash = "store-hash-1",
		from = { kind = "store" }, author = { kind = "verified", username = "grace" }, store = { id = "midnight", publisher = "grace", verified = false }, store_changed = { unverified = true, publisher = true },
	},
	store_verified = {
		pending = "midnight-1", id = "midnight", name = "Midnight", version = "1.2.0", zeron_version = "0.3.18", key = "AbCdEfGhIjKlMnOpQrStUv", files = 12, ignored = {}, hash = "store-hash-1",
		from = { kind = "store" }, author = { kind = "verified", username = "ada" }, store = { id = "midnight", publisher = "ada", verified = true }, store_changed = { verified = true },
	},
	link = {
		pending = "linked-1", id = "linked", name = "Linked", version = "1.0.0", key = "AbCdEfGhIjKlMnOpQrStUv", files = 3, ignored = {}, hash = "link-hash",
		from = { kind = "link", url = "https://example.com/x" }, author = { kind = "verified", username = "grace" },
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
	["templates.install_verified"] = function()
		note("install_verified")
	end,
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
for _, id in { "store", "token", "link", "instead", "instead_unknown", "worded", "long", "changed", "plain_update", "store_said", "store_verified" } do
	ui.command { id = id, title = id, run = function() show(id) end }
end
for _, name in { "en", "zh-Hans" } do
	ui.command { id = `locale_{name}`, title = name, run = function() ui.locale:set(name) end }
end
-- (Allow refused, the store's word as the engine gives it.)
for id, word in { refused_taken_down = "taken_down: It reads your keys", refused_gone = "not_in_store", refused_offline = "Zeron couldn't check this with the store. Try again." } do
	ui.command { id = id, title = id, run = function()
		shown:set({ busy = false, item = table.clone(PACKAGES.store), error = word } :: any)
	end }
end
-- (Pictures: each scheme.)
local actions = require("@screens/onboarding/actions")
for _, scheme in { "light", "dark" } do
	ui.command { id = scheme, title = scheme, run = function()
		actions.apply_scheme(scheme :: any)
	end }
end
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
