-- Setup's order (screens/onboarding/state + actions): where a run opens,
-- where Continue and Back go from every step, and the dots, for each kind
-- of account. Continue and Back go through every step in order; only a
-- run's start passes over pages finished before it. Never back to claiming
-- a username, naming the organization, or (signed in) Sign in. The cases
-- are in ui/onboarding_order.luau, mounted (the modules need `ui`, which only
-- a mounted app has). Run with `zgx test ui onboarding`. Nothing here
-- reaches Zeron: its calls fail in a test, and what it would answer is set
-- straight on the state.

test("setup goes through its steps in order for every kind of account", function()
	local app = mount("onboarding_order.luau")
	expect(app:text("#result")).to_be("ok")
end)
