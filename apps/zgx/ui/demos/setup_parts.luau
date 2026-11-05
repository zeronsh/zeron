-- What setup's device steps share (Appearance, Sync, GitHub, Providers and
-- the ending): whether you're signed in, which decides how many steps
-- there are, and the switch over a page that sets it.
--
-- The steps are the real setup's (crates/ui/src/onboarding.rs `Step`):
-- Appearance, Sync, GitHub, Providers, then "You're all set". GitHub is a
-- step only when you're signed in (Cloud agents need an account), so signed
-- in there are four numbered steps, signed out three, and Providers is the
-- last of either (`Step::number`, `Step::total`). The ending isn't counted.

local ds = require("@ds")

local M = {}

-- Signed in ("in") or not ("out"), the same on every page that asks.
M.account = ui.state("ex_setup_account", "in")

function M.signed_in(): boolean
	return M.account:get() == "in"
end

-- How many numbered steps there are, as `Step::total` counts them.
function M.count(): number
	return if M.signed_in() then 4 else 3
end

-- Who is signed in, and the device setup runs on (fictional).
M.EMAIL = "mara@halcyon.dev"
M.DEVICE = "Mara's MacBook Pro"

-- The switch beside a page's State, for a page that reads differently
-- signed in and out.
function M.account_switch(): Node
	return ds.Segmented {
		value = M.account,
		label = "Account",
		options = { { value = "in", label = "Signed in" }, { value = "out", label = "Signed out" } },
	}
end

-- A step whose dots follow the account: built again when it changes, as
-- the real frame's progress is (the count is fixed when the dots are made).
function M.counted(build: (count: number) -> Node): Node
	return ui.presence(function(): string
		return M.account:get()
	end, function(key: string): Node
		return build(if key == "in" then 4 else 3)
	end, { w_full = true, h_full = true })
end

return M
