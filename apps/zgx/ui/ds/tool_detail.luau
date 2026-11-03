-- What a tool row opens to under its call, as Zeron's transcript does
-- (crates/ui transcript.rs `tool_detail`, `blob_detail` and the chip's
-- affordance): an edit's diff, or each file's lines added and removed; a
-- command's output, a read's or a search's result.
--
-- The chat keeps little of a tool: a short summary of its output (at most
-- 24 lines show, then "… N more lines"), each file's counts, and refs to
-- the whole output and the whole diff, which come only when asked for. A
-- line under the detail asks: "Show full diff", "Show full output (12 KB)";
-- while it comes "Loading full output…"; failed, "Couldn't load full output
-- — tap to retry". Fetched, the whole of it shows (a diff up to 600 lines,
-- an output up to 400) in the detail's place; with both fetched, the one
-- asked for last shows and the line offers the other. What was fetched is
-- kept by its ref (a reload, the row scrolled back, nothing fetched again).
--
-- A diff is the git pages' (ds/git): a hunk's header, then its
-- lines, each with its old and new numbers and its sign, added lines on the
-- success's wash and removed on the danger's, in mono.
--
--   ToolDetail { tool = function() return tool end }

local locale = require("@self/locale")

local M = {}

export type Line = { kind: string, old: number?, new: number?, text: string }
export type Diff = { kind: "diff", path: string?, lines: { Line }, added: number?, removed: number?, more: number? }
export type Output = { kind: "output", lines: { string }, more: number? }
export type Stat = { path: string, added: number, removed: number }
-- What of a tool the detail reads (ds/activity's Tool has these).
export type Tool = {
	id: string,
	output: string?,
	output_ref: string?,
	output_bytes: number?,
	diff_ref: string?,
	stats: { Stat }?,
	diff: Diff?,
}
type Blob = { state: "loading" | "ready" | "failed", value: any?, order: number? }

-- The summary's lines that show before "… N more lines" (the shipping
-- app's OUTPUT_DETAIL_MAX_LINES).
local OUTPUT_LINES = 24
-- A diff line's height, and its gutter: two numbers and the sign.
local LINE = 20
local NUM, SIGN = 36, 16

-- What was fetched, by ref, and the order things were asked for.
local BLOBS = ui.state("tool_blobs", {} :: { [string]: Blob })
local asked = 0

-- How a ref is fetched: the host's action (a test or the gallery brings its
-- own).
M.fetch = function(blob_ref: string): any
	return ui.call("chat.tool_blob", { ref = blob_ref })
end

local function put(blob_ref: string, blob: Blob)
	local now = table.clone(BLOBS:peek())
	now[blob_ref] = blob
	BLOBS:set(now)
end

-- Fetches `blob_ref` (again, if it failed); fetched already, it just comes
-- to the front.
local function request(blob_ref: string)
	asked += 1
	local was = BLOBS:peek()[blob_ref]
	if was and was.state == "ready" then
		put(blob_ref, { state = "ready", value = was.value, order = asked })
		return
	end
	if was and was.state == "loading" then
		return
	end
	local order = asked
	put(blob_ref, { state = "loading", order = order })
	local ok, promise = pcall(M.fetch, blob_ref)
	if not ok or not promise then
		put(blob_ref, { state = "failed", order = order })
		return
	end
	promise:after(function(value: any)
		put(blob_ref, if type(value) == "table" then { state = "ready", value = value, order = order } else { state = "failed", order = order })
	end, function()
		put(blob_ref, { state = "failed", order = order })
	end)
end

local function kb(bytes: number): string
	return if bytes < 1024 then `{bytes} B` else `{math.ceil(bytes / 1024)} KB`
end

-- Which of the tool's fetched refs shows (the one asked for last).
local function shown_ref(t: Tool, blobs: { [string]: Blob }): string?
	local best: string? = nil
	local best_order = -1
	for _, r in { t.diff_ref, t.output_ref } do
		local b = r and blobs[r]
		if r and b and b.state == "ready" and (b.order or 0) > best_order then
			best, best_order = r, b.order or 0
		end
	end
	return best
end

