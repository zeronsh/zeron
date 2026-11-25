--!strict
-- Where Settings → Templates is (screens/plugins/store_nav). Kept apart from
-- the page so what opens a template's page from elsewhere needn't load it.

return require("@app/screens/plugins/store_nav").make {
	view = ui.state("templates_view", "catalog"),
	tab = ui.state("templates_tab", "installed"),
	opened = ui.state("templates_view_opened", false),
	linked = ui.state("templates_view_linked", nil :: string?),
	linked_version = ui.state("templates_view_linked_version", nil :: string?),
	link_seq = ui.state("templates_view_link_seq", 0),
}
