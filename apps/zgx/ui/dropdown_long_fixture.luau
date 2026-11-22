-- Long dropdowns, for tests/dropdown_long.test.luau and for measuring what
-- opening one and typing in its search cost: the composer's branch chip
-- over `test_long_n` branch-like refs (a repository's thousands), and a
-- plain Dropdown over as many items, some with a description, a hint or a
-- tag. Here, beside app.luau, as a mounted script loads modules from its
-- own directory down.

ui.theme.strict("warn")
local ds = require("@ds")
local Dropdown = require("@ds/dropdown")
ds.setup()

local n = ui.state("test_long_n", 2000)

-- A repository's refs: local branches, then remote ones, names as people
-- give them.
local WHO = { "wing", "gael", "alex", "dependabot", "release", "fix", "feat", "zeron" }
local WHAT = { "dropdown", "composer", "engine", "relay", "cloud", "sidebar", "transcript", "perf", "scroll", "billing" }
local function refs(count: number): { Dropdown.Item }
	local out: { Dropdown.Item } = {}
	for i = 1, count do
		local name = `{WHO[(i % #WHO) + 1]}/{WHAT[((i * 7) % #WHAT) + 1]}-{i}`
		if i > count // 4 then
			name = `origin/{name}`
		end
		table.insert(out, {
			value = name,
			label = name,
			icon = "git-branch",
			tag = if i == 1 then "current" elseif i % 97 == 0 then "worktree" else nil,
		})
	end
	return out
end
local branches = ui.computed(function(): { Dropdown.Item }
	return refs(n:get())
end)

-- Every few a description (a taller row), a hint or a tag.
local items = ui.computed(function(): { Dropdown.Item }
	local out: { Dropdown.Item } = {}
	for i = 1, n:get() do
		table.insert(out, {
			value = `item-{i}`,
			label = `Item {i}`,
			description = if i % 5 == 0 then `What item {i} does, said in a line` else nil,
			hint = if i % 7 == 0 then `hint {i}` else nil,
			tag = if i % 11 == 0 then "You" else nil,
		})
	end
	return out
end)

return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.4",
	gap = "$space.4",
	ds.Dropdown {
		label = "Long",
		value = ui.state("test_long_value", "item-3" :: string?),
		items = items,
		search = "Search items…",
		open_with = ui.state("test_long_open", 0),
		width = 280,
		actions = { { label = "New item", icon = "plus", on_click = function() end } },
	},
	-- A few, no search (as the composer's access); and a Select's five.
	ui.row {
		gap = "$space.4",
		ds.Dropdown {
			label = "Few",
			value = ui.state("test_long_few", "b" :: string?),
			search = false,
			items = {
				{ value = "a", label = "Ask first", icon = "shield-check", description = "Asks before edits and commands" },
				{ value = "b", label = "Edits", icon = "pen", description = "Edits on its own, asks before commands" },
				{ value = "c", label = "Full", icon = "lock-keyhole-unlocked", description = "Asks nothing" },
			},
		},
		ds.Select {
			name = "Small select",
			value = ui.state("test_long_select", "two"),
			options = {
				{ value = "one", label = "One" },
				{ value = "two", label = "Two" },
				{ value = "three", label = "Three" },
				{ value = "four", label = "Four" },
				{ value = "five", label = "Five" },
			},
			w = 180,
		},
	},
	ui.div { grow = 1 },
	ds.Composer {
		value = ui.state("test_long_draft", ""),
		model = ui.state("test_long_model", "sonnet" :: string?),
		effort = ui.state("test_long_effort", "medium" :: string?),
		harnesses = { { id = "claude", label = "Claude", mark = "claude" } },
		models = { { value = "sonnet", label = "Sonnet", harness = "claude" } },
		branch = ui.state("test_long_branch", "gael/perf-1" :: string?),
		branches = branches,
	},
}