-- An output's summary as the detail shows it.
local function summary(text: string): Output?
	local lines: { string } = {}
	for line in string.gmatch(text .. "\n", "([^\n]*)\n") do
		table.insert(lines, line)
	end
	while #lines > 0 and string.match(lines[#lines], "^%s*$") do
		table.remove(lines)
	end
	if #lines == 0 then
		return nil
	end
	local more = math.max(0, #lines - OUTPUT_LINES)
	while #lines > OUTPUT_LINES do
		table.remove(lines)
	end
	return { kind = "output", lines = lines, more = more }
end

-- "… N more lines", under what was cut.
local function more_line(n: number?): Node?
	if not n or n <= 0 then
		return nil
	end
	return ui.text {
		function(): string
			return locale.s("ds.tool_detail.more_lines", { n = n })
		end,
		literal = true,
		px = "$space.3",
		pb = "$space.2",
		font = "$font.mono",
		text_size = "$font.size.xs",
		color = "$text.tertiary",
	}
end

-- An output: its lines in mono on the code's wash, cut (not wrapped) at
-- the block's edge, as the shipping app's.
-- (`o` a function where the output follows its tool: a summary.)
local function output_view(o: Output | () -> Output): Node
	local function get(): Output
		return if type(o) == "function" then o() else o
	end
	return ui.column {
		w_full = true,
		rounded = "$radius.md",
		bg = "$zeron.code.wash",
		overflow = "hidden",
		class = "tool-output",
		ui.text {
			function(): string
				return table.concat(get().lines, "\n")
			end,
			literal = true,
			px = "$space.3",
			py = "$space.2",
			font = "$font.mono",
			text_size = "$font.size.xs",
			line_height = "$font.line.xs",
			color = "$text.secondary",
		},
		ui.show(function(): boolean
			return (get().more or 0) > 0
		end, ui.text {
			function(): string
				local n = get().more or 0
				return locale.s("ds.tool_detail.more_lines", { n = n })
			end,
			literal = true,
			px = "$space.3",
			pb = "$space.2",
			font = "$font.mono",
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		}),
	}
end

-- A diff's line, the gallery's look.
local function diff_line(l: Line): Node
	if l.kind == "hunk" then
		return ui.text {
			l.text,
			literal = true,
			h = LINE,
			px = "$space.3",
			font = "$font.mono",
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		}
	end
	local wash = if l.kind == "+" then "$zeron.tone.success.wash" elseif l.kind == "-" then "$zeron.tone.danger.wash" else nil
	local ink = if l.kind == "+" then "$zeron.diff.added" elseif l.kind == "-" then "$zeron.diff.removed" else "$text.tertiary"
	local function num(n: number?): Node
		return ui.text {
			if n then tostring(n) else "",
			w = NUM,
			shrink = 0,
			align = "right",
			font = "$font.mono",
			text_size = "$font.size.xs",
			color = "$text.tertiary",
			tabular = true,
		}
	end
	return ui.row {
		w_full = true,
		h = LINE,
		items = "center",
		bg = wash,
		num(l.old),
		num(l.new),
		ui.text { l.kind, w = SIGN, shrink = 0, align = "center", font = "$font.mono", text_size = "$font.size.sm", color = ink },
		ui.text {
			(string.gsub(l.text, "\t", "    ")),
			literal = true,
			shrink = 0,
			pr = "$space.3",
			font = "$font.mono",
			text_size = "$font.size.xs",
			color = "$text.primary",
		},
	}
end

local function diff_view(d: Diff): Node
	local rows: { Node } = {}
	for _, l in d.lines do
		table.insert(rows, diff_line(l))
	end
	return ui.column {
		w_full = true,
		rounded = "$radius.md",
		bg = "$zeron.code.wash",
		overflow = "hidden",
		py = "$space.1",
		class = "tool-diff",
		label = if d.path then locale.t("ds.tool_detail.diff_of", { path = d.path }) else locale.t("ds.tool_detail.diff_of_file"),
		ui.column { w_full = true, table.unpack(rows) },
		more_line(d.more),
	}
end

-- Each file's lines added and removed (an edit as the chat keeps it).
local function stats_view(stats: { Stat }): Node
	local rows: { Node } = {}
	for _, s in stats do
		table.insert(rows, ui.row {
			gap = "$space.2",
			items = "center",
			h = 24,
			ui.text { s.path, literal = true, truncate = true, font = "$font.mono", text_size = "$font.size.xs", color = "$text.primary", min_w = 0 },
			ui.text { `+{s.added}`, shrink = 0, tabular = true, font = "$font.mono", text_size = "$font.size.xs", color = "$success" },
			ui.text { `−{s.removed}`, shrink = 0, tabular = true, font = "$font.mono", text_size = "$font.size.xs", color = "$danger" },
		})
	end
	return ui.column {
		w_full = true,
		px = "$space.3",
		py = "$space.1",
		rounded = "$radius.md",
		bg = "$zeron.code.wash",
		class = "tool-stats",
		table.unpack(rows),
	}
end

local function view_of(v: any): Node
	if v.kind == "diff" then
		return diff_view(v)
	end
	return output_view(v)
end

M.ToolDetail = ui.component(function(props: { tool: () -> Tool }): Node
	local function now(): Tool
		return props.tool()
	end
	-- What shows: the fetched one asked for last, else what the chat keeps
	-- (an older doc's diff, then the files' counts, then the summary), as a
	-- kind and the value, so the view is made again only when it changes.
	local shown = ui.computed(function(): any
		local t = now()
		local blobs = BLOBS:get()
		local r = shown_ref(t, blobs)
		if r then
			return { key = `blob:{r}`, value = blobs[r].value }
		end
		if t.diff then
			return { key = "diff", value = t.diff }
		end
		if t.stats and #t.stats > 0 then
			return { key = "stats", value = t.stats }
		end
		if t.output and summary(t.output) then
			return { key = "output" }
		end
		return { key = "none" }
	end)
	-- The line that asks for more: the diff first (the richer), then the
	-- output; one already showing gives its place to the other.
	local function affordance(): (string?, string?)
		local t = now()
		local blobs = BLOBS:get()
		local showing = shown_ref(t, blobs)
		for _, c in { { r = t.diff_ref, what = "diff", bytes = nil }, { r = t.output_ref, what = "output", bytes = t.output_bytes } } do
			local r = c.r
			if not r then
				continue
			end
			local b = blobs[r]
			if b and b.state == "ready" then
				if showing == r then
					continue
				end
				return r, locale.s("ds.tool_detail.show_full", { what = c.what })
			elseif b and b.state == "loading" then
				return r, locale.s("ds.tool_detail.loading_full", { what = c.what })
			elseif b and b.state == "failed" then
				return r, locale.s("ds.tool_detail.failed_full", { what = c.what })
			end
			return r,
				if c.bytes
					then locale.s("ds.tool_detail.show_full_size", { what = c.what, size = kb(c.bytes) })
					else locale.s("ds.tool_detail.show_full", { what = c.what })
		end
		return nil, nil
	end
	return ui.column {
		w_full = true,
		gap = "$space.1",
		class = "tool-detail",
		ui.presence(function(): string
			return shown:get().key
		end, function(key: string): Node
			local s = shown:peek()
			if key == "none" then
				return ui.div {}
			elseif key == "stats" then
				return stats_view(s.value)
			elseif key == "output" then
				-- (The summary follows its tool, in place.)
				return output_view(function(): Output
					local t = now()
					return (if t.output then summary(t.output) else nil) or { kind = "output", lines = {} }
				end)
			end
			return view_of(s.value)
		end),
		ui.show(
			function(): boolean
				local r = affordance()
				return r ~= nil
			end,
			ui.button {
				label = function(): string
					local _, label = affordance()
					return label or ""
				end,
				self = "start",
				h = 24,
				px = "$space.1",
				rounded = "$radius.md",
				bg = "#00000000",
				color = "$text.tertiary",
				hover = { bg = "$hover", color = "$text.primary" },
				active = { bg = "$pressed", color = "$text.primary" },
				cursor = "pointer",
				on_click = function()
					local r = affordance()
					if r then
						request(r)
					end
				end,
				ui.text {
					function(): string
						local _, label = affordance()
						return label or ""
					end,
					text_size = "$font.size.xs",
				},
			}
		),
	}
end)

-- Whether a tool has anything for its row to open to beyond its call.
function M.has(t: Tool): boolean
	return t.output ~= nil or t.diff ~= nil or t.output_ref ~= nil or t.diff_ref ~= nil or (t.stats ~= nil and #t.stats > 0)
end

-- Forgets what was fetched (a test's fresh start).
function M.forget()
	BLOBS:set({})
end

return M
