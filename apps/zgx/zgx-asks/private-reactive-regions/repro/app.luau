-- A private column (`hook = false`), as a Settings page is, holding a part
-- built bare and parts built by each kind of reactive region.
local key = ui.state("key", "a")
local shown = ui.state("shown", true)
local rows = ui.state("rows", { "r1" })
return ui.column {
	w_full = true,
	hook = false,
	ui.text { id = "bare", "bare" },
	ui.presence(function(): string
		return key:get()
	end, function(_k: string): Node
		return ui.column { ui.text { id = "in-presence", "in a presence" } }
	end),
	ui.show(function(): boolean
		return shown:get()
	end, ui.column { ui.text { id = "in-show", "in a show" } }),
	ui.each(rows, function(r: string): Node
		return ui.column { ui.text { id = `in-each-{r}`, "in an each" } }
	end, { key = function(r: string): string
		return r
	end }),
}
