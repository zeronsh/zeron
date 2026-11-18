-- Two words in a row, 4 apart: by `gap_x` as a token (zgx's own
-- `$space.1`, 4), as a number, and by `gap` as the token.

local function row(id: string, props: { [string]: any }): Node
	local p: { [any]: any } = { id = id, w = 300, ui.text { `{id}-one` }, ui.text { `{id}-two` } }
	for k, v in props do
		p[k] = v
	end
	return ui.row(p)
end

return ui.column {
	row("token", { gap_x = "$space.1" }),
	row("number", { gap_x = 4 }),
	row("gap", { gap = "$space.1" }),
}
