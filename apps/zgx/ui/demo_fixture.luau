-- One gallery example, mounted on its own for the tests of examples that
-- change in place (tests/examples_in_place.test.luau): the one named by
-- `test_demo`. Here, beside app.luau, as a mounted script loads modules
-- from its own directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()

local EXAMPLES: { [string]: string } = {
	appearance = "@demos/example_appearance",
	orgs = "@demos/example_orgs",
	members = "@demos/example_members",
	plugins = "@demos/example_plugins",
}
local which = ui.state("test_demo", "appearance")

-- (Each its own page.)
return ui.presence(which, function(name: string): Node
	return (require :: any)(EXAMPLES[name]).demo()
end, { w_full = true, h_full = true })
