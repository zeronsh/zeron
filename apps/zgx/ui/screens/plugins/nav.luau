--!strict
-- Where Settings → Plugins is (screens/plugins/store_nav). Kept apart from
-- the page so what opens a plugin's page from elsewhere (a
-- `zeron://open/plugin/<id>` link, the shell's route) needn't load it.

return require("@app/screens/plugins/store_nav").make {
	view = ui.state("plugins_view", "catalog"),
	tab = ui.state("plugins_tab", "installed"),
	opened = ui.state("plugins_view_opened", false),
	linked = ui.state("plugins_view_linked", nil :: string?),
	linked_version = ui.state("plugins_view_linked_version", nil :: string?),
	link_seq = ui.state("plugins_view_link_seq", 0),
}
