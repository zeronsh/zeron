-- A sentence and its link, one rich paragraph: words, then a link span.
-- At 248 wide the link doesn't fit after the words, so the line must break
-- at the boundary between the two spans (the words' trailing space).
-- At 300 the break falls earlier, inside the words' span (a control).

local function link(text: string): Node
	return ui.span { text, on_click = function() end, underline = true }
end

local function sentence(id: string, w: number, words: string): Node
	return ui.rich { id = id, w = w, wrap = true, text_size = 13, line_height = 16, ui.span { words }, link("Publish…") }
end

return ui.column {
	gap = 8,
	-- (Breaks at the boundary: "Publish…" moves whole to line two.)
	sentence("at-boundary", 248, "Share it in the plugin store "),
	-- (Breaks inside the first span, before "approved": the control.)
	sentence("inside", 300, "1.0.0 in the plugin store · 1.1.0 not approved "),
}
