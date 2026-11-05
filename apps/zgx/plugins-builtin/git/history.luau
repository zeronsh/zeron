--!strict
-- History: the branch's commits, newest first, the lanes they came down
-- drawn beside them (graph.luau). This branch, where it goes (its upstream)
-- and where it came from (its base), or every branch, remote and tag; a
-- search through their messages. A row is a line: the graph, the subject,
-- the refs at it (HEAD's branch in the accent, a remote's with a cloud, a
-- tag), who made it (a face), when (an exact date under the pointer) and,
-- with room, its id. A click opens it: its message whole, who made and
-- committed it, its parents, then its files and their diffs (the Changes
-- list's own, ds/git_changes), beside the list at the window's width, in
-- its place at a pane's. A secondary click: copy its id or subject, start
-- a branch there, and what other plugins add (`git.commit`).
--
-- Other plugins decorate a row through slots, each providing `git_commit`
-- ({ sha, subject, author, email, at, parents, refs, remote_url }):
-- `git.commit.face` lies over the face (a picture in its place: GitHub's
-- avatars), `git.commit` stands after the refs (a pull request's chip), and
-- `git.commit.head` in an opened commit's head (View on GitHub). With no
-- plugin there, History is git alone.
--
-- The rows are a virtualized list keyed by commit and how its lines run:
-- more history, or a refresh, builds only the rows that came or changed.

local Changes = require("@ds/git_changes")
local git = require("@ds/git")
local Badge = require("@ds/badge")
local FadeText = require("@ds/fade_text")
local IconButton = require("@ds/icon_button").IconButton
local ToggleButton = require("@ds/toggle_button")
local SearchField = require("@ds/search_field")
local EmptyState = require("@ds/empty_state")
local Button = require("@ds/button")
local Rail = require("@ds/rail")
local Tooltip = require("@ds/tooltip")
local ContextMenu = require("@ds/context_menu")
local ListRow = require("@ds/list_row").ListRow
local motion = require("@ds/motion")
local Dialog = require("@ds/dialog").Dialog
local TextField = require("@ds/text_field")
local Icon = require("@ds/icon")
local people = require("@ds/people")
local progress = require("@ds/progress")
local banner = require("@ds/banner")
local skeleton = require("@ds/skeleton")
local control = require("@ds/control")
local graph = require("./graph")

-- A row's height (the sidebar's and the panel home's), a lane's width, the
-- dot's radius, a line's weight, and the most lanes drawn (production's
-- graph: crates/ui history.rs, 12 / 3 / 1.5).
local ROW, LANE, DOT, STROKE, MAX_LANES = 30, 12, 3, 1.5, 8
-- A path pointed at (Zeron's graph focus, history.rs): its lines drawn
-- this much wider and its dots this much bigger; every other line and dot
-- faded to this much of itself, every other row's words to this much; the
-- pointer finds a dot this near its centre and a line this near itself.
local FOCUS_STROKE, NODE_GROW, DIM, ROW_DIM = 2.25, 0.75, 0.24, 0.6
local NODE_HIT, LINE_HIT = 8.5, 5.5
-- The graph's room (Zeron's): a third of the list at most (0.34); never
-- under 48; compact once the subject would get under 160, back at 184.
local GRAPH_SHARE, COMPACT_W, COMPACT_ENTER, COMPACT_EXIT = 0.34, 48, 160, 184

-- What the graph's hover lights (production's GraphFocus): the path, by
-- its colour's number (a first-parent run of lanes, graph.luau), and how
-- whether the pointer is on it; `graph`/`row` say what the pointer is
-- on (a line of the graph, else its row's own path), each by who said it.
-- How the graph stands at the list's width (Zeron's graph geometry):
-- its lanes' spacing (12 at most, less as the list narrows), or, past
-- that, compact: one rail, every dot on it (`rail`: joined row to row,
-- when every branch is shown).
export type Geometry = { spacing: number, compact: boolean, rail: boolean }
-- A row's place in the rail: the dot above's colour (nil, the first),
-- and whether one comes under it.
export type Around = { above: number?, below: boolean }

export type Focus = {
	path: State<number?>,
	-- Whether the pointer is on a path now (the path stays lit while it
	-- fades as the pointer leaves).
	active: State<boolean>,
	graph: (owner: string, color: number?) -> (),
	row: (owner: string, color: number?) -> (),
}
-- Past this width a row has room for its id.
local SHA_WIDTH = 520
-- The lanes' colours: the accent first (the branch's own), then the
-- visuals' fixed hues.
local HUES = {
	"$accent",
	"$zeron.visual.series.teal",
	"$zeron.visual.series.violet",
	"$zeron.visual.series.amber",
	"$zeron.visual.series.rose",
	"$zeron.visual.series.sky",
}
local function hue(n: number): string
	return HUES[(n % #HUES) + 1]
end

-- When, short: minutes, hours, days, then the date (its year when it
-- isn't this one's). And whole, for the pointer.
local function when_short(at: number): string
	local s = math.max(0, os.time() - at)
	if s < 60 then
		return "now"
	elseif s < 3600 then
		return `{s // 60}m`
	elseif s < 86400 then
		return `{s // 3600}h`
	elseif s < 7 * 86400 then
		return `{s // 86400}d`
	end
	local same_year = os.date("%Y", at) == os.date("%Y")
	local day = (string.gsub(os.date("%d", at) :: string, "^0", ""))
	return if same_year then `{os.date("%b", at)} {day}` else `{os.date("%b", at)} {day}, {os.date("%Y", at)}`
end
local function when_whole(at: number): string
	local day = (string.gsub(os.date("%d", at) :: string, "^0", ""))
	return `{os.date("%a", at)}, {os.date("%b", at)} {day}, {os.date("%Y %H:%M", at)}`
end

-- The refs at a commit as git says them (`%D`), as chips: HEAD's branch, a
-- local branch, a remote's (folded into its local one when they agree:
-- it's pushed), a tag. Two at most and how many more, its tooltip naming
-- them.
type Ref = { kind: string, name: string }
-- (`remotes`: which are a remote's, as the service says; without it, a
-- name with a slash is taken for one.)
local function refs_of(raw: { string }?, remotes: { string }?): { Ref }
	local out: { Ref } = {}
	local remote: { [string]: boolean }? = nil
	if remotes then
		remote = {}
		for _, r in remotes do
			(remote :: { [string]: boolean })[r] = true
		end
	end
	local function is_remote(r: string): boolean
		return if remote then remote[r] == true else string.find(r, "/", 1, true) ~= nil
	end
	local locals: { [string]: boolean } = {}
	for _, r in raw or {} do
		local head = string.match(r, "^HEAD %-> (.+)$")
		if head then
			locals[head] = true
		elseif r ~= "HEAD" and not string.match(r, "^tag: ") and not is_remote(r) then
			locals[r] = true
		end
	end
	for _, r in raw or {} do
		local head = string.match(r, "^HEAD %-> (.+)$")
		local tag = string.match(r, "^tag: (.+)$")
		if head then
			table.insert(out, 1, { kind = "head", name = head })
		elseif tag then
			table.insert(out, { kind = "tag", name = tag })
		elseif r == "HEAD" then
			continue
		else
			local remote_of = if is_remote(r) then string.match(r, "^[^/]+/(.+)$") else nil
			if remote_of and locals[remote_of] then
				continue
			end
			table.insert(out, { kind = if is_remote(r) then "remote" else "branch", name = r })
		end
	end
	return out
end

local function is_head(raw: { string }?): boolean
	for _, r in raw or {} do
		if r == "HEAD" or string.match(r, "^HEAD %-> ") then
			return true
		end
	end
	return false
end

return function(zeron: any, Repo: any, words: any)
	-- (`max_w`: the most it takes, its label fading past it.)
	local function chip(r: Ref, max_w: number?): Node
		if r.kind == "head" then
			return Badge.Badge { r.name, icon = "git-branch", tone = "accent", max_w = max_w, tooltip = zeron.s("history.ref_head", { name = r.name }), tooltip_delay = 350 }
		elseif r.kind == "tag" then
			return Badge.Badge { r.name, icon = "tag", tone = "warning", max_w = max_w, tooltip = zeron.s("history.ref_tag", { name = r.name }), tooltip_delay = 350 }
		elseif r.kind == "remote" then
			return Badge.Badge { r.name, icon = "cloud", tone = "info", max_w = max_w, tooltip = zeron.s("history.ref_remote", { name = r.name }), tooltip_delay = 350 }
		end
		return Badge.Badge { r.name, icon = "git-branch", max_w = max_w, tooltip = zeron.s("history.ref_branch", { name = r.name }), tooltip_delay = 350 }
	end
	-- The refs that didn't fit, as one more badge of theirs (their height,
	-- corner and padding, the quiet neutral wash, no outline): "+N", the
	-- last `hidden` named in its tooltip by kind, after a rest.
	local function more_badge(refs: { Ref }, hidden: number): Node
		local lines: { string } = {}
		for i = #refs - hidden + 1, #refs do
			local r = refs[i]
			local kind = if r.kind == "tag" then "tag" elseif r.kind == "remote" then "remote" else "branch"
			table.insert(lines, zeron.s(`history.ref_line.{kind}`, { name = r.name }))
		end
		return Badge.Badge {
			`+{hidden}`,
			class = "git-ref-more",
			tooltip = table.concat(lines, "\n"),
			tooltip_delay = 350,
			label = zeron.s("history.ref_more", { n = hidden }),
		}
	end

	-- (`most`: how many show before the rest fold into "+N", two unless said.)
	local function chips(raw: { string }?, most: number?, remotes: { string }?): { Node }
		local refs = refs_of(raw, remotes)
		local keep = most or 2
		local out: { Node } = {}
		for i, r in refs do
			if i > keep then
				table.insert(out, more_badge(refs, #refs - keep))
				break
			end
			table.insert(out, chip(r))
		end
		return out
	end

	local function lane_x(lane: number, g: Geometry): number
		local spacing = if g.compact then LANE else g.spacing
		local at = if g.compact then 1 else lane
		return 3 + (at - 0.5) * spacing
	end
	-- The graph's width: its lanes at their spacing (one, compact).
	local function graph_w(lanes: number, g: Geometry): number
		return if g.compact then LANE + 6 else lanes * g.spacing + 6
	end

	-- A row's lines and dot, or (`only`) those of one path, drawn `stroke`
	-- wide with a dot `dot` round. Compact: the rail through it, the dot on
	-- it.
	local function draw_row(c: any, h: number, row: graph.Row, lanes: number, head: boolean, only: number?, stroke: number, dot: number, g: Geometry, around: Around)
		if g.compact then
			local x = lane_x(1, g)
			if g.rail and around.above ~= nil and (only == nil or around.above == only) then
				c:line(x, 0, x, h / 2, hue(around.above :: number), stroke)
			end
			if g.rail and around.below and (only == nil or row.color == only) then
				c:line(x, h / 2, x, h, hue(row.color), stroke)
			end
		else
			for _, l in row.lines do
				if l.a > lanes or l.b > lanes or (only ~= nil and l.color ~= only) then
					continue
				end
				local x1, y1, x2, y2 = lane_x(l.a, g), l.from * h, lane_x(l.b, g), l.to * h
				local ink = hue(l.color)
				if x1 == x2 then
					c:line(x1, y1, x2, y2, ink, stroke)
				else
					-- A bend between lanes: leaving straight, arriving straight.
					local mid = (y1 + y2) / 2
					c:stroke(ui.path():move(x1, y1):cubic(x1, mid, x2, mid, x2, y2), ink, stroke, { cap = "round" })
				end
			end
		end
		if (g.compact or row.col <= lanes) and (only == nil or row.color == only) then
			local cx, cy = lane_x(row.col, g), h / 2
			local ink = hue(row.color)
			if head then
				c:circle(cx, cy, dot + 2.5, "$zeron.settings.card")
				c:stroke(ui.path():arc(cx, cy, dot + 2, 0, 360), ink, 1)
			end
			c:circle(cx, cy, dot, ink)
		end
	end

	-- How far a point is from a segment.
	local function from_segment(px: number, py: number, x1: number, y1: number, x2: number, y2: number): number
		local dx, dy = x2 - x1, y2 - y1
		local len = dx * dx + dy * dy
		local t = if len > 0 then math.clamp(((px - x1) * dx + (py - y1) * dy) / len, 0, 1) else 0
		local qx, qy = x1 + t * dx - px, y1 + t * dy - py
		return math.sqrt(qx * qx + qy * qy)
	end

	-- The path under the pointer in a row's cell (production's
	-- `hovered_graph_path`): its dot's, near it; else the nearest line's,
	-- near enough (a bend as ten straight pieces of its curve); else none.
	-- Compact: the rail's, anywhere near it, is the row's own.
	local function hit(row: graph.Row, lanes: number, x: number, y: number, h: number, g: Geometry): number?
		if g.compact then
			return if math.abs(x - lane_x(1, g)) <= LINE_HIT then row.color else nil
		end
		if row.col <= lanes then
			local dx, dy = x - lane_x(row.col, g), y - h / 2
			if dx * dx + dy * dy <= NODE_HIT * NODE_HIT then
				return row.color
			end
		end
		local best, best_d = nil :: number?, LINE_HIT
		for _, l in row.lines do
			if l.a > lanes or l.b > lanes then
				continue
			end
			local x1, y1, x2, y2 = lane_x(l.a, g), l.from * h, lane_x(l.b, g), l.to * h
			local d = math.huge
			if x1 == x2 then
				d = from_segment(x, y, x1, y1, x2, y2)
			else
				local mid = (y1 + y2) / 2
				local px, py = x1, y1
				for k = 1, 10 do
					local t = k / 10
					local u = 1 - t
					-- (The cubic drawn: (x1, y1), (x1, mid), (x2, mid), (x2, y2).)
					local qx = u * u * u * x1 + 3 * u * u * t * x1 + 3 * u * t * t * x2 + t * t * t * x2
					local qy = u * u * u * y1 + 3 * u * u * t * mid + 3 * u * t * t * mid + t * t * t * y2
					d = math.min(d, from_segment(x, y, px, py, qx, qy))
					px, py = qx, qy
				end
			end
			if d <= best_d then
				best, best_d = l.color, d
			end
		end
		return best
	end

	-- The graph's cell for one row: the lines through it, then its commit's
	-- dot; HEAD's in a ring. Two layers: everything, drawn once and faded as
	-- a path is lit (its opacity alone moves); over it the lit path alone,
	-- wider, drawn again only as the lit path changes (never a frame of the
	-- fade). Its width and drawing follow the list's geometry (`geom`): a
	-- resize redraws the canvases, never the row.
	local function cell(row: graph.Row, lanes: number, head: boolean, focus: Focus, owner: string, geom: State<Geometry>, around: Around): Node
		local function w(): number
			return graph_w(lanes, geom:get())
		end
		return ui.div {
			class = "git-graph",
			w = w,
			h = ROW,
			shrink = 0,
			relative = true,
			aria_hidden = true,
			on_pointer_move = function(e: any)
				focus.graph(owner, hit(row, lanes, e.local_x, e.local_y, ROW, geom:peek()))
			end,
			on_hover = function(inside: boolean)
				if not inside then
					focus.graph(owner, nil)
				end
			end,
			ui.canvas {
				w = w,
				h = ROW,
				-- (Eased in Rust: no script runs in a frame of the fade.)
				opacity = ui.tween(ui.computed(function(): number
					return if focus.active:get() then DIM else 1
				end), motion.hover()),
				draw = function(c, _w, h)
					draw_row(c, h, row, lanes, head, nil, STROKE, DOT, geom:get(), around)
				end,
			},
			ui.canvas {
				class = "git-graph-lit",
				absolute = true,
				left = 0,
				top = 0,
				w = w,
				h = ROW,
				opacity = ui.tween(ui.computed(function(): number
					return if focus.active:get() then 1 else 0
				end), motion.hover()),
				draw = function(c, _w, h)
					local p = focus.path:get()
					if p ~= nil then
						draw_row(c, h, row, lanes, head, p, FOCUS_STROKE, DOT + NODE_GROW, geom:get(), around)
					end
				end,
			},
		}
	end

	-- What a commit gives what's placed at it (other plugins' slots).
	local function given(c: any, model: any): any
		local s = model.status:peek()
		return {
			sha = c.sha,
			subject = c.subject,
			author = c.author,
			email = c.email,
			at = c.at,
			parents = c.parents,
			refs = c.refs,
			remote_url = s and s.remote_url,
			session = model.id,
		}
	end

	-- A face: the author's, their initial on their gradient; a picture may
	-- lie over it (`git.commit.face`).
	local function face(c: any, size: number): Node
		return ui.div {
			class = "git-commit-face",
			-- (Its size, for what lies over it.)
			provide = { git_face = size },
			relative = true,
			shrink = 0,
			w = size,
			h = size,
			tooltip = Tooltip { if c.email and c.email ~= "" then `{c.author} <{c.email}>` else c.author },
			people.face({ id = c.email or c.author, name = c.author }, size),
			ui.div { absolute = true, left = 0, top = 0, w = size, h = size, zeron.slot("commit.face", { w = size, h = size }) },
		}
	end

	-- A commit's row: a line, as the sidebar's rows are.
	-- A row's refs (Zeron's rule, history.rs): as many as fit their room,
	-- 45% of the commit's (never so much its subject falls under 80), each
	-- at most 112 wide with its label fading at its end; those that don't
	-- fit folded into "+N", its tooltip naming them after a rest. Each ref's
	-- width is measured once, as drawn (a hidden copy, the first time its
	-- name is seen), and kept for every row; until then, Zeron's estimate.
	local REF_RATIO, SUBJECT_MIN, REF_MAX, REF_GAP = 0.45, 80, 112, 5
	local ref_widths: { [string]: number } = {}
	local ref_widths_seen = ui.state("git.ref_widths_seen", 0)
	local function ref_key(r: Ref): string
		return `{r.kind}|{r.name}`
	end
	local function ref_width(r: Ref): number
		return ref_widths[ref_key(r)] or math.min(22 + utf8.len(r.name) :: number * 5.7, REF_MAX)
	end
	local RefChips = ui.component(function(props: { refs: { Ref }, room: () -> number }): Node
		local refs = props.refs
		local function shown(): number
			ref_widths_seen:get()
			local room = props.room()
			local n = 0
			for count = 1, #refs do
				local hidden = #refs - count
				local sum = 0
				for i = 1, count do
					sum += ref_width(refs[i])
				end
				if hidden > 0 then
					sum += 14 + #tostring(hidden) * 6
				end
				sum += (count + (if hidden > 0 then 1 else 0) - 1) * REF_GAP
				if sum <= room then
					n = count
				end
			end
			return n
		end
		local items: { Node } = {}
		for i, r in refs do
			table.insert(items, ui.show(function(): boolean
				return i <= shown()
			end, chip(r, REF_MAX)))
		end
		table.insert(items, ui.presence(function(): string
			local hidden = #refs - shown()
			return if hidden > 0 then tostring(hidden) else ""
		end, function(key: string): Node
			if key == "" then
				return ui.div {}
			end
			return more_badge(refs, tonumber(key) :: number)
		end))
		-- (The widths not measured yet: a copy no one sees, measured as it
		-- lays out.)
		local measure: Node? = nil
		local unknown: { Node } = {}
		for _, r in refs do
			local k = ref_key(r)
			if not ref_widths[k] then
				table.insert(unknown, ui.div {
					shrink = 0,
					on_resize = function(size)
						if size.w > 0 and ref_widths[k] ~= size.w then
							ref_widths[k] = size.w
							ui.after(0, function()
								ref_widths_seen:set(ref_widths_seen:peek() + 1)
							end)
						end
					end,
					chip(r, REF_MAX),
				})
			end
		end
		if #unknown > 0 then
			measure = ui.row { absolute = true, left = 0, top = 0, opacity = 0, pointer_events = false, aria_hidden = true, gap = REF_GAP, table.unpack(unknown) }
		end
		if measure then
			table.insert(items, measure)
		end
		return ui.row { class = "git-commit-refs", shrink = 0, relative = true, gap = REF_GAP, items = "center", table.unpack(items) }
	end)

	local CommitRow = ui.component(function(props: {
		c: any,
		row: graph.Row?,
		lanes: number,
		model: any,
		wide: () -> boolean,
		on_open: () -> (),
		menu: () -> { any },
		focus: Focus,
		geom: State<Geometry>,
		around: Around,
	}): Node
		local c = props.c
		local focus = props.focus
		-- The commit's room as laid out, and what other plugins put after
		-- its refs: the refs' room worked from them (Zeron's ref area).
		local main_w = ui.use_state(0)
		local end_w = ui.use_state(0)
		local function room(): number
			local inner = main_w:get()
			local area = math.min(inner * REF_RATIO, math.max(inner - SUBJECT_MIN - REF_GAP, 0))
			local taken = end_w:get()
			if taken > 0 then
				area -= taken + REF_GAP
			end
			return math.max(area, 0)
		end
		-- Whether the lit path is this row's own (its commit's lane).
		local function on_path(): boolean
			local p = focus.path:get()
			return props.row ~= nil and p ~= nil and props.row.color == p
		end
		local subject = if c.subject ~= "" then c.subject else zeron.s("history.no_subject")
		-- (The design system's list row: the sidebar's flat wash, edge to
		-- edge in the card, as every list's rows hover.)
		local line = ui.row {
			w_full = true,
			h = ROW,
			relative = true,
			pl = if props.row then 0 else "$space.3",
			pr = "$space.3",
			gap = "$space.2",
			items = "center",
			-- (On the lit path: a faint wash, as Zeron's; the list row's own
			-- hover wash over it.)
			ui.div {
				absolute = true,
				left = 0,
				top = 0,
				right = 0,
				bottom = 0,
				bg = "$hover",
				pointer_events = false,
				aria_hidden = true,
				opacity = ui.tween(ui.computed(function(): number
					return if on_path() and focus.active:get() then 0.7 else 0
				end), motion.hover()),
			},
			if props.row then cell(props.row, props.lanes, is_head(c.refs), focus, c.sha, props.geom, props.around) else nil,
			-- The rest of the row: off the lit path, quieter as it lights.
			ui.row {
				class = "git-commit-words",
				grow = 1,
				min_w = 0,
				h = ROW,
				gap = "$space.2",
				items = "center",
				opacity = ui.tween(ui.computed(function(): number
					if not focus.active:get() or props.row == nil or on_path() then
						return 1
					end
					return ROW_DIM
				end), motion.hover()),
			-- The commit: its subject (never under 80, fading at its end),
			-- its refs in their room, other plugins' word (GitHub's pull
			-- request, a ref too: its width comes off theirs).
			ui.row {
				class = "git-commit-main",
				grow = 1,
				min_w = 0,
				gap = "$space.2",
				items = "center",
				on_resize = function(size)
					if math.abs(main_w:peek() - size.w) > 0.5 then
						main_w:set(size.w)
					end
				end,
				FadeText {
					subject,
					grow = 1,
					min_w = SUBJECT_MIN,
					text_size = "$font.size.md",
					color = if c.subject ~= "" then "$text.primary" else "$text.tertiary",
				},
				RefChips { refs = refs_of(c.refs, c.remotes), room = room },
				ui.row {
					class = "git-commit-end",
					shrink = 0,
					max_w = REF_MAX,
					overflow = "hidden",
					items = "center",
					on_resize = function(size)
						if math.abs(end_w:peek() - size.w) > 0.5 then
							end_w:set(size.w)
						end
					end,
					zeron.slot("commit", { direction = "row", gap = "$space.1", items = "center" }),
				},
			},
			face(c, 18),
			ui.text {
				when_short(c.at),
				shrink = 0,
				min_w = ui.raw(30, "the widest short date, so they line up"),
				align = "right",
				tabular = true,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
				tooltip = Tooltip { when_whole(c.at) },
			},
			ui.show(props.wide, ui.text { string.sub(c.sha, 1, 7), shrink = 0, font = "$font.mono", text_size = "$font.size.xs", color = "$text.tertiary" }),
			},
		}
		local body = ListRow {
			class = "git-commit-row",
			provide = { git_commit = given(c, props.model) },
			h = ROW,
			label = subject,
			on_click = props.on_open,
			-- (Its own path lit as the pointer comes, Zeron's row hover.)
			on_hover = function(inside: boolean)
				focus.row(c.sha, if inside and props.row then props.row.color else nil)
			end,
			line,
		}
		return ContextMenu {
			label = subject,
			ring = "$radius.sm",
			w_full = true,
			items = function()
				return props.menu()
			end,
			body,
		}
	end)

	-- The id as a chip: a click copies it, and says so a moment.
	local function sha_chip(sha: string): Node
		local copied = ui.use_state(false)
		return ui.row {
			id = "git-commit-sha",
			role = "button",
			label = zeron.s("history.copy_sha"),
			tooltip = Tooltip { zeron.t("history.copy_sha") },
			focusable = true,
			cursor = "pointer",
			shrink = 0,
			h = 20,
			px = "$space.1",
			gap = "$space.1",
			items = "center",
			rounded = "$radius.md",
			bg = "$zeron.secondary.bg",
			hover = { bg = "$hover" },
			focus = control.ring,
			on_click = function()
				ui.clipboard.write(sha)
				copied:set(true)
				ui.after(1200, function()
					copied:set(false)
				end)
			end,
			ui.text {
				function(): string
					-- (Seven, as its parents and its row say it.)
					return if copied:get() then zeron.s("history.copied") else string.sub(sha, 1, 7)
				end,
				font = "$font.mono",
				text_size = "$font.size.xs",
				color = function(): string
					return if copied:get() then "$accent" else "$text.secondary"
				end,
			},
		}
	end

	-- A commit opened: its message whole, who and when, its parents, what it
	-- adds and takes, what other plugins say of it, then its files.
	local CommitDetail = ui.component(function(props: { model: any, c: any, narrow: boolean, on_back: (() -> ())?, open_sha: (sha: string) -> () }): Node
		local model, c = props.model, props.c
		model.commit_info(c.sha)
		local answer = ui.state(`git.commit.{c.sha}`, nil :: any)
		local failed = ui.state(`git.commit.{c.sha}.failed`, nil :: any)
		if not answer:peek() then
			model.commit_changes(c.sha, function(a: any)
				answer:set(a)
			end, function(p: any)
				failed:set(p)
			end)
		end
		local filter = ui.state(`git.commit.{c.sha}.filter`, "")
		local open = ui.state(`git.commit.{c.sha}.open`, {} :: { [string]: boolean })
		local folded = ui.state(`git.commit.{c.sha}.folded`, {} :: { [string]: boolean })
		local whole = ui.state(`git.commit.{c.sha}.whole`, false)
		local function info(): any
			local i = model.infos:get()[c.sha]
			return if type(i) == "table" then i else nil
		end
		local function lines_of(f: Changes.File): { any }?
			local d = model.diff_of(f, answer:get())
			return if d then d.lines else nil
		end
		local parents: { Node } = {}
		for _, p in c.parents or {} do
			table.insert(parents, ui.row {
				role = "link",
				label = zeron.s("history.open_parent", { sha = string.sub(p, 1, 7) }),
				cursor = "pointer",
				focusable = true,
				shrink = 0,
				h = 20,
				px = "$space.1",
				items = "center",
				rounded = "$radius.md",
				hover = { bg = "$hover" },
				focus = control.ring,
				on_click = function()
					props.open_sha(p)
				end,
				ui.text { string.sub(p, 1, 7), font = "$font.mono", text_size = "$font.size.xs", color = "$text.secondary" },
			})
		end
		-- (The refs on the facts line: what they need, and what they have.)
		local refs_clip, refs_room = ui.ref(), ui.ref()
		local head = ui.column {
			id = "git-commit-head",
			w_full = true,
			shrink = 0,
			-- (Its words on the files card's words, the card hanging past.)
			px = ui.raw(git.text_inset(), "the card's text inset: its edge and padding"),
			gap = "$space.2",
			provide = { git_commit = given(c, model) },
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "start",
				if props.on_back
					then IconButton { icon = "alt-arrow-left", label = zeron.s("history.back"), tone = "quiet", tooltip_side = "bottom", on_click = props.on_back }
					else nil,
				ui.text {
					if c.subject ~= "" then c.subject else zeron.s("history.no_subject"),
					grow = 1,
					min_w = 0,
					wrap = true,
					selectable = true,
					text_size = "$font.size.lg",
					weight = "$font.weight.medium",
					color = "$text.primary",
				},
			},
			-- The message's body, eight lines until asked for whole.
			ui.show(function(): boolean
				local i = info()
				return i ~= nil and i.body ~= ""
			end, ui.column {
				w_full = true,
				gap = "$space.1",
				ui.presence(function(): string
					return if whole:get() then "whole" else "cut"
				end, function(key: string): Node
					return ui.text {
						function(): string
							local i = info()
							return if i then i.body else ""
						end,
						w_full = true,
						wrap = true,
						lines = if key == "whole" then nil else 8,
						selectable = true,
						-- (Prose, in the interface's face, as a message is.)
						text_size = "$font.size.md",
						line_height = "$font.line.md",
						color = "$text.secondary",
					}
				end, { w_full = true }),
				ui.show(function(): boolean
					local i = info()
					local _, n = string.gsub(if i then i.body else "", "\n", "")
					return n >= 8
				end, Button {
					function(): string
						return zeron.s(if whole:get() then "history.body_less" else "history.body_more")
					end,
					tone = "ghost",
					size = "xs",
					on_click = function()
						whole:set(not whole:peek())
					end,
				}),
			}),
			-- Who and when; who committed it, when not the author; its id.
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				flex_wrap = true,
				face(c, 20),
				ui.text { c.author, shrink = 0, text_size = "$font.size.sm", weight = "$font.weight.medium", color = "$text.primary" },
				ui.text { when_whole(c.at), shrink = 0, text_size = "$font.size.sm", color = "$text.tertiary" },
				ui.show(function(): boolean
					local i = info()
					return i ~= nil and i.committer ~= "" and i.committer ~= c.author
				end, ui.text {
					function(): string
						local i = info()
						return if i then zeron.s("history.committed_by", { name = i.committer }) else ""
					end,
					shrink = 0,
					text_size = "$font.size.sm",
					color = "$text.tertiary",
				}),
			},
			-- One line, at any width: the id and the end (what it adds, other
			-- plugins' way to it) always on it; at a pane's width the parents
			-- go by their count and the refs fold to one and "+N", and what
			-- the refs can't fit fades at their end.
			ui.row {
				id = "git-commit-facts",
				w_full = true,
				-- (The id is a chip: its words, not its box, on the text edge.)
				ml = ui.raw(-((ui.theme.get("space.1") :: number?) or 4), "the id chip's own padding"),
				gap = "$space.2",
				items = "center",
				sha_chip(c.sha),
				if #parents > 0
					then ui.row {
						shrink = 0,
						gap = "$space.1",
						items = "center",
						ui.text {
							if props.narrow then zeron.t("history.parents_n", { n = #parents }) else zeron.t("history.parents", { n = #parents }),
							text_size = "$font.size.xs",
							color = "$text.tertiary",
						},
						if props.narrow then nil else ui.row { gap = "$space.1", items = "center", table.unpack(parents) },
					}
					else nil,
				ui.row {
					ref = refs_clip,
					shrink = 1,
					min_w = 0,
					overflow = "hidden",
					fade_x = function(): { number }
						return { 0, if (refs_room.width or 0) > (refs_clip.width or 0) + 0.5 then 16 else 0 }
					end,
					items = "center",
					ui.row { ref = refs_room, shrink = 0, gap = "$space.1", items = "center", table.unpack(chips(c.refs, if props.narrow then 1 else 2, c.remotes)) },
				},
				ui.div { grow = 1 },
				ui.show(function(): boolean
					return info() ~= nil
				end, ui.row {
					shrink = 0,
					gap = "$space.2",
					items = "center",
					ui.text {
						function(): string
							local i = info()
							return if i then zeron.s("history.files", { n = i.files }) else ""
						end,
						text_size = "$font.size.xs",
						color = "$text.tertiary",
					},
					ui.presence(function(): string
						local i = info()
						return if i then `{i.adds}/{i.dels}` else ""
					end, function(): Node
						local i = info()
						return Badge.DiffStat { added = if i then i.adds else 0, removed = if i then i.dels else 0 }
					end),
				}),
				-- Other plugins' word on it (GitHub's pull request, its page).
				ui.row { class = "git-commit-head-end", shrink = 0, items = "center", zeron.slot("commit.head", { direction = "row", gap = "$space.1", items = "center" }) },
			},
		}
		return ui.column {
			id = "git-commit",
			w_full = true,
			grow = 1,
			min_h = 0,
			gap = "$space.3",
			head,
			ui.presence(function(): string
				return if failed:get() then "failed" elseif answer:get() then "files" else "loading"
			end, function(key: string): Node
				if key == "failed" then
					local said = words(failed:peek(), model.status:peek())
					return banner.Callout { tone = "danger", title = said.title, body = said.body }
				elseif key == "loading" then
					return ui.column { w_full = true, gap = "$space.2", p = "$space.2", skeleton.Bar { w_full = true, h = 14 }, skeleton.Bar { w = 220, h = 14 }, skeleton.Bar { w = 260, h = 14 } }
				end
				local files = Repo.to_files(answer:peek().files)
				if #files == 0 then
					return EmptyState { icon = "git-commit", title = zeron.s("history.no_files_title"), body = zeron.s("history.no_files_body"), size = "sm", fill = true }
				end
				return Changes.panel {
					id = "git-commit-files",
					files = files,
					filter = filter,
					narrow = true,
					columns = if props.narrow then 1 else 2,
					folded = folded,
					open = open,
					lines = lines_of,
				}
			end, { w_full = true, grow = 1, min_h = 0, enter = { opacity = { 0, 1 }, duration = 120 } }),
		}
	end)

	local HistoryView = ui.component(function(props: { model: any, wide: boolean, panel: any? }): Node
		local model = props.model
		local wide = props.wide
		local offset = ui.state(`git.{model.id}.history_scroll`, 0)
		local query = ui.state(`git.{model.id}.history_typed`, model.history_query:peek())
		local all = model.history_all
		local seen = ui.use_state(0)
		local width = ui.use_state(0)
		-- The graph at the list's width (Zeron's responsive graph): its lanes
		-- at their 12 while the graph takes at most a third of the list and
		-- leaves the subject its 80 beside the columns after it; past that,
		-- closer; past where the subject would keep 160 (or the lanes 4
		-- apart), compact, one rail, until it has 184 (and 5) again. Worked
		-- out as the list's width moves; the rows keep, their graphs redraw.
		local geom = ui.use_state({ spacing = LANE, compact = false, rail = false } :: Geometry)
		-- A branch started at a commit: which, and its name, while asked.
		local branching = ui.use_state(false)
		local branch_from = ui.use_state("")
		local branch_name = ui.use_state("")
		-- The graph's hover (Zeron's): the path lit, kept through its fade out;
		-- whether the pointer is on one; how far it's lit. What the graph's
		-- lines say wins over what a row says; each said by who's under the
		-- pointer, so one leaving never clears another that came.
		local lit = ui.use_state(nil :: number?)
		local active = ui.use_state(false)
		local on_graph: { owner: string?, color: number? } = { owner = nil, color = nil }
		local on_row: { owner: string?, color: number? } = { owner = nil, color = nil }
		local clear: (() -> ())? = nil
		local function settle()
			local p = if on_graph.color ~= nil then on_graph.color else on_row.color
			if p ~= nil then
				if clear then
					clear()
					clear = nil
				end
				if lit:peek() ~= p then
					lit:set(p)
				end
				if not active:peek() then
					active:set(true)
				end
				return
			end
			if active:peek() then
				active:set(false)
			end
			-- (The path stays lit while it fades, then goes.)
			local was = lit:peek()
			if clear then
				clear()
			end
			clear = ui.after(motion.ms(motion.HOVER_MS), function()
				clear = nil
				if not active:peek() and lit:peek() == was then
					lit:set(nil)
				end
			end)
		end
		local function said(where: { owner: string?, color: number? }, owner: string, color: number?)
			if color == nil and where.owner ~= owner then
				return
			end
			if where.owner == owner and where.color == color then
				return
			end
			where.owner, where.color = if color ~= nil then owner else nil, color
			settle()
		end
		local focus: Focus = {
			path = lit,
			active = active,
			graph = function(owner: string, color: number?)
				said(on_graph, owner, color)
			end,
			row = function(owner: string, color: number?)
				said(on_row, owner, color)
			end,
		}
		if not model.history:peek() then
			model.load_history(false)
		end
		-- The search, asked a moment after typing rests; All, at once.
		local typed_at = 0
		ui.effect(function()
			local q = query:get()
			if q == model.history_query:peek() then
				return
			end
			typed_at += 1
			local mine = typed_at
			ui.after(250, function()
				if mine == typed_at and query:peek() ~= model.history_query:peek() then
					model.history_query:set(query:peek())
					offset:set(0)
					model.load_history(false)
				end
			end)
		end)
		local searching = function(): boolean
			return model.history_query:get() ~= ""
		end
		-- The graph, worked out as the commits change (none while searching:
		-- the commits found aren't a line), and the lanes it needs at most.
		local laid = ui.computed(function(): { rows: { graph.Row }, lanes: number, commits: { any } }
			local h = model.history:get()
			local commits = if h then h.commits else {}
			if searching() then
				return { rows = {}, lanes = 0, commits = commits }
			end
			local rows, widest = graph.layout(commits)
			return { rows = rows, lanes = math.min(widest, MAX_LANES), commits = commits }
		end)
		ui.effect(function()
			local w = width:get()
			local lanes = laid:get().lanes
			local rail = all:get()
			if w <= 0 then
				return
			end
			-- (What follows the subject: face, date, the id when there's room,
			-- their gaps and the row's end.)
			local columns = 18 + 30 + (if w >= SHA_WIDTH then 52 else 0) + 3 * 8 + 12
			local natural = lanes * LANE + 6
			local target = natural
			if lanes > 1 and natural > COMPACT_W then
				local content = math.max(w - columns - SUBJECT_MIN, COMPACT_W)
				local share = math.max(w * GRAPH_SHARE, COMPACT_W)
				target = math.min(natural, content, share)
				-- (In steps of 2, so a drag doesn't redraw at every point.)
				target = if target >= natural then natural else math.floor(target / 2) * 2
			end
			local spacing = if lanes > 0 then math.max(target - 6, 0) / math.max(lanes, 1) else LANE
			local was = geom:peek()
			local subject = w - columns - target
			local compact = lanes > 1
				and (if was.compact then subject < COMPACT_EXIT or spacing < 5 else subject < COMPACT_ENTER or spacing < 4)
			local now = { spacing = math.min(spacing, LANE), compact = compact, rail = rail }
			if now.spacing ~= was.spacing or now.compact ~= was.compact or now.rail ~= was.rail then
				geom:set(now)
			end
		end)
		-- Rows keyed by commit and how its lines run (a row whose lines
		-- moved is drawn again), then "older commits" when there are more.
		local keys = ui.computed(function(): { string }
			local l = laid:get()
			local out: { string } = {}
			for i, c in l.commits do
				local r = l.rows[i]
				local sig: { string } = { tostring(l.lanes) }
				if r then
					table.insert(sig, tostring(r.col))
					for _, line in r.lines do
						table.insert(sig, `{line.a}{line.b}{line.from}{line.to}{line.color}`)
					end
				end
				out[i] = `{c.sha}:{table.concat(sig, ",")}`
			end
			local h = model.history:get()
			if h and h.more then
				table.insert(out, "more")
			end
			return out
		end)
		local index = ui.computed(function(): { [string]: number }
			local map: { [string]: number } = {}
			for i, k in keys:get() do
				map[k] = i
			end
			return map
		end)
		local function find(sha: string): any?
			for _, x in laid:peek().commits do
				if x.sha == sha then
					return x
				end
			end
			return nil
		end
		-- A commit opened as a tab of its own (Zeron's commit diff tab):
		-- the one open already brought forward; closed, gone.
		local panel = props.panel
		local function open_commit(sha: string)
			local c = find(sha)
			if c and panel then
				Repo.open_commit(panel, model.id, c)
			end
		end
		-- A row's menu: its id, its subject, a branch from it, then what
		-- other plugins add (`git.commit`, given the commit).
		local function menu_of(c: any): { any }
			local entries: { any } = {
				{
					label = zeron.s("history.menu.copy_sha"),
					icon = "copy",
					on_select = function()
						ui.clipboard.write(c.sha)
					end,
				},
				{
					label = zeron.s("history.menu.copy_subject"),
					icon = "copy",
					on_select = function()
						ui.clipboard.write(c.subject)
					end,
				},
				{ separator = true },
				{
					label = zeron.s("history.menu.branch_here"),
					icon = "git-branch",
					on_select = function()
						branch_from:set(c.sha)
						branch_name:set("")
						branching:set(true)
					end,
				},
			}
			local extra = zeron.menu_entries("commit", given(c, model))
			if #extra > 0 then
				table.insert(entries, { separator = true })
				for _, e in extra do
					table.insert(entries, e)
				end
			end
			return entries
		end
		local function list(): Node
			return ui.div {
				id = "git-history",
				w_full = true,
				grow = 1,
				min_h = 0,
				relative = true,
				on_resize = function(size)
					seen:set(size.h)
					width:set(size.w)
				end,
				ui.list {
					w_full = true,
					h_full = true,
					offset = offset,
					overscan = 8,
					heights = function(): { number }
						local hs: { number } = {}
						for i = 1, #keys:get() do
							hs[i] = ROW
						end
						return hs
					end,
					key = function(i: number): string
						return keys:peek()[i] or `none:{i}`
					end,
					row = function(key: string): Node
						if key == "more" then
							return ui.row {
								w_full = true,
								h = ROW,
								justify = "center",
								items = "center",
								Button {
									function(): string
										return zeron.s(if model.history_loading_state:get() then "loading" else "history.older")
									end,
									tone = "ghost",
									size = "xs",
									on_click = function()
										model.load_history(true)
									end,
								},
							}
						end
						local i = index:peek()[key]
						local l = laid:peek()
						local c = if i then l.commits[i] else nil
						if not c then
							return ui.div { h = ROW }
						end
						return ui.div {
							w_full = true,
							CommitRow {
								c = c,
								row = l.rows[i],
								lanes = l.lanes,
								geom = geom,
								around = { above = if i > 1 and l.rows[i - 1] then l.rows[i - 1].color else nil, below = l.rows[i + 1] ~= nil },
								model = model,
								wide = function(): boolean
									return width:get() >= SHA_WIDTH
								end,
								on_open = function()
									open_commit(c.sha)
								end,
								menu = function(): { any }
									return menu_of(c)
								end,
								focus = focus,
							},
						}
					end,
					row_text = function(i: number): string
						local c = laid:peek().commits[i]
						return if c then c.subject else ""
					end,
				},
				Rail {
					id = "git-history-rail",
					offset = offset,
					extent = function(): number
						return #keys:get() * ROW
					end,
					view = seen,
				},
			}
		end
		-- How far the branch is from its base, as a pill.
		local function base_pill(): Node
			return ui.show(function(): boolean
				local s = model.status:get()
				return s ~= nil and s.base ~= nil and ((s.base_ahead or 0) > 0 or (s.base_behind or 0) > 0)
			end, ui.row {
				id = "git-history-base",
				shrink = 0,
				-- (A chip's height, its corner round: the band's chips.)
				h = 24,
				px = "$zeron.space.tight",
				gap = "$space.1",
				items = "center",
				rounded = "$radius.full",
				bg = "$zeron.secondary.bg",
				tooltip = Tooltip {
					function(): string
						local s = model.status:get()
						return zeron.s("history.base_tip", { base = (s and s.base) or "", ahead = (s and s.base_ahead) or 0, behind = (s and s.base_behind) or 0 })
					end,
				},
				ui.text {
					function(): string
						local s = model.status:get()
						return `↑{(s and s.base_ahead) or 0}`
					end,
					tabular = true,
					text_size = "$font.size.sm",
					color = "$accent",
				},
				ui.text {
					function(): string
						local s = model.status:get()
						return `↓{(s and s.base_behind) or 0}`
					end,
					tabular = true,
					text_size = "$font.size.sm",
					color = "$warning",
				},
			})
		end
		-- The list in its card: what it holds and the controls as its head,
		-- the search under it, the commits under that.
		local function card(): Node
			return ui.column {
				w_full = true,
				grow = 1,
				min_h = 0,
				rounded = git.RADIUS,
				bg = "$zeron.settings.card",
				border = git.EDGE,
				border_color = "$zeron.settings.edge",
				overflow = "hidden",
				ui.row {
					id = "git-history-head",
					w_full = true,
					h = Changes.HEAD,
					shrink = 0,
					pl = "$space.3",
					pr = git.HEAD_END,
					gap = "$space.2",
					items = "center",
					FadeText {
						function(): string
							local h = model.history:get()
							local n = if h then #h.commits else 0
							if searching() then
								return zeron.s("history.found", { n = n, more = if h and h.more then "yes" else "no" })
							end
							local s = model.status:get()
							local branch = (s and s.branch) or ""
							if model.history_all:get() then
								return zeron.s(if h and h.more then "history.head_all_more" else "history.head_all", { n = n })
							end
							return zeron.s(if h and h.more then "history.head_more" else "history.head", { n = n, branch = branch })
						end,
						grow = 1,
						min_w = 0,
						text_size = "$font.size.sm",
						weight = "$font.weight.medium",
						color = "$text.secondary",
					},
					-- Its trailing controls, one gap apart, the band's size.
					ui.row {
						id = "git-history-tools",
						shrink = 0,
						gap = git.HEAD_GAP,
						items = "center",
					base_pill(),
					ToggleButton {
						on = all,
						icon = "git-merge",
						label = zeron.s("history.all_branches"),
						size = "md",
						on_change = function()
							offset:set(0)
							model.load_history(false)
						end,
					},
					ui.presence(function(): string
						return if model.acting:get() == "fetch" then "fetching" else "idle"
					end, function(key: string): Node
						if key == "fetching" then
							return ui.row { w = 24, h = 24, items = "center", justify = "center", progress.ProgressRing { size = 14, tone = "neutral", label = zeron.s("history.fetching") } }
						end
						return IconButton {
							icon = "refresh",
							label = zeron.s("history.fetch"),
							size = "md",
							tone = "quiet",
							on_click = function()
								model.act("fetch", "fetch", nil, function()
									model.load_history(false)
								end)
							end,
						}
					end, { shrink = 0 }),
					},
				},
				ui.div { h = 1, w_full = true, shrink = 0, bg = "$zeron.settings.rule", aria_hidden = true },
				ui.div {
					w_full = true,
					shrink = 0,
					p = "$space.1",
					theme = { radius = { md = control.nest(git.RADIUS, git.EDGE, "$space.1") } },
					SearchField { value = query, placeholder = zeron.t("history.search"), size = "sm", quiet = true, w_full = true },
				},
				ui.div { h = 1, w_full = true, shrink = 0, bg = "$zeron.settings.rule", aria_hidden = true },
				-- A fetch that failed, said over the list until put away.
				ui.presence(function(): string
					local a = model.act_problem:get()
					return if a and a.what == "fetch" then `{a.problem.reason}:{a.problem.words}` else ""
				end, function(key: string): Node
					if key == "" then
						return ui.div {}
					end
					local a = model.act_problem:peek()
					local said = words(a.problem, model.status:peek())
					return ui.column {
						w_full = true,
						p = "$space.2",
						banner.Callout {
							tone = "danger",
							title = zeron.s("history.fetch_failed"),
							body = said.body,
							on_dismiss = function()
								model.act_problem:set(nil)
							end,
						},
					}
				end, { w_full = true, shrink = 0 }),
				ui.presence(function(): string
					local h = model.history:get()
					if model.history_problem:get() and not h then
						return "failed"
					end
					if not h then
						return "loading"
					end
					if #h.commits == 0 then
						return if searching() then "none_found" else "empty"
					end
					return "list"
				end, function(key: string): Node
					if key == "failed" then
						local said = words(model.history_problem:peek(), model.status:peek())
						return ui.column {
							w_full = true,
							p = "$space.3",
							banner.Callout {
								tone = "danger",
								title = said.title,
								body = said.body,
								action = {
									label = zeron.s("try_again"),
									on_click = function()
										model.load_history(false)
									end,
								},
							},
						}
					elseif key == "loading" then
						-- Rows to come, as their shapes.
						local rows: { Node } = {}
						for n = 1, 8 do
							table.insert(rows, ui.row {
								w_full = true,
								h = ROW,
								px = "$space.3",
								gap = "$space.3",
								items = "center",
								skeleton.Bar { w = 8, h = 8, rounded = "$radius.full" },
								skeleton.Bar { w = ui.raw(120 + (n * 37) % 140, "a subject's length, varied"), h = 10 },
								ui.div { grow = 1 },
								skeleton.Bar { w = 18, h = 18, rounded = "$radius.full" },
							})
						end
						return ui.column { id = "git-history-loading", w_full = true, pt = "$space.1", table.unpack(rows) }
					elseif key == "none_found" then
						return EmptyState {
							icon = "search",
							title = zeron.s("history.none_found_title"),
							body = function(): string
								return zeron.s("history.none_found_body", { query = model.history_query:get() })
							end,
							size = "sm",
							fill = true,
						}
					elseif key == "empty" then
						return EmptyState { icon = "git-commit", title = zeron.s("history.empty_title"), body = zeron.s("history.empty_body"), size = "sm", fill = true }
					end
					return list()
				end, { w_full = true, grow = 1, min_h = 0 }),
			}
		end
		local new_branch = Dialog {
			open = branching,
			title = zeron.t("branches.new_title"),
			body = function(): string
				return zeron.s("history.branch_from", { sha = string.sub(branch_from:get(), 1, 7) })
			end,
			confirm = {
				label = zeron.s("branches.create"),
				on_click = function()
					local name = string.gsub(string.gsub(branch_name:peek(), "^%s+", ""), "%s+$", "")
					if name == "" then
						return
					end
					branching:set(false)
					model.act("switch", "switch", { branch = name, create = true, start = branch_from:peek() }, function()
						model.branches:set(nil)
						model.load_history(false)
					end)
				end,
			},
			TextField {
				value = branch_name,
				name = zeron.s("branches.name"),
				placeholder = zeron.t("branches.name_placeholder"),
				autofocus = true,
			},
		}
		-- The list, the tab's whole width: a commit opens in a tab of its
		-- own (closed, the list is all there was).
		return ui.column {
			w_full = true,
			grow = 1,
			min_h = 0,
			card(),
			new_branch,
		}
	end)

	-- A commit's own tab: its detail, the whole tab (a parent it names opens
	-- in a tab of its own too).
	local CommitView = ui.component(function(props: { model: any, c: any, wide: boolean, panel: any? }): Node
		local model = props.model
		local panel = props.panel
		return CommitDetail {
			model = model,
			c = props.c,
			narrow = not props.wide,
			open_sha = function(p: string)
				if not panel then
					return
				end
				local h = model.history:peek()
				for _, x in (h and h.commits) or {} do
					if x.sha == p then
						Repo.open_commit(panel, model.id, x)
						return
					end
				end
				Repo.open_commit(panel, model.id, { sha = p, subject = "", parents = {}, refs = {} })
			end,
		}
	end)

	return HistoryView, CommitView
end
