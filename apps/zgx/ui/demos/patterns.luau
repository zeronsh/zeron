-- Gallery groups of pages owned by one chat's work (patterns): a list of
-- { title, pages }, each page a { id, title, about, demo, fill? } (see
-- gallery.luau's `Page`). The gallery appends these groups to its own.
-- Put each page in a file of its own under demos/ and require it here.

return {
	{
		title = "Patterns",
		pages = {
			require("@demos/icon_button"),
			require("@demos/toggle_button"),
			require("@demos/menu"),
			require("@demos/glass"),
			require("@demos/small_pieces"),
		},
	},
}
