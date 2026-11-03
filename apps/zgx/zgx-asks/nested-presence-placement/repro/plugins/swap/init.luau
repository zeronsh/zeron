-- Replaces each slot with its own words.
for _, id in { "slot-bare", "slot-root", "slot-child", "slot-nested-root", "slot-nested-child" } do
	ui.contribute {
		target = `#{id}`,
		replace = ui.text { id = `swapped-{id}`, "the plugin's" },
	}
end
