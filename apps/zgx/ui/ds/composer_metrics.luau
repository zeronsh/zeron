-- The editor owns shaping, spans and IME. This adapter keeps a complete
-- last sample during initialization and tags every publication with the
-- request revision, independent of the editor's animated viewport.
local M = {}

export type Sample = {
	unwrapped_width: number,
	wrapped_height: number,
	line_height: number,
	width: number,
	revision: number,
}
export type Result = {
	-- A complete last sample, never one axis from a different request.
	sample: Sample?,
	status: "pending" | "stale" | "ready",
	revision: number,
	width: number,
}
export type Props = {
	value: State<string>,
	-- Content width at the expanded destination, not the animated viewport.
	width: () -> number,
	editor: Ref,
	keep: string?,
}

function M.use(props: Props)
	-- New namespace: a restored plain-text measurement is not editor geometry.
	local measured: State<Sample?> = if props.keep
		then ui.state(`composer:{props.keep}:editor_text_metrics`, nil :: Sample?)
		else ui.use_state(nil :: Sample?)
	local revision = if measured:peek() then (measured:peek() :: Sample).revision else 0
	local request = ui.computed(function()
		local text = props.value:get()
		local width = math.max(0, props.width())
		local theme = ui.theme.current:get()
		local el = props.editor:get()
		-- Reading follows the editor's settled content/IME/style revision.
		local source = if el and width > 0 then el:text_metrics(width) else nil
		revision += 1
		return { text = text, width = width, theme = theme, editor = el, source = source, revision = revision }
	end)
	local alive = true
	local cancel: (() -> ())? = nil
	ui.effect(function()
		local wanted = request:get()
		if cancel then
			cancel()
			cancel = nil
		end
		if wanted.source == nil then
			return
		end
		-- Defer publication until bindings and editor layout have consumed
		-- the request. A replacement effect cancels this acknowledgement.
		cancel = ui.after(0, function()
			if not alive or request:peek().revision ~= wanted.revision then
				return
			end
			local source = wanted.editor:text_metrics(wanted.width)
			if source == nil or source.revision ~= wanted.source.revision then
				return
			end
			measured:set {
				unwrapped_width = source.unwrapped_width,
				wrapped_height = source.wrapped_height,
				line_height = source.line_height,
				width = source.width,
				revision = wanted.revision,
			}
		end)
	end)
	ui.on_cleanup(function()
		alive = false
		if cancel then
			cancel()
		end
	end)
	local result = ui.computed(function(): Result
		local want, sample = request:get(), measured:get()
		return {
			sample = sample,
			status = if sample == nil then "pending" elseif sample.revision == want.revision then "ready" else "stale",
			revision = want.revision,
			width = want.width,
		}
	end)
	return { result = result }
end

return M
