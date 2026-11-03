-- Checkbox and radio group, and a staging list whose folder box is
-- tri-state.

local Checkbox = require("@ds/checkbox")
local RadioGroup = require("@ds/radio")
local kit = require("@demos/kit")

local a = ui.state("cb_a", true)
local b = ui.state("cb_b", false)
local c = ui.state("cb_c", true)
local strategy = ui.state("rg_strategy", "squash")
local density = ui.state("rg_density", "regular")
-- A folder's files, each staged or not.
local staged = ui.state("cb_staged", { true, false, true })
local FILES = { "ds/badge.luau", "ds/checkbox.luau", "ds/tokens.luau" }

local function all(): boolean
	for _, on in staged:get() do
		if not on then
			return false
		end
	end
	return true
end
local function some(): boolean
	local any, every = false, true
	for _, on in staged:get() do
		any = any or on
		every = every and on
	end
	return any and not every
end

return {
	id = "checkbox",
	title = "Checkbox and radio",
	about = "A box that fills with the accent as its check springs in, or a dash while what it stands for is partly on. Radios: one of a few, the arrows moving the choice.",
	demo = function()
		local files: { Node } = {}
		for i, name in FILES do
			table.insert(
				files,
				ui.div {
					pl = "$space.5",
					Checkbox {
						value = function()
							return staged:get()[i]
						end,
						on_change = function(on)
							staged:set_key(i, on)
						end,
						label = name,
					},
				}
			)
		end
		return kit.page(
			kit.block(
				"STATES",
				kit.row(
					Checkbox { value = a, label = "On" },
					Checkbox { value = b, label = "Off" },
					Checkbox {
						value = function()
							return false
						end,
						mixed = function()
							return true
						end,
						label = "Mixed",
					},
					Checkbox { value = c, label = "Disabled", disabled = true },
					Checkbox { value = b, name = "Alone" }
				)
			),
			kit.block(
				"WITH A DESCRIPTION",
				ui.column {
					w = 360,
					gap = "$space.3",
					Checkbox { value = a, label = "Sign commits", description = "With the SSH key Zeron made for this device." },
					Checkbox { value = b, label = "Push after committing", description = "To the branch's upstream, setting one if it has none." },
				}
			),
			kit.block(
				"A FOLDER, PARTLY STAGED",
				ui.column {
					gap = "$space.2",
					Checkbox {
						value = all,
						mixed = some,
						on_change = function(on)
							staged:set { on, on, on }
						end,
						label = "ds/",
					},
					table.unpack(files),
				}
			),
			kit.block(
				"RADIO GROUP",
				ui.row {
					gap = "$space.7",
					items = "start",
					flex_wrap = true,
					ui.column {
						w = 300,
						RadioGroup {
							value = strategy,
							label = "Merge strategy",
							options = {
								{ value = "merge", label = "Merge commit", description = "Every commit, joined by a merge." },
								{ value = "squash", label = "Squash and merge", description = "One commit on the base branch." },
								{ value = "rebase", label = "Rebase and merge", description = "Each commit replayed on top." },
								{ value = "ff", label = "Fast-forward only", description = "Not allowed in this repository.", disabled = true },
							},
						},
					},
					RadioGroup {
						value = density,
						label = "Density",
						direction = "row",
						options = {
							{ value = "compact", label = "Compact" },
							{ value = "regular", label = "Regular" },
							{ value = "roomy", label = "Roomy" },
						},
					},
				}
			)
		)
	end,
}
