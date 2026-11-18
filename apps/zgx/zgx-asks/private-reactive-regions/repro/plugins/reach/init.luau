-- Replaces, and adds after, each part of the private column.
for _, id in { "bare", "in-presence", "in-show", "in-each-r1" } do
	ui.contribute { target = `#{id}`, replace = ui.text { id = `replaced-{id}`, "the plugin's" } }
	ui.contribute { target = `#{id}`, after = ui.text { id = `added-{id}`, "the plugin's" } }
end
