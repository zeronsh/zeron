-- What a tool row opens to under its call, as Zeron's transcript does
-- (crates/ui transcript.rs `tool_detail`, `blob_detail` and the chip's
-- affordance): an edit's diff, or each file's lines added and removed; a
-- command's output, a read's or a search's result.
--
-- An output is shown as a terminal would leave it: its escapes (colours,
-- cursor moves, a title) taken out, a line a carriage return rewrote as its
-- last write, tabs to their stops, in mono on the code's wash, each line
-- whole (a long one scrolls sideways, as a code block's does). Its first
-- ten lines show, fading at their foot, with "Show all N lines" under them
-- (the thought's button, ds/fold_button), "Show fewer lines" once open; the
-- fold is kept by the tool's id. A command that failed says its exit code
-- under its output, the block's edge in the danger's tone. A huge output
-- keeps its last 32 KB, 400 lines at most, "… earlier output trimmed" over
-- them. While the command runs, what it has written so far shows, and
-- grows in place.
--
-- A command's output the chat cut to its last ten lines (`output_truncated`)
-- shows them at once, its exit code under them, and "Show all N lines" (N
-- the whole output's, `output_lines`): that asks the device that ran it for
-- the whole output (`chat.tool_output`, by the doc it is in, `chat`),
-- "Loading all N lines…" while it comes, then shows it as any output, kept
-- by the tool's id; "Show fewer lines" folds back to the ten. When it can't
-- come, a quiet line under the output says why (that device offline, too
-- old, or it didn't keep the output) and the button asks again.
--
-- The chat keeps little of a tool: its output as the harness gave it (or a
-- summary of it), each file's counts, and refs to the whole output and the
-- whole diff, which come only when asked for. The whole output comes as
-- the row opens; a diff, from a line under the detail: "Show full diff",
-- while it comes "Loading full diff…", failed "Couldn't load full diff —
-- tap to retry". Fetched, the whole of it shows (a diff up to 600 lines, an
-- output up to 400) in the detail's place; with both fetched, the one asked
-- for last shows and the line offers the other. What was fetched is kept by
-- its ref (a reload, the row scrolled back, nothing fetched again).
--
-- A diff is the git pages' (ds/git): a hunk's header, then its
-- lines, each with its old and new numbers and its sign, added lines on the
-- success's wash and removed on the danger's, in mono.
--
--   ToolDetail { tool = function() return tool end }

local locale = require("@self/locale")
local FoldButton = require("@self/fold_button")

local M = {}

export type Line = { kind: string, old: number?, new: number?, text: string }
export type Diff = { kind: "diff", path: string?, lines: { Line }, added: number?, removed: number?, more: number? }
export type Output = { kind: "output", lines: { string }, more: number? }
export type Stat = { path: string, added: number, removed: number }
-- What of a tool the detail reads (ds/activity's Tool has these).
export type Tool = {
	id: string,
	output: string?,
	exit_code: number?,
	-- `output` is the end of a longer command output, of `output_lines`
	-- lines, whose whole the host of doc `chat` serves.
	output_truncated: boolean?,
	output_lines: number?,
	chat: string?,
	failed: boolean?,
	output_ref: string?,
	output_bytes: number?,
	diff_ref: string?,
	stats: { Stat }?,
	diff: Diff?,
}
-- An output as it shows: its lines, whether earlier ones were let go, and
-- the exit code its text led with (Claude Code's "Exit code 1").
export type Clean = { lines: { string }, earlier: boolean, exit_code: number? }
-- (`error`: a failed whole output's word, from `chat.tool_output`.)
type Blob = { state: "loading" | "ready" | "failed", value: any?, order: number?, error: string? }

-- The lines an output shows folded, before "Show all N lines".
M.FOLD_LINES = 10
-- What of an output is kept, from its end: bytes, then lines; and the
-- characters kept of one line.
M.KEEP_BYTES = 32 * 1024
M.KEEP_LINES = 400
local LINE_CHARS = 2000
-- A tab's stops, as a terminal's.
local TAB = 8
-- The folded lines' fade, in lines, as a thought's.
local FADE_LINES = 1.5
-- A diff line's height, and its gutter: two numbers and the sign.
local LINE = 20
local NUM, SIGN = 36, 16

-- What was fetched, by ref, and the order things were asked for.
local BLOBS = ui.state("tool_blobs", {} :: { [string]: Blob })
-- Outputs the reader opened past their fold, by tool id.
local OUTPUT_OPEN = ui.state("tool_output_open", {} :: { [string]: boolean })
local asked = 0

-- How a ref is fetched: the host's action (a test or the gallery brings its
-- own).
M.fetch = function(blob_ref: string): any
	return ui.call("chat.tool_blob", { ref = blob_ref })
end

-- How a command's whole output is read from the device that ran it, by the
-- doc it is in and its id (a test brings its own).
M.fetch_output = function(chat: string, id: string): any
	return ui.call("chat.tool_output", { chat = chat, id = id })
end

local function put(blob_ref: string, blob: Blob)
	local now = table.clone(BLOBS:peek())
	now[blob_ref] = blob
	BLOBS:set(now)
end

-- A failure's word: an error's message, or what it is.
local function word(err: any): string
	return if type(err) == "table" and err.message then tostring(err.message) else tostring(err)
end

-- Fetches `blob_ref` (again, if it failed) through `fetch` (the sidecar's,
-- by default); fetched already, it just comes to the front.
local function request(blob_ref: string, fetch: (() -> any)?)
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
	local ok, promise = pcall(fetch or function(): any
		return M.fetch(blob_ref)
	end)
	if not ok or not promise then
		put(blob_ref, { state = "failed", order = order, error = if ok then nil else word(promise) })
		return
	end
	promise:after(function(value: any)
		put(blob_ref, if type(value) == "table" then { state = "ready", value = value, order = order } else { state = "failed", order = order })
	end, function(err: any)
		put(blob_ref, { state = "failed", order = order, error = word(err) })
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

-- The whole output comes as the row opens, for a tool with nothing else to
-- show (a command, a read): an edit's diff stays a click away.
local function fetches_output(t: Tool): boolean
	return t.output_ref ~= nil and t.diff_ref == nil and t.diff == nil and not (t.stats and #t.stats > 0)
end

-- The end of `text`: its last KEEP_BYTES, from a line's start (or, in one
-- long line, a character's), and whether anything was let go.
local function tail(text: string): (string, boolean)
	if #text <= M.KEEP_BYTES then
		return text, false
	end
	local s = string.sub(text, #text - M.KEEP_BYTES + 1)
	local nl = string.find(s, "\n", 1, true)
	if nl and nl < #s then
		return string.sub(s, nl + 1), true
	end
	local i = 1
	while i <= #s and bit32.band(string.byte(s, i), 0xC0) == 0x80 do
		i += 1
	end
	return string.sub(s, i), true
end

-- A terminal's escapes out of `s`: OSC (a title, a link) to its BEL or ST,
-- CSI (colours, cursor moves, erasing), a charset's pick, any other
-- two-byte escape, then a stray ESC.
local function strip_escapes(s: string): string
	if not string.find(s, "\27", 1, true) then
		return s
	end
	s = string.gsub(s, "\27%][^\7\27]*\7", "")
	s = string.gsub(s, "\27%][^\7\27]*\27\\", "")
	s = string.gsub(s, "\27%[[\48-\63]*[\32-\47]*[\64-\126]", "")
	s = string.gsub(s, "\27[%(%)][%w]", "")
	s = string.gsub(s, "\27[\64-\95]", "")
	s = string.gsub(s, "\27", "")
	return s
end

local function width(s: string): number
	return utf8.len(s) or #s
end

local function expand_tabs(line: string): string
	if not string.find(line, "\t", 1, true) then
		return line
	end
	local parts = string.split(line, "\t")
	local out = parts[1]
	local col = width(out)
	for i = 2, #parts do
		local pad = TAB - col % TAB
		out ..= string.rep(" ", pad) .. parts[i]
		col += pad + width(parts[i])
	end
	return out
end

local function cut(line: string): string
	if #line <= LINE_CHARS then
		return line
	end
	local ok, at = pcall(utf8.offset, line, LINE_CHARS + 1)
	if ok and at then
		return string.sub(line, 1, at - 1) .. "…"
	end
	return line
end

local function blank(line: string): boolean
	return string.match(line, "^%s*$") ~= nil
end

-- An output as it shows (see the top): its escapes out, carriage returns
-- and tabs as a terminal leaves them, its blank lines at either end gone,
-- an "Exit code N" it leads with taken out as its code, and only its end
-- kept when it's huge.
function M.clean(text: string): Clean
	local kept, earlier = tail(text)
	kept = strip_escapes(kept)
	local lines: { string } = {}
	for _, raw in string.split(kept, "\n") do
		local l = raw
		if string.find(l, "\r", 1, true) then
			-- (What a carriage return wrote over: the last write shows.)
			local parts = string.split(l, "\r")
			l = ""
			for i = #parts, 1, -1 do
				if parts[i] ~= "" then
					l = parts[i]
					break
				end
			end
		end
		l = (string.gsub(l, "[\0-\8\11\12\14-\31\127]", ""))
		table.insert(lines, cut(expand_tabs(l)))
	end
	while #lines > 0 and blank(lines[#lines]) do
		table.remove(lines)
	end
	local first = 1
	while first <= #lines and blank(lines[first]) do
		first += 1
	end
	local exit_code: number? = nil
	if not earlier and lines[first] then
		local code = string.match(lines[first], "^Exit code (%-?%d+)%s*$")
		if code then
			exit_code = tonumber(code)
			first += 1
			while first <= #lines and blank(lines[first]) do
				first += 1
			end
		end
	end
	local out = table.move(lines, first, #lines, 1, {})
	if #out > M.KEEP_LINES then
		out = table.move(out, #out - M.KEEP_LINES + 1, #out, 1, {})
		earlier = true
	end
	return { lines = out, earlier = earlier, exit_code = exit_code }
end

-- What an output view shows: the output's text, how many lines past it
-- weren't sent (a fetched output's cut), and whether its host let earlier
-- output go.
type Source = { text: string, more: number, earlier: boolean? }

-- Where a command's whole output is kept here once read from its host.
local function whole_key(t: Tool): string?
	if t.output_truncated and t.chat then
		return `output:{t.chat}/{t.id}`
	end
	return nil
end

-- Why a whole output didn't come, for the line under it.
local WHY = {
	offline = "ds.tool_detail.whole_offline",
	update = "ds.tool_detail.whole_update",
	unavailable = "ds.tool_detail.whole_unavailable",
	running = "ds.tool_detail.whole_running",
}

local function mono_note(text: () -> string, color: string): Node
	return ui.text {
		text,
		literal = true,
		px = "$space.3",
		font = "$font.mono",
		text_size = "$font.size.xs",
		line_height = "$font.line.xs",
		color = color,
	}
end

-- An output (see the top). `id`: the tool's, keeping its fold; `source`
-- follows the tool, so a running command's output grows in place.
local function output_view(id: string, source: () -> Source, tool: () -> Tool): Node
	local cleaned = ui.computed(function(): Clean
		return M.clean(source().text)
	end)
	local line = (ui.theme.get("font.line.xs") :: number?) or 14
	-- The whole output, on the device that ran it (its key here), when
	-- the chat kept only its end.
	local function whole(): string?
		return whole_key(tool())
	end
	local function blob(): Blob?
		local k = whole()
		return if k then BLOBS:get()[k] else nil
	end
	local function is_open(): boolean
		if OUTPUT_OPEN:get()[id] ~= true then
			return false
		end
		-- (Asked for from its host: open once it came.)
		local b = blob()
		return whole() == nil or (b ~= nil and b.state == "ready")
	end
	local function long(): boolean
		return #cleaned:get().lines > M.FOLD_LINES or whole() ~= nil
	end
	local function folded(): boolean
		return long() and not is_open()
	end
	-- The lines "Show all" counts: the whole output's, when the chat kept
	-- its end.
	local function all_lines(): number
		local t = tool()
		if whole() and t.output_lines and not is_open() then
			return t.output_lines
		end
		return #cleaned:get().lines
	end
	local function loading(): boolean
		local b = blob()
		return OUTPUT_OPEN:get()[id] == true and b ~= nil and b.state == "loading"
	end
	-- Why the whole output didn't come, or nil.
	local function trouble(): string?
		local b = blob()
		if not b or b.state ~= "failed" then
			return nil
		end
		return locale.s(WHY[b.error or ""] or "ds.tool_detail.whole_failed")
	end
	local function text(): string
		local c = cleaned:get()
		if folded() then
			return table.concat(c.lines, "\n", 1, M.FOLD_LINES)
		end
		return table.concat(c.lines, "\n")
	end
	local function code(): number?
		local c = tool().exit_code or cleaned:get().exit_code
		return if c and c ~= 0 then c else nil
	end
	local function failed(): boolean
		return tool().failed == true or code() ~= nil
	end
	local function more(): number
		return if folded() then 0 else source().more
	end
	local function label(): string
		if loading() then
			return locale.s("ds.tool_detail.loading_all_lines", { n = tostring(all_lines()) })
		end
		return if is_open()
			then locale.s("ds.activity.show_fewer_lines")
			else locale.s("ds.activity.show_all_lines", { n = tostring(all_lines()) })
	end
	return ui.column {
		w_full = true,
		gap = "$space.1",
		class = "tool-output",
		ui.column {
			w_full = true,
			py = "$space.2",
			gap = "$space.1",
			rounded = "$radius.md",
			bg = "$zeron.code.wash",
			border = 1,
			border_color = function(): string
				return if failed() then "$zeron.tone.danger.edge" else "#00000000"
			end,
			overflow = "hidden",
			ui.show(function(): boolean
				return cleaned:get().earlier or source().earlier == true
			end, mono_note(function(): string
				return locale.s("ds.tool_detail.earlier_trimmed")
			end, "$text.tertiary")),
			ui.show(
				function(): boolean
					return #cleaned:get().lines > 0
				end,
				ui.column {
					w_full = true,
					min_w = 0,
					-- (Only a cut that hides lines below fades: the chat's
					-- end of an output is its end.)
					fade = function(): { number }
						return if folded() and #cleaned:get().lines > M.FOLD_LINES then { 0, line * FADE_LINES } else { 0, 0 }
					end,
					-- (Each line whole: a long one scrolls sideways, as a code
					-- block's, fading at the edge it runs past.)
					ui.scroll_x {
						w_full = true,
						fade_x = { 16, 32 },
						fade_overflow = true,
						ui.text {
							text,
							literal = true,
							class = "tool-output-text",
							px = "$space.3",
							font = "$font.mono",
							text_size = "$font.size.xs",
							line_height = "$font.line.xs",
							color = "$text.secondary",
						},
					},
				}
			),
			ui.show(function(): boolean
				return more() > 0
			end, mono_note(function(): string
				return locale.s("ds.tool_detail.more_lines", { n = more() })
			end, "$text.tertiary")),
			ui.show(
				function(): boolean
					return code() ~= nil
				end,
				ui.text {
					function(): string
						return locale.s("ds.tool_detail.exit_code", { code = tostring(code() or 0) })
					end,
					class = "tool-exit-code",
					px = "$space.3",
					text_size = "$font.size.xs",
					line_height = "$font.line.xs",
					color = "$zeron.tone.danger.ink",
				}
			),
		},
		ui.show(
			function(): boolean
				return trouble() ~= nil
			end,
			ui.text {
				function(): string
					return trouble() or ""
				end,
				class = "tool-output-trouble",
				px = "$space.1",
				text_size = "$font.size.xs",
				line_height = "$font.line.xs",
				color = "$text.tertiary",
			}
		),
		ui.show(
			long,
			FoldButton.FoldButton {
				label = label,
				open = is_open,
				class = "tool-output-fold",
				on_click = function()
					if loading() then
						return
					end
					local opening = not is_open()
					local all = table.clone(OUTPUT_OPEN:peek())
					all[id] = if opening then true else nil
					OUTPUT_OPEN:set(all)
					local k, t = whole(), tool()
					if opening and k and t.chat then
						local chat = t.chat
						request(k, function(): any
							return M.fetch_output(chat, t.id)
						end)
					end
				end,
				place = { self = "start" },
			}
		),
	}
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

-- Whether a tool has an output of its own to show: words, or a failed
-- command's code.
local function has_output(t: Tool): boolean
	return (t.output ~= nil and not blank(t.output)) or (t.exit_code ~= nil and t.exit_code ~= 0)
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
		if has_output(t) then
			return { key = "output" }
		end
		return { key = "none" }
	end)
	-- The whole output, for a tool with nothing else to show, as the row
	-- opens (once: what was fetched is kept by its ref, a reload too).
	ui.effect(function()
		local t = now()
		local r = t.output_ref
		if r and fetches_output(t) and BLOBS:peek()[r] == nil then
			request(r)
		end
	end)
	-- The line that asks for more: the diff first (the richer), then the
	-- output; one already showing gives its place to the other. (An output
	-- that comes as the row opens is offered again only if it failed.)
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
			if not b and c.what == "output" and fetches_output(t) then
				continue
			end
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
				-- (It follows its tool, in place: a running command's grows.
				-- Opened, a command the chat kept the end of shows its whole
				-- output, once its host sent it.)
				return output_view(now().id, function(): Source
					local t = now()
					local k = whole_key(t)
					local b = if k and OUTPUT_OPEN:get()[t.id] then BLOBS:get()[k] else nil
					if b and b.state == "ready" and type(b.value) == "table" and b.value.text then
						return { text = b.value.text, more = 0, earlier = b.value.trimmed == true }
					end
					return { text = t.output or "", more = 0 }
				end, now)
			end
			local v = s.value
			if v.kind == "diff" then
				return diff_view(v)
			end
			-- A fetched output: its lines as the host cut them.
			local fetched: Source = { text = table.concat(v.lines or {}, "\n"), more = v.more or 0 }
			return output_view(now().id, function(): Source
				return fetched
			end, now)
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
	return has_output(t) or t.diff ~= nil or t.output_ref ~= nil or t.diff_ref ~= nil or (t.stats ~= nil and #t.stats > 0)
end

-- Forgets what was fetched (a test's fresh start).
function M.forget()
	BLOBS:set({})
	OUTPUT_OPEN:set({})
end

return M
