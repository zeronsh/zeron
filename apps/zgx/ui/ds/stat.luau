-- A headline figure over a short label, plain type with no tile: the
-- figure large and semibold in tabular figures (a number changing in place
-- doesn't jitter), a change note beside it on its baseline, the label
-- under it in the secondary ink. A tone marks the change, not the figure;
-- a figure with no change takes the tone itself (the shipping app's
-- rule). Each prop may be a function, followed as it changes.
--
--   Stat { value = "1,284", label = "Requests", delta = "+12%", tone = "$success" }

export type Live<T> = T | () -> T
export type Props = {
	value: Live<string>,
	label: Live<string>?,
	delta: Live<string?>?,
	-- A colour (a token) for the change, or the figure with no change.
	tone: Live<any>?,
	size: string?,
	line: string?,
}

local function read<T>(v: Live<T>?): T?
	if type(v) == "function" then
		return (v :: any)()
	end
	return v :: any
end

return function(props: Props): Node
	local function delta(): string
		local d = read(props.delta)
		return if type(d) == "string" then d else ""
	end
	local function label(): string
		local l = read(props.label)
		return if type(l) == "string" then l else ""
	end
	return ui.column {
		class = "stat",
		gap = "$zeron.space.hair",
		min_w = 0,
		ui.row {
			items = "baseline",
			gap = "$zeron.space.tight",
			min_w = 0,
			ui.text {
				function(): string
					return read(props.value) or ""
				end,
				literal = true,
				tabular = true,
				truncate = true,
				text_size = props.size or "$zeron.visual.text.xxl.size",
				line_height = props.line or "$zeron.visual.text.xxl.line",
				weight = "$font.weight.semibold",
				letter_spacing = -0.3,
				color = function(): any
					local tone = read(props.tone)
					return if tone and delta() == "" then tone else "$text.primary"
				end,
			},
			ui.show(
				function(): boolean
					return delta() ~= ""
				end,
				ui.text {
					delta,
					literal = true,
					tabular = true,
					text_size = "$zeron.visual.text.sm.size",
					line_height = "$zeron.visual.text.sm.line",
					weight = "$font.weight.medium",
					color = function(): any
						return read(props.tone) or "$text.secondary"
					end,
				}
			),
		},
		ui.show(
			function(): boolean
				return label() ~= ""
			end,
			ui.text {
				label,
				literal = true,
				truncate = true,
				text_size = "$zeron.visual.text.sm.size",
				line_height = "$zeron.visual.text.sm.line",
				color = "$text.secondary",
			}
		),
	}
end
