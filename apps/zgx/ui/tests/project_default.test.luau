-- A project's default tile is its repository's: two copies of one, on any
-- devices, get the same hue and look for artwork in the same order; a
-- `local:` repository and a plain folder keep to their own. Pure
-- (screens/shell/project_default.luau); the tile drawn is tested below on the
-- shell in ui/shell_fixture.luau.

local default = require("../screens/shell/project_default")

local HUES = { violet = true, blue = true, emerald = true, sky = true, amber = true, rose = true, lime = true, teal = true, orange = true, indigo = true, pink = true, cyan = true }

-- Copies whose own ids give different hues: else the test proves nothing.
local function apart(): (string, string)
	for i = 1, 100 do
		local a, b = `laptop-{i}`, `desk-{i}`
		if default.hue_of(a) ~= default.hue_of(b) then
			return a, b
		end
	end
	error("no two ids apart")
end

test("project default: two copies of a repository get the same hue, whatever their ids", function()
	local a, b = apart()
	local one = { id = a, repository = "commit:abc", created = 200 }
	local two = { id = b, repository = "commit:abc", created = 100 }
	expect(default.hue(one)).to_be(default.hue(two))
	expect(default.hue(one)).to_be(default.hue_of("commit:abc"))
	expect(HUES[default.hue(one)]).to_be(true)
	expect(default.key(one)).to_be("repo:commit:abc")
	expect(default.key(two)).to_be(default.key(one))
end)

test("project default: a local repository and a folder keep their own hue, as before", function()
	local a, b = apart()
	local one = { id = a, repository = "local:abc" }
	local two = { id = b, repository = "local:abc" }
	expect(default.hue(one)).to_be(default.hue_of(a))
	expect(default.hue(two)).to_be(default.hue_of(b))
	expect(default.hue(one) == default.hue(two)).to_be(false)
	expect(default.key(one)).to_be(a)
	local folder = { id = b }
	expect(default.hue(folder)).to_be(default.hue_of(b))
	expect(default.key({ id = b, repository = "" })).to_be(b)
end)

test("project default: artwork is looked for in the repository's copies, oldest first, the same from any copy", function()
	local all = {
		{ id = "c", repository = "commit:abc", created = 300 },
		{ id = "b", repository = "commit:abc", created = 100 },
		{ id = "x", repository = "commit:other", created = 50 },
		{ id = "a", repository = "commit:abc", created = 100 },
		{ id = "n", repository = "commit:abc" },
		{ id = "l", repository = "local:abc", created = 1 },
	}
	local function ids(list: { any }): string
		local out = {}
		for _, q in list do
			table.insert(out, q.id)
		end
		return table.concat(out, ",")
	end
	expect(ids(default.copies(all[1], all))).to_be("a,b,c,n")
	expect(ids(default.copies(all[4], all))).to_be("a,b,c,n")
	expect(ids(default.copies(all[6], all))).to_be("l")
	expect(ids(default.copies({ id = "f" }, all))).to_be("f")
end)

-- The colour its editor ticks as its own, opened on a copy.
local DIALOG = `role:dialog "Edit project"`
local function own_colour(app: any, command: string): string?
	app:command(command)
	app:frames(40)
	local ticked: string? = nil
	for line in app:tree(DIALOG):gmatch("[^\n]+") do
		local label = line:match(' radio "([^"]+)"')
		if label and line:find(" checked", 1, true) and label ~= "Default" and label ~= "Emoji" and label ~= "Image" then
			ticked = label
		end
	end
	app:key("Escape")
	app:frames(20)
	return ticked
end

test("project default: both copies of a repository wear the same colour by default, on any device", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("atlas_cloned_on_box")
	-- (Its dialog placed, as the sidebar does once an editor is first opened.)
	app:set_state("screens/shell/sidebar.luau::shell_project_editor_used", true)
	app:frames(20)
	local here = own_colour(app, "fixture.edit_project_atlas")
	local there = own_colour(app, "fixture.edit_project_atlas_clone")
	expect(here).never.to_be_nil()
	expect(there).to_be(here)
	expect(app:error()).to_be_nil()
end)
