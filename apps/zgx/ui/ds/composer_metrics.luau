-- The composer's measurement provider. For now the same two invisible
-- plain-text nodes measure its unwrapped width and its expanded height.
-- Keep this contract independent of those nodes so an editor-backed
-- provider can replace them without changing layout rules.
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
	left: number,
	keep: string?,
}

function M.use(props: Props)
	local measured: State<Sample?> = if props.keep
		then ui.state(`composer:{props.keep}:text_metrics`, nil :: Sample?)
		else ui.use_state(nil :: Sample?)
	local revision = if measured:peek() then (measured:peek() :: Sample).revision else 0
	local request = ui.computed(function()
		local text = props.value:get()
		local width = math.max(0, props.width())
		local theme = ui.theme.current:get()
		local size = (ui.theme.get("zeron.composer.text") :: number?) or 14
		local line = (ui.theme.get("zeron.composer.line") :: number?) or 22
		local font = ui.theme.get("font.sans")
		revision += 1
		return {
			text = text,
			width = width,
			size = size,
			line = line,
			font = font,
			theme = theme,
			revision = revision,
		}
	end)
	local line, wrapped = ui.ref(), ui.ref()
	local alive = true
	local cancel: (() -> ())? = nil
	local queue: () -> ()
	queue = function()
		if not alive then
			return
		end
		-- Timers created in an effect are cancelled when it re-runs. Always
		-- replace the handle: a retained "queued" flag could otherwise leave
		-- the provider waiting forever for a callback that was cancelled.
		if cancel then
			cancel()
		end
		local wanted = request:peek()
		-- Coalesce both resize notifications into one read of both nodes.
		-- Also sample on a request change: equal-size edits have no resize.
		cancel = ui.after(0, function()
			if not alive then
				return
			end
			if request:peek().revision ~= wanted.revision then
				queue()
				return
			end
			local a, b = line:peek(), wrapped:peek()
			if not a or not b or wanted.width <= 0 then
				return
			end
			local w, h, at = a.width, b.height, b.width
			if w == nil or h == nil or at == nil or h <= 0 or math.abs(at - wanted.width) >= 0.5 then
				return
			end
			measured:set({
				unwrapped_width = w,
				wrapped_height = h,
				line_height = wanted.line,
				width = at,
				revision = wanted.revision,
			})
		end)
	end
	ui.effect(function()
		request:get()
		queue()
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
	local nodes = {
		ui.text {
			class = "composer-metric-line",
			ref = line,
			on_resize = queue,
			absolute = true,
			left = 0,
			top = 0,
			opacity = 0,
			pointer_events = "none",
			text_size = "$zeron.composer.text",
			line_height = "$zeron.composer.line",
			function()
				return request:get().text
			end,
		},
		ui.text {
			class = "composer-metric-wrapped",
			ref = wrapped,
			on_resize = queue,
			absolute = true,
			left = props.left,
			w = function()
				return request:get().width
			end,
			top = 0,
			opacity = 0,
			pointer_events = "none",
			wrap = true,
			text_size = "$zeron.composer.text",
			line_height = "$zeron.composer.line",
			-- A final empty line still contributes to the measured height.
			function()
				return request:get().text .. "\u{200B}"
			end,
		},
	}
	return { result = result, nodes = nodes }
end

return M
