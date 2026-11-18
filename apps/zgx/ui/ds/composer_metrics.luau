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
	width: number, editor: Element?, source: Sample?, estimate: Size?, revision: number,
	-- Which edit of the draft (`edits`), not the draft itself.
	edit: number, theme: string, mount: number,
}
export type Result = {
	-- A complete last sample, never one axis from a different request.
	sample: Sample?,
	status: "pending" | "stale" | "ready",
	revision: number,
	width: number,
}
-- What layout reads of a sample: its size alone, so a new revision of the
-- same size (an edit that wraps alike, a width that wraps alike) wakes no
-- layout.
export type Size = { unwrapped_width: number, wrapped_height: number, line_height: number }
export type Props = {
	value: State<string>,
	-- Content width at the expanded destination, not the animated viewport;
	-- with `peek`, read without following it.
	width: (peek: boolean?) -> number,
	editor: Ref,
	keep: string?,
	-- A size standing in for the editor's measure where the caller knows
	-- the exact one can't change its layout (a long draft past the cap),
	-- with the line height last measured; nil to measure. Measuring walks
	-- every line of the draft, so a long one paid that at every keystroke.
	estimate: ((line_height: number) -> Size?)?,
}

function M.use(props: Props)
	-- Keep measurements under the editor-metrics provider namespace.
	local measured: State<Sample?> = if props.keep
		then ui.state(`composer:{props.keep}:editor_text_metrics`, nil :: Sample?)
		else ui.use_state(nil :: Sample?)
	local function size_of(s: Sample?): Size?
		return if s then { unwrapped_width = s.unwrapped_width, wrapped_height = s.wrapped_height, line_height = s.line_height } else nil
	end
	local size: State<Size?> = if props.keep
		then ui.state(`composer:{props.keep}:editor_text_size`, size_of(measured:peek()))
		else ui.use_state(size_of(measured:peek()))
	local revision = if measured:peek() then (measured:peek() :: Sample).revision else 0
	-- Element userdata wrappers are recreated on reads. Follow the ref's
	-- mount generation to distinguish replacement from a width-only query.
	local mount_revision = 0
	local mounted = ui.computed(function()
		local editor = props.editor:get()
		mount_revision += 1
		return { editor = editor, revision = mount_revision }
	end)
	-- The editor, theme and line height of the last exact measure: an
	-- estimate stands in only while those hold (a theme's type changes the
	-- line height it counts by).
	local exact: { mount: number, theme: string, line_height: number }? = nil
	-- Whether a new width can change the measure: only while the widest
	-- line doesn't fit (it wraps, or would, narrower). A draft that fits, an
	-- empty one, one past the cap (estimated), measures the same at any
	-- width it fits, so a pane resized asks nothing of it at each frame.
	local follows = ui.computed(function(): boolean
		local s = size:get()
		return s == nil or s.unwrapped_width >= math.max(0, props.width()) - 1
	end)
	-- Each edit of the draft, counted: what a request follows edits by.
	-- The draft itself was copied into the request (and out again at each
	-- read of it): 2 ms of a keystroke in a draft of 30,000 lines.
	local edit_count = 0
	local edits = ui.computed(function(): number
		props.value:len()
		edit_count += 1
		return edit_count
	end)
	local last: Request? = nil
	local request = ui.computed(function(): Request
		-- Track edits even when the measured dimensions stay equal.
		local edit = edits:get()
		local width = math.max(0, if follows:get() then props.width() else props.width(true))
		local theme = ui.theme.current:get()
		local mount = mounted:get()
		local el = mount.editor
		local source, estimate = nil, nil
		if el and width > 0 then
			if props.estimate and exact and exact.mount == mount.revision and exact.theme == theme then
				estimate = props.estimate(exact.line_height)
			end
			-- Reading follows the editor's settled content/IME/style revision.
			if estimate == nil then
				source = el:text_metrics(width)
			end
		end
		-- An estimate as the last one, for the same editor, theme and width:
		-- the same request, which notifies nobody (a keystroke past the cap
		-- publishes nothing).
		local was = last
		if estimate and was and was.estimate and was.editor == el and was.mount == mount.revision
			and was.theme == theme and was.width == width
			and was.estimate.wrapped_height == estimate.wrapped_height
			and was.estimate.line_height == estimate.line_height then
			return was
		end
		revision += 1
		local now: Request = { width = width, editor = el, source = source, estimate = estimate, revision = revision, edit = edit, theme = theme, mount = mount.revision }
		last = now
		return now
	end)
	local settled: { mount: number, edit: number, theme: string, revision: number, width: number }? = nil
	local function publish(wanted: Request, source: Size, width: number)
		ui.batch(function()
			-- (An equal size notifies nobody: layout wakes only for a new one.)
			size:set {
				unwrapped_width = source.unwrapped_width,
				wrapped_height = source.wrapped_height,
				line_height = source.line_height,
			}
			measured:set {
				unwrapped_width = source.unwrapped_width,
				wrapped_height = source.wrapped_height,
				line_height = source.line_height,
				width = width,
				revision = wanted.revision,
			}
		end)
	end
	local function publish_exact(wanted: Request, source: Sample)
		settled = { mount = wanted.mount, edit = wanted.edit, theme = wanted.theme,
			revision = source.revision, width = wanted.width }
		exact = { mount = wanted.mount, theme = wanted.theme, line_height = source.line_height }
		publish(wanted, source, source.width)
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
		if wanted.estimate and editor then
			publish(wanted, wanted.estimate, wanted.width)
			return
		end
		if wanted.source == nil or editor == nil then
			return
		end
		-- A width-only query already measures settled editor content at that
		-- exact width. Publish it as one sample now, avoiding a stale/ready
		-- round-trip for every frame of a page resize. Content, IME, style,
		-- editor replacement and initial requests still need acknowledgement.
		if settled and settled.width ~= wanted.width and settled.mount == wanted.mount
			and settled.edit == wanted.edit and settled.theme == wanted.theme
			and settled.revision == wanted.source.revision then
			publish_exact(wanted, wanted.source)
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
			publish_exact(wanted, source)
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
	return { result = result, sample = size :: ReadOnlyState<Size?> }
end

return M
