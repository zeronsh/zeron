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
type Request = {
	width: number, editor: Element?, source: Sample?, revision: number,
	text: string, theme: string, mount: number,
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
	-- Keep measurements under the editor-metrics provider namespace.
	local measured: State<Sample?> = if props.keep
		then ui.state(`composer:{props.keep}:editor_text_metrics`, nil :: Sample?)
		else ui.use_state(nil :: Sample?)
	local revision = if measured:peek() then (measured:peek() :: Sample).revision else 0
	-- Element userdata wrappers are recreated on reads. Follow the ref's
	-- mount generation to distinguish replacement from a width-only query.
	local mount_revision = 0
	local mounted = ui.computed(function()
		local editor = props.editor:get()
		mount_revision += 1
		return { editor = editor, revision = mount_revision }
	end)
	local request = ui.computed(function(): Request
		-- Track edits even when the measured dimensions stay equal.
		local text = props.value:get()
		local width = math.max(0, props.width())
		local theme = ui.theme.current:get()
		local mount = mounted:get()
		local el = mount.editor
		-- Reading follows the editor's settled content/IME/style revision.
		local source = if el and width > 0 then el:text_metrics(width) else nil
		revision += 1
		return { width = width, editor = el, source = source, revision = revision, text = text, theme = theme, mount = mount.revision }
	end)
	local settled: { mount: number, text: string, theme: string, revision: number, width: number }? = nil
	local function publish(wanted: Request, source: Sample)
		settled = { mount = wanted.mount, text = wanted.text, theme = wanted.theme,
			revision = source.revision, width = wanted.width }
		measured:set {
			unwrapped_width = source.unwrapped_width,
			wrapped_height = source.wrapped_height,
			line_height = source.line_height,
			width = source.width,
			revision = wanted.revision,
		}
	end
	local alive = true
	local cancel: (() -> ())? = nil
	ui.effect(function()
		local wanted = request:get()
		if cancel then
			cancel()
			cancel = nil
		end
		local editor = wanted.editor
		if wanted.source == nil or editor == nil then
			return
		end
		-- A width-only query already measures settled editor content at that
		-- exact width. Publish it as one sample now, avoiding a stale/ready
		-- round-trip for every frame of a page resize. Content, IME, style,
		-- editor replacement and initial requests still need acknowledgement.
		if settled and settled.width ~= wanted.width and settled.mount == wanted.mount
			and settled.text == wanted.text and settled.theme == wanted.theme
			and settled.revision == wanted.source.revision then
			publish(wanted, wanted.source)
			return
		end
		-- Defer publication until bindings and editor layout have consumed
		-- the request. A replacement effect cancels this acknowledgement.
		cancel = ui.after(0, function()
			if not alive or request:peek().revision ~= wanted.revision then
				return
			end
			local source = editor:text_metrics(wanted.width)
			if source == nil or source.revision ~= wanted.source.revision then
				return
			end
			publish(wanted, source)
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
	-- Geometry follows measured dimensions; request/readiness changes have
	-- their own subscribers and do not invalidate the last complete sample.
	return { result = result, sample = measured :: ReadOnlyState<Sample?> }
end

return M
