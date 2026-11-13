--!strict
-- A pull request in a chat's panel (the `github-pr` tab), and beside the
-- board's list: the gallery's review (demos/example_git_review), drawn from
-- Zeron's design system as it is lent to plugins (`@ds`, the running app's,
-- so it follows the template in use), with what GitHub says of it. Its
-- behaviour is Zeron's viewer's (crates/ui/src/pull_request_detail.rs).
--
-- Its header: its state and number, its title, who opened it and when,
-- from which branch into which, how much it changes; a way to it on GitHub
-- and to ask again. Then the app's three tabs:
--   - Summary, where to act: its checks folded to a line (first, when one
--     failed), its description, the verdicts so far and a comment;
--   - Code, the change: its files and their diffs, a + on any line to
--     comment there (as you: "github.write"), each line's threads under
--     it, to reply to and resolve;
--   - Activity, what was said and decided, in order, and a comment.
--
-- Two widths, as the gallery's: a side pane's (under 640, its header
-- stacked, its tabs' badges under them, a file opening its diff in place)
-- and the window's; past 880 the Code tab's list stands beside the diff.

local git = require("@ds/git")
local Changes = require("@ds/git_changes")
local Badge = require("@ds/badge")
local Button = require("@ds/button")
local IconButton = require("@ds/icon_button").IconButton
local Markdown = require("@ds/markdown")
local Html = require("./html")
local Lightbox = require("@ds/lightbox")
local Links = require("@ds/links")
local MenuAt = require("@ds/menu_at")
local Inline = require("@ds/inline_block")
local comment = require("@ds/comment")
local Settings = require("@ds/settings")
local banner = require("@ds/banner")
local TextArea = require("@ds/text_area")
local RadioGroup = require("@ds/radio")
local Switch = require("@ds/switch")
local ProgressRing = require("@ds/progress").ProgressRing
local Menu = require("@ds/menu")
local Glass = require("@ds/glass")
local people = require("@ds/people")
local progress = require("@ds/progress")
local EmptyState = require("@ds/empty_state")
local Icon = require("@ds/icon")

export type Detail = {
	title: string,
	body: string,
	url: string,
	number: number,
	author: { login: string? }?,
	baseRefName: string?,
	headRefName: string?,
	state: string?,
	isDraft: boolean?,
	createdAt: string?,
	reviewDecision: string?,
	additions: number?,
	deletions: number?,
	ci: { state: string?, totalCount: number? }?,
	statusCheckRollup: { Check }?,
	files: { { path: string, additions: number?, deletions: number?, status: string? } }?,
	headRefOid: string?,
	baseRefOid: string?,
	comments: { Comment }?,
	reviews: { Comment }?,
	reviewThreads: { Thread }?,
	-- Who you are on GitHub (the engine's `viewer`).
	viewerLogin: string?,
	-- Whether and how it can go in (pulls.luau's merge fields).
	id: string?,
	mergeable: string?,
	mergeStateStatus: string?,
	merged: boolean?,
	mergedAt: string?,
	mergedBy: { login: string? }?,
	closedAt: string?,
	isCrossRepository: boolean?,
	viewerDidAuthor: boolean?,
	commitCount: number?,
	repo: { squash: boolean?, merge: boolean?, rebase: boolean?, deleteOnMerge: boolean?, permission: string? }?,
}
export type Comment = { id: string?, author: { login: string? }?, body: string?, state: string?, createdAt: string?, submittedAt: string?, url: string?, replyTo: any?, viewerDidAuthor: boolean? }
export type Thread = { id: string, path: string, line: number?, originalLine: number?, diffSide: string?, isResolved: boolean?, isOutdated: boolean?, comments: { Comment } }
export type Check = { name: string?, context: string?, status: string?, conclusion: string?, state: string?, detailsUrl: string?, targetUrl: string? }

-- A file's diff shows at most this many lines (and says so past them).
local MAX_LINES = 2000
-- Under this width the header stacks, the gutters tighten and a file opens
-- its diff in place (the gallery's side-pane layout); past the second, the
-- Code tab's list stands beside the diff (its window layout).
local NARROW, SPLIT = 640, 880

-- The plugin's words (the SDK's `s` and `t`, locales/en.json), set as the
-- viewer is made: every line here says them through it.
local T: any = nil

-- What went wrong, from a promise's rejection.
local function reason(err: any): string
	return if type(err) == "table" and err.message then tostring(err.message) else tostring(err)
end

-- A time as GitHub gives it (`2026-11-01T10:00:00Z`), in seconds since
-- the epoch (nil if it can't be read).
local UTC_OFFSET = os.time() - os.time(os.date("!*t") :: any)
local function epoch(iso: string?): number?
	local y, mo, d, h, mi, se = string.match(iso or "", "^(%d+)-(%d+)-(%d+)T(%d+):(%d+):(%d+)")
	if not y then
		return nil
	end
	local t = os.time({ year = tonumber(y) :: number, month = tonumber(mo) :: number, day = tonumber(d) :: number, hour = tonumber(h) :: number, min = tonumber(mi) :: number, sec = tonumber(se) :: number })
	return t + UTC_OFFSET
end

-- How long ago, in words, for a line that is text ("12 minutes ago").
local function ago(at: number?): string
	if not at then
		return ""
	end
	local s = math.max(0, os.time() - at)
	if s < 60 then
		return T.s("time.just_now")
	elseif s < 3600 then
		return T.s("time.minutes", { n = s // 60 })
	elseif s < 86400 then
		return T.s("time.hours", { n = s // 3600 })
	end
	return T.s("time.days", { n = s // 86400 })
end

-- Someone's GitHub picture (set up with the plugin).
local face_of: ((login: string) -> () -> string?)? = nil

-- Someone on GitHub, as the design system's people are: their picture once
-- it's here, their gradient until then.
local function who(author: { login: string?, avatarUrl: string? }?): people.Who
	local login = (author and author.login) or T.s("people.someone")
	local own = author and type(author.avatarUrl) == "string" and string.match(author.avatarUrl, "^https://avatars%.githubusercontent%.com/") and author.avatarUrl or nil
	local src = if own
		then function(): string?
			return own
		end
		elseif face_of and author and author.login then face_of(login)
		else nil
	return { id = login, name = login, src = src }
end

-- Its state as the badge says it: GitHub's, a draft its own.
local function state_of(d: Detail): string
	local s = string.lower(d.state or "open")
	if d.isDraft and s == "open" then
		return "draft"
	end
	return if s == "merged" or s == "closed" then s else "open"
end

-- Where its review stands (GitHub's decision): approved, changes asked
-- for, or waiting.
local function review_of(d: Detail): string
	local r = string.lower((string.gsub(d.reviewDecision or "", "_", "")))
	return if r == "approved" then "approved" elseif r == "changesrequested" then "changes" else "pending"
end

-- A check's status, as GitHub gives it: its conclusion, else its state,
-- else its status (pull_request_detail.rs's `check_status`).
local function check_status(c: Check): string
	for _, v in { c.conclusion, c.state, c.status } do
		if v and v ~= "" then
			return string.upper(v)
		end
	end
	return ""
end

local function humanize(raw: string): string
	local text = string.lower((string.gsub(raw, "_", " ")))
	if text == "" then
		return T.s("checks.not_reported")
	end
	return string.upper(string.sub(text, 1, 1)) .. string.sub(text, 2)
end

-- What a status says and how it stands, as Zeron's viewer has them
-- (`status_style`): passing, failing, running, or anything else.
local function style(raw: string): (string, string)
	local key = string.gsub(string.upper(raw), " ", "_")
	if key == "SUCCESS" then
		return T.s("checks.passed"), "passing"
	elseif key == "FAILURE" or key == "ERROR" or key == "TIMED_OUT" or key == "ACTION_REQUIRED" then
		return humanize(raw), "failing"
	elseif key == "IN_PROGRESS" or key == "PENDING" or key == "QUEUED" or key == "WAITING" then
		return humanize(raw), "running"
	end
	return humanize(raw), "other"
end

-- Its checks, as the design system's: each its name, how it stands, and a
-- way to its page on GitHub, where it has one.
local function checks_of(zeron: any, d: Detail): { git.Check }
	local out: { git.Check } = {}
	for _, c in d.statusCheckRollup or {} do
		local label, kind = style(check_status(c))
		local link = if c.detailsUrl and c.detailsUrl ~= "" then c.detailsUrl else c.targetUrl
		local name = if c.name and c.name ~= "" then c.name elseif c.context and c.context ~= "" then c.context else T.s("checks.check")
		table.insert(out, {
			name = name,
			state = kind,
			detail = label,
			link = if link and link ~= "" then (if kind == "failing" then T.s("checks.see_failed") else T.s("checks.details")) else "",
			on_open = if link and link ~= ""
				then function()
					zeron.open_url(link)
				end
				else nil,
		})
	end
	return out
end

-- The size each `<img>` asked for (its width and height, in points), by
-- its address, as GitHub sets it, for a picture markdown draws.
local asked_size: { [string]: { w: number?, h: number? } } = {}
local function note_size(img: Html.Image)
	if img.w or img.h then
		asked_size[img.src] = { w = img.w, h = img.h }
	end
end

local function is_svg(src: string): boolean
	local lower = string.lower(src)
	return string.match(lower, "%.svg$") ~= nil or string.match(lower, "%.svg[?#]") ~= nil
end

-- How large a picture shows: as its `<img>` asked; an SVG that didn't ask
-- (a bot's "View in …" button) a badge's height, as GitHub draws it at its
-- own; anything else its own size.
local BADGE_H = 28
local function picture_size(src: string): (number?, number?)
	local a = asked_size[src]
	if a then
		return a.w, a.h
	end
	if is_svg(src) then
		return nil, BADGE_H
	end
	return nil, nil
end

-- A picture in GitHub's words, from the web ("network"); anything else stays
-- its alt text.
local function picture(src: string): string?
	return if string.match(src, "^https://") then src else nil
end

-- One of a line of pictures (a bot's buttons): the source for the theme
-- (a `<picture>`'s dark or light), at the size it asked for (or a badge's
-- height for an SVG that didn't, else its own), never past the column; a
-- link opens where links go.
local LinePicture = ui.component(function(props: { img: Html.Image }): Node
	local img = props.img
	local natural = ui.use_state({ w = 0, h = 0 })
	local function src(): string
		local dark = string.find(ui.theme.kind(), "dark", 1, true) ~= nil
		local chosen = (if dark then img.dark else img.light) or img.src
		return picture(chosen) or ""
	end
	local function size(): (number, number)
		local n = natural:get()
		local aspect = if n.h > 0 then n.w / n.h else 0
		if img.w and img.h then
			return img.w, img.h
		elseif img.w then
			return img.w, if aspect > 0 then img.w / aspect else 0
		elseif img.h then
			return img.h * aspect, img.h
		elseif is_svg(img.src) then
			return BADGE_H * aspect, BADGE_H
		end
		return n.w, n.h
	end
	-- (A web picture that didn't come is asked for again by zgx while it
	-- shows.)
	local link = img.href
	return ui.div {
		class = "github-line-picture",
		relative = true,
		shrink = 1,
		min_w = 0,
		max_w = "100%",
		-- (Its own size, as it loads: `on_load` on a point-sized stand-in,
		-- so nothing is decoded at full size to measure it.)
		ui.image {
			src = src,
			aria_hidden = true,
			absolute = true,
			left = 0,
			top = 0,
			w = 1,
			h = 1,
			opacity = 0,
			pointer_events = "none",
			on_load = function(s: { w: number, h: number })
				if s.w > 0 and s.h > 0 and (s.w ~= natural:peek().w or s.h ~= natural:peek().h) then
					natural:set({ w = s.w, h = s.h })
				end
			end,
		},
		ui.div {
			w = function(): number
				local w = size()
				return w
			end,
			h = function(): number
				local _, h = size()
				return h
			end,
			max_w = "100%",
			cursor = if link then "pointer" else "zoom_in",
			role = if link then "link" else "button",
			label = if link then (if img.alt ~= "" then img.alt else nil) else T.s("viewer.pictures.view", { name = if img.alt ~= "" then img.alt else T.s("viewer.pictures.image") }),
			focusable = true,
			-- A link opens where links go; a picture alone opens in the
			-- lightbox, whole.
			on_click = function()
				if link then
					Links.open(link)
					return
				end
				local n = natural:peek()
				local shown = src()
				Lightbox.open({ { src = shown, name = if img.alt ~= "" then img.alt else "image", w = n.w, h = n.h } }, 1, `github:{shown}`)
			end,
			ui.image { src = src, aria_hidden = true, w_full = true, h_full = true, fit = "contain" },
		},
	}
end)

-- A `<details>` block: its summary on a line that opens and closes it (a
-- chevron turning at its start), folded as GitHub folds it unless it stands
-- `open`. What it holds is built as it opens, not before: a bot's folded
-- commit lists and release notes (hundreds of lines and links) cost
-- nothing while folded, which is most of the time.
-- Where a long text is windowed (ds/markdown's `window`): the page's scroll
-- and its offset, so only the blocks near the view are built and laid out
-- (a description of pages, with tables, costs what shows of it).
export type Window = { key: string, view: any, offset: State<number> }
local segments: (list: { Html.Segment }, window: Window?) -> { Node }
local Details = ui.component(function(props: { seg: Html.Segment }): Node
	local seg = props.seg
	local open = ui.use_state(seg.open == true)
	local summary = if seg.summary and seg.summary ~= "" then seg.summary else T.s("viewer.details")
	local bold = if string.find(summary, "**", 1, true) then summary else `**{summary}**`
	local plain = string.gsub(summary, "[%*`_%[%]]", "")
	return ui.column {
		class = "github-details",
		w_full = true,
		min_w = 0,
		gap = "$space.2",
		ui.row {
			w_full = true,
			gap = "$space.1",
			items = "center",
			cursor = "pointer",
			role = "button",
			label = function(): string
				return T.s(if open:get() then "viewer.pictures.hide" else "viewer.pictures.show", { name = plain })
			end,
			expanded = open,
			focusable = true,
			rounded = "$radius.md",
			hover = { bg = "$hover" },
			on_click = function()
				open:set(not open:peek())
			end,
			on_key = function(e: KeyEvent): boolean?
				if (e.key == "Enter" or e.key == " " or e.key == "Space") and not (e.cmd or e.ctrl or e.alt) then
					open:set(not open:peek())
					return true
				end
				return nil
			end,
			-- (Pointing along the line folded, down open: the design
			-- system's, never left rotated, so no layer of its own.)
			ui.div {
				w = 16,
				h = 16,
				shrink = 0,
				items = "center",
				justify = "center",
				git.chevron(function(): number
					return if open:get() then 0 else -90
				end, 14),
			},
			ui.column { grow = 1, min_w = 0, Markdown { text = bold, image = picture, image_size = picture_size } },
		},
		ui.presence(function(): string?
			return if open:get() then "open" else nil
		end, function(): Node
			return ui.column {
				w_full = true,
				min_w = 0,
				gap = "$space.3",
				-- (In from the chevron, under the summary's words.)
				pl = ui.raw(20, "the chevron's box and its gap"),
				table.unpack(segments(seg.body or {})),
			}
		end, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 } }),
	}
end)

-- Markdown as GitHub's words are, pictures and all, set at the size the
-- gallery sets a comment's words: a thread's comment (`md`, ds/comment's) or
-- an activity's note (`sm`, ds/git's). GitHub's HTML read first (html.luau):
-- a line of pictures side by side, a `<sub>` line a step smaller, a
-- `<details>` its summary in bold over what it holds.
function segments(list: { Html.Segment }, window: Window?): { Node }
	local out: { Node } = {}
	for i, seg in list do
		local w = if window then { key = `{window.key}:{i}`, view = window.view, offset = window.offset } else nil
		-- (Windowed, the words are given as a function: ds/markdown windows
		-- only a text that is one, as a transcript's reply is; a plain
		-- string is built whole whatever `window` says.)
		local words = seg.text or ""
		local text: any = if w then function(): string
			return words
		end else words
		if seg.kind == "md" then
			table.insert(out, Markdown { text = text, image = picture, image_size = picture_size, window = w })
		elseif seg.kind == "small" then
			table.insert(out, Markdown { text = text, small = true, image = picture, image_size = picture_size, window = w })
		elseif seg.kind == "images" then
			local row: { Node } = {}
			for _, img in seg.items or {} do
				table.insert(row, LinePicture { img = img })
			end
			table.insert(out, ui.row { class = "github-pictures", w_full = true, flex_wrap = true, gap = "$space.1", items = "center", table.unpack(row) })
		elseif seg.kind == "details" then
			table.insert(out, Details { seg = seg })
		end
	end
	return out
end

-- What GitHub's words were read as, kept by the words: a page built again
-- (its width's layout changing, the pull request read again) reads them
-- once. (A few texts at a time: the pull request's and its comments'.)
local parsed: { [string]: { Html.Segment } } = {}
local parsed_n = 0
local function read(text: string): { Html.Segment }
	local found = parsed[text]
	if found then
		return found
	end
	if parsed_n >= 64 then
		parsed, parsed_n = {}, 0
	end
	local segs = Html.segments(text, note_size)
	parsed[text] = segs
	parsed_n += 1
	return segs
end

local function prose(text: string, size: ("md" | "sm")?, window: Window?): Node
	return ui.column {
		w_full = true,
		min_w = 0,
		gap = "$space.3",
		theme = if size
			then { zeron = { prose = { size = `\{font.size.{size}\}`, line = `\{font.line.{size}\}` } } }
			else nil,
		table.unpack(segments(read(text), window)),
	}
end

-- The box to comment on the pull request as you, and its button: under the
-- verdicts (Summary), and at the foot of what was said (Activity). Sent, it
-- empties at once and the comment stands in Activity (`send` puts it there);
-- if GitHub says no, the words come back with why.
local CommentBox = ui.component(function(props: {
	send: (body: string, failed: (why: string) -> ()) -> (),
	anchor: string?,
	-- A review's verdict too (the Summary's box, a pull request still open
	-- and not yours): "APPROVE" or "REQUEST_CHANGES" with the note, then
	-- `failed` with why if GitHub says no. `busy`: the verdict on its way.
	verdict: ((event: string, body: string, failed: (why: string) -> ()) -> ())?,
	busy: (() -> string?)?,
	narrow: boolean?,
}): Node
	local draft = ui.use_state("")
	local said = ui.use_state(nil :: string?)
	local function fail_with(body: string): (why: string) -> ()
		return function(why: string)
			draft:set(body)
			said:set(why)
		end
	end
	local function send()
		local body = draft:peek()
		if string.match(body, "%S") == nil then
			return
		end
		draft:set("")
		said:set(nil)
		props.send(body, fail_with(body))
	end
	local function empty(): boolean
		return string.match(draft:get(), "%S") == nil
	end
	local function busy(): string?
		return if props.busy then props.busy() else nil
	end
	local narrow = props.narrow == true
	local reviewing = props.verdict ~= nil
	local comment = Button {
		T.t("viewer.comment"),
		tone = if reviewing then "ghost" else nil,
		w_full = narrow and reviewing,
		disabled = empty,
		on_click = send,
	}
	local buttons: { Node } = { comment }
	if reviewing then
		local function verdict(event: string)
			return function()
				local body = draft:peek()
				draft:set("")
				said:set(nil);
				(props.verdict :: any)(event, body, fail_with(body))
			end
		end
		-- (Asking for changes needs words; an approval doesn't.)
		local changes = Button {
			function(): string
				return if busy() == "REQUEST_CHANGES" then T.s("viewer.requesting_changes") else T.s("viewer.request_changes")
			end,
			tone = "secondary",
			w_full = narrow,
			disabled = function(): boolean
				return busy() ~= nil or empty()
			end,
			on_click = verdict("REQUEST_CHANGES"),
		}
		local approve = Button {
			function(): string
				return if busy() == "APPROVE" then T.s("viewer.approving") else T.s("viewer.approve")
			end,
			w_full = narrow,
			disabled = function(): boolean
				return busy() ~= nil
			end,
			on_click = verdict("APPROVE"),
			Icon { name = "check", size = 16 },
		}
		-- (Narrow, the three stack, the way on first: the gallery's.)
		buttons = if narrow then { approve, changes, comment } else { comment, changes, approve }
	end
	local said_line = ui.text {
		function(): string
			local why = said:get()
			return if why then T.s("viewer.couldnt_send", { why = why }) else ""
		end,
		grow = 1,
		min_w = 0,
		wrap = true,
		text_size = "$font.size.sm",
		color = "$danger",
	}
	return ui.column {
		id = props.anchor,
		class = "github-comment-box",
		w_full = true,
		gap = "$space.3",
		TextArea { value = draft, placeholder = T.s(if reviewing then "viewer.leave_review" else "viewer.leave_comment"), name = T.s("viewer.comment"), rows = 3, max_rows = 8, w_full = true, on_submit = send },
		if narrow and reviewing
			then ui.column { w_full = true, gap = "$space.2", said_line, table.unpack(buttons) }
			else ui.row { w_full = true, gap = "$space.2", items = "center", said_line, table.unpack(buttons) },
	}
end)

-- A review thread under its line: the design system's band, the thread's
-- comments in it (each a way to it on GitHub), whether it is resolved, a
-- reply at its foot. Resolving and replying ask GitHub, then the pull
-- request again.
-- A comment's words, or, yours, being edited (a box and Save) or about to
-- go (Delete, said once more): which, as `editing` and `confirming` say.
local CommentBody = ui.component(function(props: {
	id: string,
	body: string,
	editing: State<string>,
	confirming: State<string>,
	draft: State<string>,
	save: (text: string) -> (),
	remove: () -> (),
}): Node
	local function cancel()
		props.editing:set("")
		props.confirming:set("")
	end
	local function save()
		local text = props.draft:peek()
		if string.match(text, "%S") == nil then
			return
		end
		props.editing:set("")
		if text ~= props.body then
			props.save(text)
		end
	end
	return ui.presence(function(): string
		if props.editing:get() == props.id then
			return "edit"
		end
		return if props.confirming:get() == props.id then "confirm" else "read"
	end, function(key: string): Node
		if key == "edit" then
			return ui.column {
				w_full = true,
				gap = "$space.2",
				TextArea { value = props.draft, name = T.s("viewer.edit_comment"), rows = 3, max_rows = 12, w_full = true, autofocus = true, on_submit = save },
				ui.row {
					w_full = true,
					gap = "$space.2",
					items = "center",
					justify = "end",
					Button { T.t("viewer.cancel"), tone = "ghost", size = "sm", on_click = cancel },
					Button {
						T.t("viewer.save"),
						size = "sm",
						disabled = function(): boolean
							return string.match(props.draft:get(), "%S") == nil
						end,
						on_click = save,
					},
				},
			}
		elseif key == "confirm" then
			return ui.column {
				w_full = true,
				gap = "$space.2",
				prose(props.body, "md"),
				ui.row {
					w_full = true,
					gap = "$space.2",
					items = "center",
					ui.text { T.t("viewer.delete_ask"), grow = 1, text_size = "$font.size.sm", color = "$text.secondary" },
					Button { T.t("viewer.cancel"), tone = "ghost", size = "sm", on_click = cancel },
					Button {
						T.t("viewer.delete"),
						label = T.s("viewer.delete_comment"),
						tone = "danger",
						size = "sm",
						on_click = function()
							cancel()
							props.remove()
						end,
					},
				},
			}
		end
		return prose(props.body, "md")
	end, { w_full = true })
end)

local ThreadBlock = ui.component(function(props: {
	t: Thread,
	zeron: any,
	gh: any,
	url: string,
	ctx: git.BlockCtx,
	-- Replies sent and not yet read back, drawn as the thread's own.
	replies: { { body: string, at: number } },
	me: people.Who,
	reply: (text: string, failed: (why: string) -> ()) -> (),
	resolve: (now: boolean, failed: (why: string) -> ()) -> (),
	-- Your comments edited or deleted and not yet read back: their words now,
	-- and those gone.
	edits: { [string]: string },
	deleted: { [string]: boolean },
	edit: (id: string, body: string) -> (),
	remove: (id: string) -> (),
}): Node
	local t = props.t
	local draft = ui.use_state("")
	local resolved = ui.use_state(t.isResolved == true)
	local editing = ui.use_state("")
	local confirming = ui.use_state("")
	local edit_draft = ui.use_state("")
	local comments: { any } = {}
	for _, c in t.comments or {} do
		if c.id and props.deleted[c.id] then
			continue
		end
		local link = c.url
		local body = (c.id and props.edits[c.id]) or c.body or ""
		-- Its corner's actions, drawn by the design system (ds/comment's
		-- Actions: in the hovered comment's own corner).
		local actions: { comment.Action } = {}
		-- Yours: edit it, or delete it (GitHub's own, as you).
		if c.viewerDidAuthor and c.id then
			local id = c.id
			table.insert(actions, {
				icon = "pen",
				label = T.s("viewer.edit"),
				on_click = function()
					edit_draft:set(body)
					confirming:set("")
					editing:set(id)
				end,
			})
			table.insert(actions, {
				icon = "trash-bin-minimalistic",
				label = T.s("viewer.delete"),
				danger = true,
				on_click = function()
					editing:set("")
					confirming:set(id)
				end,
			})
		end
		if link and link ~= "" then
			table.insert(actions, {
				icon = "arrow-up-right",
				label = T.s("viewer.open_github"),
				role = "link",
				on_click = function()
					props.zeron.open_url(link)
				end,
			})
		end
		table.insert(comments, {
			author = who(c.author),
			at = epoch(c.createdAt) or os.time(),
			body = body,
			content = if c.id
				then CommentBody {
					id = c.id,
					body = body,
					editing = editing,
					confirming = confirming,
					draft = edit_draft,
					save = function(text: string)
						props.edit(c.id, text)
					end,
					remove = function()
						props.remove(c.id)
					end,
				}
				else prose(body, "md"),
			actions = actions,
		})
	end
	for _, r in props.replies do
		table.insert(comments, { author = props.me, at = r.at, body = r.body, content = prose(r.body, "md"), actions = {} })
	end
	return ui.div {
		class = "github-thread",
		w_full = true,
		Inline.Thread {
			inset = props.ctx.inset,
			joined = props.ctx.joined,
			location = `{t.path}:{t.line or t.originalLine or "?"}{if t.isOutdated then ` · {T.s("viewer.outdated")}` else ""}`,
			resolved = resolved,
			draft = draft,
			-- (Your face before Reply, as the gallery's review has it.)
			me = props.me,
			comments = comments,
			on_reply = function(text: string)
				props.reply(text, function()
					draft:set(text)
				end)
			end,
			-- (Said at once; set back if GitHub says no.)
			on_resolve = function(now: boolean)
				resolved:set(now)
				props.resolve(now, function()
					resolved:set(not now)
				end)
			end,
		},
	}
end)

-- A unified diff by file: each file's lines from its first hunk on, and
-- the status its header says (a new file, one deleted, one renamed).
local function by_file(diff: string): { [string]: { lines: { git.Line }, status: string, cut: boolean } }
	local raw: { [string]: { string } } = {}
	local status: { [string]: string } = {}
	local current: { string }? = nil
	local path: string? = nil
	for line in string.gmatch(diff .. "\n", "([^\n]*)\n") do
		local named = string.match(line, "^diff %-%-git a/.- b/(.+)$")
		if named then
			path = named
			current = {}
			raw[named] = current :: { string }
			status[named] = "modified"
		elseif current and path then
			if #current == 0 and string.sub(line, 1, 2) ~= "@@" then
				if string.match(line, "^new file mode") then
					status[path] = "added"
				elseif string.match(line, "^deleted file mode") then
					status[path] = "deleted"
				elseif string.match(line, "^rename from") then
					status[path] = "renamed"
				end
			else
				table.insert(current, line)
			end
		end
	end
	local out: { [string]: { lines: { git.Line }, status: string, cut: boolean } } = {}
	for p, lines in raw do
		-- (A diff ends in a newline: no line of its own after it.)
		if lines[#lines] == "" then
			table.remove(lines)
		end
		local parsed = git.parse(lines, MAX_LINES)
		out[p] = { lines = parsed, status = status[p], cut = #lines > MAX_LINES }
	end
	return out
end

return function(zeron: any, pulls: any): (props: { tab: any }) -> Node
	T = zeron
	-- A link in GitHub's words opens where Zeron opens links: this VM's own
	-- copy of the design system's links, set for the plugin's markdown.
	Links.opener = function(url: string): boolean
		zeron.open_url(url)
		return true
	end
	-- Its menus: Open, where Zeron opens links ("links.open"), the only
	-- place it may. Copy link needs "clipboard" in zeron-plugin.json, which
	-- it doesn't ask for: left out until it does.
	Links.system = false
	Links.can_copy = false
	-- People's GitHub pictures: GitHub's own address for each, loaded as any
	-- remote image is ("network"), a face's gradient showing until it's here.
	face_of = function(login: string): () -> string?
		local src = if #login <= 39 and string.match(login, "^%w[%w%-]*$") then `https://github.com/{login}.png?size=96` else nil
		return function(): string?
			return src
		end
	end
	-- Whether the checks block stands open, remembered (as the gallery's).
	local checks_open = ui.persist("github_checks_open", false)
	-- Who you are on GitHub, as its answers to what you send say, kept for
	-- a pull request that doesn't (the engine's `viewerLogin` comes first).
	local me_login = ui.persist("github_me", "")

	return ui.component(function(props: { tab: any }): Node
		local data = props.tab.data or {}
		local url = data.url
		-- GitHub, through gh on this Mac (pulls.luau).
		local gh = pulls
		local detail = ui.use_state(nil :: Detail?)
		local failed = ui.use_state(nil :: string?)
		local loading = ui.use_state(true)
		-- Each pull request read again (after a comment) is a new version:
		-- the page is drawn again for it, where it was.
		local version = ui.use_state(0)
		local tab = ui.use_state("summary")
		local width = ui.use_state(0)
		local scroll = ui.use_state(0)
		-- (The page's scroll, for what's windowed in it.)
		local page_ref = ui.ref()
		local resized_to = 0
		local settle_stop: (() -> ())? = nil
		local locale = ui.shared("zeron.locale", "en")
		-- The Code tab's: the diff (asked once, the first time it shows), the
		-- file chosen (the window's width) and the files open in place (a
		-- pane's), the folders folded, the filter, where its lists stand, the
		-- line a comment is being written on, its words, and what failed.
		local diffs = ui.use_state(nil :: { [string]: any }?)
		local diff_failed = ui.use_state(nil :: string?)
		local asked_diff = false
		local chosen = ui.use_state("")
		local open = ui.use_state({} :: { [string]: boolean })
		local folded = ui.use_state({} :: { [string]: boolean })
		local filter = ui.use_state("")
		local list_offset = ui.use_state(0)
		local pane_offset = ui.use_state(0)
		local list_frame: any = nil
		local pane_frame: any = nil
		local composing = ui.use_state("")
		local new_draft = ui.use_state("")
		local said = ui.use_state(nil :: string?)
		-- The merge section's: how it goes in, whether its branch goes after,
		-- what's on its way to GitHub (merging, closing, a verdict…) and what
		-- GitHub said no to.
		local method = ui.use_state("squash")
		local delete_after = ui.use_state(true)
		local acting = ui.use_state(nil :: string?)
		local act_failed = ui.use_state(nil :: { what: string, why: string }?)
		local after_note = ui.use_state(nil :: string?)
		-- What you sent and GitHub hasn't been read back with yet: each comment
		-- on a line, reply and comment on the pull request stands at once, and
		-- goes as the pull request read again has it (or as GitHub says no).
		local pending = ui.use_state({} :: { any })
		local sent = 0
		-- You, on GitHub: as the pull request says (its `viewerLogin`), else
		-- as GitHub's last answer to you did; not known yet, a skeleton.
		local function me(): people.Who
			local d = detail:peek()
			local login = (d and d.viewerLogin) or me_login:peek()
			if login and login ~= "" then
				return who({ login = login })
			end
			return { id = "you", name = T.s("people.you"), unknown = true }
		end
		local function learn(reply: any)
			local by = type(reply) == "table" and ((reply.author and reply.author.login) or (reply.user and reply.user.login)) or nil
			if type(by) == "string" and by ~= "" and by ~= me_login:peek() then
				me_login:set(by)
			end
		end
		local function hold(entry: any): number
			sent += 1
			entry.seq = sent
			entry.at = os.time()
			local list = table.clone(pending:peek())
			table.insert(list, entry)
			pending:set(list)
			return sent
		end
		local function release(keep: (entry: any) -> boolean)
			local list: { any } = {}
			for _, e in pending:peek() do
				if keep(e) then
					table.insert(list, e)
				end
			end
			if #list ~= #pending:peek() then
				pending:set(list)
			end
		end

		local function load(refresh: boolean)
			if type(url) ~= "string" then
				failed:set(T.s("viewer.no_pr"))
				loading:set(false)
				return
			end
			loading:set(true)
			-- (What was sent before this ask is in what it answers.)
			local upto = sent
			gh.pr(url, refresh, function(kept: Detail)
				-- (What was kept, at once, while GitHub is asked.)
				if not detail:peek() then
					detail:set(kept)
					version:set(version:peek() + 1)
				end
			end):after(function(d: Detail)
				release(function(e: any): boolean
					return e.seq > upto
				end)
				failed:set(nil)
				loading:set(false)
				-- (GitHub saying what was kept already redraws nothing: what
				-- was open a moment ago stays as it stood.)
				local before = detail:peek()
				if before and ui.json.encode(before) == ui.json.encode(d) then
					return
				end
				detail:set(d)
				version:set(version:peek() + 1)
			end, function(err: any)
				failed:set(reason(err))
				loading:set(false)
			end)
		end
		local function refresh()
			load(true)
		end
		-- Something done to the pull request on GitHub (`what` names it while
		-- it's on its way): then the pull request read again; GitHub's no said
		-- where it was asked.
		local function act(what: string, task: any, done: ((value: any) -> ())?)
			if acting:peek() then
				return
			end
			acting:set(what)
			act_failed:set(nil)
			after_note:set(nil)
			task:after(function(value: any)
				if done then
					done(value)
				end
				acting:set(nil)
				load(true)
			end, function(why: any)
				acting:set(nil)
				act_failed:set({ what = what, why = reason(why) })
			end)
		end
		load(false)

		-- Sends `entry` to GitHub with `ask`: held meanwhile, then the pull
		-- request read again; let go with why (`failed`) if GitHub says no.
		local function send(entry: any, ask: any, failed_with: (why: string) -> ())
			local n = hold(entry)
			ask:after(function(reply: any)
				learn(reply)
				refresh()
			end, function(err: any)
				release(function(e: any): boolean
					return e.seq ~= n
				end)
				failed_with(reason(err))
			end)
		end
		local function comment_on_pr(body: string, failed_with: (why: string) -> ())
			send({ kind = "comment", body = body }, gh.comment(url, body), failed_with)
		end

		local function load_diff()
			local d = detail:peek()
			if asked_diff or not d then
				return
			end
			asked_diff = true
			diff_failed:set(nil)
			gh.diff(url, { head = d.headRefOid, base = d.baseRefOid }):after(function(text: any)
				diffs:set(by_file(if type(text) == "string" then text else ""))
			end, function(err: any)
				asked_diff = false
				diff_failed:set(reason(err))
			end)
		end
		ui.effect(function()
			if tab:get() == "code" and detail:get() then
				load_diff()
			end
		end)

		-- The files, as the design system's list keeps them (their status the
		-- diff's, once it's here).
		local function files_of(d: Detail): { Changes.File }
			local known = diffs:peek() or {}
			local out: { Changes.File } = {}
			for _, f in d.files or {} do
				local seen = known[f.path]
				table.insert(out, Changes.file(f.path, f.status or (if seen then seen.status else "modified"), f.additions or 0, f.deletions or 0))
			end
			return out
		end

		-- A file on GitHub, as it is at the pull request's head (or, gone
		-- in it, at its base).
		local function open_file(path: string)
			local d = detail:peek()
			local gone = false
			for _, f in (d and d.files) or {} do
				gone = gone or (f.path == path and f.status == "deleted")
			end
			local link = gh.file_url(url, path, d and (if gone then d.baseRefOid else d.headRefOid))
			if link then
				zeron.open_url(link)
			end
		end

		-- What stands under a line of a file, top to bottom: the threads on
		-- it (GitHub's, on the side they were written on), and the comment
		-- being written. A line is keyed as the design system keys it: the
		-- new side's number, or the old side's for a line taken out.
		local function blocks_under(path: string, key: string): { git.Block }?
			local out: { git.Block } = {}
			local d = detail:get()
			for _, t in (d and d.reviewThreads) or {} do
				if t.path == path and t.line then
					local side = if string.upper(t.diffSide or "RIGHT") == "LEFT" then "o" else "n"
					if `{side}{t.line}` == key then
						local replies: { any } = {}
						local edits: { [string]: string } = {}
						local deleted: { [string]: boolean } = {}
						local mine: { [string]: boolean } = {}
						for _, c in t.comments or {} do
							if c.id then
								mine[c.id] = true
							end
						end
						local moved = {}
						for _, e in pending:get() do
							if e.kind == "reply" and e.thread == t.id then
								table.insert(replies, e)
							elseif e.kind == "edit" and mine[e.id] then
								edits[e.id] = e.body
								table.insert(moved, e.seq)
							elseif e.kind == "delete" and mine[e.id] then
								deleted[e.id] = true
								table.insert(moved, e.seq)
							end
						end
						local left = #replies
						for _, c in t.comments or {} do
							left += if c.id and deleted[c.id] then 0 else 1
						end
						-- (Every comment of it deleted: the thread goes too.)
						if left == 0 then
							continue
						end
						table.insert(out, {
							-- (Its comments, those on their way, what's edited or
							-- gone, and whether it's resolved in its key: one that
							-- moved is a new row, measured again.)
							kind = `thread:{t.id}:{#(t.comments or {})}:{#replies}:{table.concat(moved, ".")}:{t.isResolved == true}`,
							build = function(ctx: git.BlockCtx): Node
								return ThreadBlock {
									t = t,
									zeron = zeron,
									gh = gh,
									url = url,
									ctx = ctx,
									replies = replies,
									me = me(),
									reply = function(text: string, undo: (why: string) -> ())
										said:set(nil)
										send({ kind = "reply", thread = t.id, body = text }, gh.reply(url, t.id, text), function(why: string)
											undo(why)
											said:set(why)
										end)
									end,
									edits = edits,
									deleted = deleted,
									edit = function(id: string, body: string)
										said:set(nil)
										send({ kind = "edit", id = id, body = body }, gh.edit(id, body, true), function(why: string)
											said:set(why)
										end)
									end,
									remove = function(id: string)
										said:set(nil)
										send({ kind = "delete", id = id }, gh.delete(id, true), function(why: string)
											said:set(why)
										end)
									end,
									resolve = function(now: boolean, undo: (why: string) -> ())
										said:set(nil)
										gh.resolve(url, t.id, now):after(function()
											refresh()
										end, function(err: any)
											undo(reason(err))
											said:set(reason(err))
										end)
									end,
								}
							end,
						})
					end
				end
			end
			-- Comments on this line on their way: each its thread to be.
			for _, e in pending:get() do
				if e.kind == "thread" and e.path == path and e.key == key then
					local author, at, body = me(), e.at, e.body
					table.insert(out, {
						kind = `pending:{e.seq}`,
						build = function(ctx: git.BlockCtx): Node
							return ui.div {
								class = "github-thread",
								w_full = true,
								Inline.InlineBlock {
									inset = ctx.inset,
									joined = ctx.joined,
									ui.row { w_full = true, min_w = 0, comment.Location(e.where) },
									comment.CommentCard { author = author, at = at, body = body, content = prose(body, "md"), actions = {} },
								},
							}
						end,
					})
				end
			end
			local at = `{path}|{key}`
			if composing:get() == at then
				local number = tonumber(string.sub(key, 2)) :: number
				local side = if string.sub(key, 1, 1) == "o" then "LEFT" else "RIGHT"
				table.insert(out, {
					kind = "composer",
					build = function(ctx: git.BlockCtx): Node
						return ui.div {
							class = "github-writer",
							w_full = true,
							Inline.Composer {
								inset = ctx.inset,
								joined = ctx.joined,
								reveal = ctx.reveal,
								location = `{path}:{number}{if side == "LEFT" then " (old)" else ""}`,
								value = new_draft,
								placeholder = T.s("viewer.comment_line"),
								submit = T.s("viewer.comment"),
								on_submit = function(text: string)
									composing:set("")
									said:set(nil)
									local now = detail:peek()
									local ask = gh.line_comment(url, { path = path, line = number, side = side, commit = (now and now.headRefOid) or "" }, text)
									send({ kind = "thread", path = path, key = key, where = `{path}:{number}`, body = text }, ask, function(why: string)
										-- (Kept: the words come back where they were written.)
										new_draft:set(text)
										composing:set(at)
										said:set(why)
									end)
								end,
								on_cancel = function()
									composing:set("")
								end,
							},
						}
					end,
				})
			end
			return if #out > 0 then out else nil
		end

		local function lines_of(f: Changes.File): { git.Line }?
			local known = diffs:get()
			local entry = known and known[f.path]
			return if entry then entry.lines else nil
		end

		-- How an open file's diff stands: still coming, failed, nothing to
		-- show, cut short, or all there.
		local function diff_state(f: Changes.File): string
			if diff_failed:get() then
				return "failed"
			end
			local known = diffs:get()
			if not known then
				return "loading"
			end
			local entry = known[f.path]
			if not entry or #entry.lines == 0 then
				return "empty"
			end
			return if entry.cut then "cut" else "ready"
		end
		local function has_foot(f: Changes.File): boolean
			return diff_state(f) ~= "ready"
		end
		-- Under an open file's lines, while there's something to say: that it's
		-- still coming, failed, has nothing to show, or was cut short.
		local function foot(f: Changes.File): Node
			return ui.switch(function(): string
				return diff_state(f)
			end, {
				loading = ui.row { gap = "$space.2", items = "center", progress.ProgressRing { size = 14, tone = "neutral", label = T.s("viewer.loading") }, ui.text { T.t("viewer.code.loading"), text_size = "$font.size.sm", color = "$text.tertiary" } },
				failed = ui.row {
					gap = "$space.2",
					items = "center",
					ui.text { T.t("viewer.code.failed"), text_size = "$font.size.sm", color = "$danger" },
					Button { T.t("viewer.try_again"), tone = "ghost", size = "sm", on_click = load_diff },
				},
				empty = ui.row { gap = "$space.3", items = "center", ui.text { T.t("viewer.code.empty"), text_size = "$font.size.sm", color = "$text.tertiary" }, nil },
				cut = ui.row { gap = "$space.3", items = "center", ui.text { T.s("viewer.code.cut", { n = Changes.count(MAX_LINES) }), text_size = "$font.size.sm", color = "$text.tertiary" } },
				ready = ui.div {},
			} :: any)
		end

		-- The way to a file on GitHub, a quiet icon at the end of a diff card's
		-- head (the window's width).
		local function file_link(path: string): Node
			return IconButton {
				icon = "arrow-up-right",
				label = T.s("viewer.code.open_file", { path = path }),
				role = "link",
				size = "sm",
				tone = "quiet",
				on_click = function()
					open_file(path)
				end,
			}
		end

		-- What failed as you wrote (a comment, a reply, resolving), over the
		-- Code tab until it's put away or the next one lands.
		local function failure(): Node
			return ui.show(function(): boolean
				return said:get() ~= nil
			end, banner.Callout {
				tone = "danger",
				title = T.s("viewer.code.send_failed"),
				body = function(): string
					return said:get() or ""
				end,
				on_dismiss = function()
					said:set(nil)
				end,
			})
		end

		-- Code: the changed files as one list, in their folders, with a filter
		-- and what there is over it. At a pane's width a file opens its diff in
		-- place under its row, threads and all (the list is the one scroll
		-- surface); past SPLIT, the diff of the file chosen stands beside the
		-- list in a card of its own. Both draw only the rows in sight.
		local function code(d: Detail, split: boolean): Node
			local files = files_of(d)
			if #files == 0 then
				return EmptyState { icon = "document", title = T.s("viewer.code.no_files_title"), body = T.s("viewer.code.no_files_body"), size = "sm", fill = true }
			end
			if chosen:peek() == "" then
				chosen:set(files[1].path)
				if not split then
					open:set({ [files[1].path] = true })
				end
			end
			local after = blocks_under
			local function on_comment(path: string, key: string)
				composing:set(`{path}|{key}`)
			end
			if not split then
				return ui.column {
					id = "github-pr-code",
					w_full = true,
					grow = 1,
					min_h = 0,
					gap = "$space.3",
					failure(),
					Changes.panel {
						files = files,
						filter = filter,
						narrow = true,
						folded = folded,
						open = open,
						lines = lines_of,
						foot = foot,
						foot_when = has_foot,
						after = after,
						on_comment = on_comment,
						reveal = {
							offset = list_offset,
							frame = function()
								return list_frame
							end,
						},
						offset = list_offset,
						on_frame = function(el)
							list_frame = el
						end,
						row_class = "github-file",
					},
				}
			end
			local by_path: { [string]: Changes.File } = {}
			for _, f in files do
				by_path[f.path] = f
			end
			-- The chosen file's diff, in its card: its head (and the way to it
			-- on GitHub), then its lines, a slice at a time.
			local diff = ui.presence(function(): string
				return chosen:get()
			end, function(path: string): Node
				local f = by_path[path] or files[1]
				return git.frame {
					fill = true,
					git.diff_head({ path = f.path, status = f.status, add = f.add, del = f.del }, file_link(f.path)),
					git.rule(),
					ui.presence(function(): string
						local known = diffs:get()
						if diff_failed:get() or not known then
							return "waiting"
						end
						local entry = known[f.path]
						return if entry and #entry.lines > 0 then "ready" else "waiting"
					end, function(key: string): Node
						if key == "waiting" then
							return ui.column { w_full = true, px = "$space.3", py = "$space.3", foot(f) }
						end
						return Changes.list {
							id = "github-pr-diff",
							files = { f },
							bare = true,
							columns = 2,
							lines = lines_of,
							foot = foot,
							foot_when = has_foot,
							after = after,
							on_comment = on_comment,
							reveal = {
								offset = pane_offset,
								frame = function()
									return pane_frame
								end,
							},
							offset = pane_offset,
							on_frame = function(el)
								pane_frame = el
							end,
						}
					end, { w_full = true, grow = 1, min_h = 0 }),
				}
			end, { w_full = true, h_full = true, enter = { opacity = { 0, 1 }, duration = 100 }, exit = { opacity = 0, duration = 50 }, mode = "wait" })
			-- The list's card and the diff's start on one line, their heads of
			-- one height, so their first rows share a line.
			return ui.column {
				id = "github-pr-code",
				w_full = true,
				grow = 1,
				min_h = 0,
				gap = "$space.3",
				failure(),
				ui.row {
					w_full = true,
					grow = 1,
					min_h = 0,
					gap = "$space.4",
					items = "stretch",
					ui.column {
						w = ui.raw(300, "the gallery's file list beside a diff"),
						shrink = 0,
						min_h = 0,
						Changes.panel {
							files = files,
							filter = filter,
							folded = folded,
							chosen = chosen,
							on_choose = function(f: Changes.File)
								chosen:set(f.path)
								pane_offset:set(0)
							end,
							offset = list_offset,
							row_class = "github-file",
						},
					},
					ui.column { grow = 1, min_w = 0, min_h = 0, diff },
				},
			}
		end

		-- The verdicts so far: each reviewer's last word (a verdict over a
		-- comment), with who and when.
		local function verdicts(d: Detail): Node?
			local last: { [string]: Comment } = {}
			local order: { string } = {}
			local reviews = table.clone(d.reviews or {})
			table.sort(reviews, function(a: Comment, b: Comment): boolean
				return (a.submittedAt or a.createdAt or "") < (b.submittedAt or b.createdAt or "")
			end)
			for _, r in reviews do
				local login = (r.author and r.author.login) or T.s("people.someone")
				local s = string.upper(r.state or "")
				local held = last[login]
				if not held then
					table.insert(order, login)
				end
				-- (A comment after a verdict leaves the verdict standing.)
				if not held or s ~= "COMMENTED" or string.upper(held.state or "") == "COMMENTED" then
					last[login] = r
				end
			end
			if #order == 0 then
				return nil
			end
			local rows: { Node } = {}
			for _, login in order do
				local r = last[login]
				local s = string.upper(r.state or "")
				local verdict = if s == "APPROVED" then "approved" elseif s == "CHANGES_REQUESTED" then "changes" elseif s == "PENDING" then "pending" else "commented"
				local said_as = T.s(if s == "APPROVED" then "viewer.verdict.approved" elseif s == "CHANGES_REQUESTED" then "viewer.verdict.changes" elseif s == "DISMISSED" then "viewer.verdict.dismissed" else "viewer.verdict.commented")
				local when = ago(epoch(r.submittedAt or r.createdAt))
				table.insert(rows, Settings.Row {
					label = login,
					description = if when ~= "" then T.s("viewer.verdict.when", { verdict = said_as, when = when }) else said_as,
					lead = people.face(who({ login = login }), 32),
					badge = Badge.Review { state = verdict :: any, size = "sm" },
				})
			end
			return Settings.Group(rows :: any)
		end

		-- Whether you may change the pull request: merge it, close it (its
		-- author may close and reopen their own too).
		local function may_write(d: Detail): boolean
			local p = d.repo and d.repo.permission
			return p == "ADMIN" or p == "MAINTAIN" or p == "WRITE"
		end

		-- What stands between the pull request and its base, as GitHub says
		-- it (`mergeStateStatus`): ready, or what's in the way and what clears
		-- it. Its row, and whether a merge can go now.
		local function standing(d: Detail): (Node, boolean)
			local base = d.baseRefName or "main"
			local m = string.upper(d.mergeStateStatus or "")
			local review = string.upper(d.reviewDecision or "")
			local function row(label: string, description: string, icon: string, tone: string?, control: Node?): Node
				return Settings.Row { label = label, description = description, lead = git.glyph(icon, tone), control = control }
			end
			-- A button beside the row's words, while it's on its way saying so.
			local function act_button(what: string, label: string, busy_label: string, task: () -> any): Node
				return Button {
					function(): string
						return if acting:get() == what then busy_label else label
					end,
					tone = "secondary",
					size = "sm",
					disabled = function(): boolean
						return acting:get() ~= nil
					end,
					on_click = function()
						act(what, task())
					end,
				}
			end
			if d.isDraft or m == "DRAFT" then
				local control = if may_write(d) or d.viewerDidAuthor
					then act_button("ready", T.s("viewer.merge.ready"), T.s("viewer.merge.marking_ready"), function(): any
						return gh.ready(d.id or "", true)
					end)
					else nil
				return row(T.s("viewer.merge.draft_title"), T.s("viewer.merge.draft_body"), "pen", nil, control), false
			elseif string.upper(d.mergeable or "") == "CONFLICTING" or m == "DIRTY" then
				return row(T.s("viewer.merge.conflicts_title", { base = base }), T.s("viewer.merge.conflicts_body", { base = base }), "danger-triangle", "danger"), false
			elseif m == "BEHIND" then
				local control = if may_write(d)
					then act_button("update", T.s("viewer.merge.update"), T.s("viewer.merge.updating"), function(): any
						return gh.update_branch(d.id or "")
					end)
					else nil
				return row(T.s("viewer.merge.behind_title", { base = base }), T.s("viewer.merge.behind_body", { base = base }), "clock-circle", "warning", control), false
			elseif m == "BLOCKED" then
				local why: { string } = {}
				if review == "CHANGES_REQUESTED" then
					table.insert(why, T.s("viewer.merge.why.changes"))
				elseif review == "REVIEW_REQUIRED" then
					table.insert(why, T.s("viewer.merge.why.review"))
				end
				local ci = (d.ci and d.ci.state) or ""
				if ci == "failed" then
					table.insert(why, T.s("viewer.merge.why.check_failed"))
				elseif ci == "pending" then
					table.insert(why, T.s("viewer.merge.why.checks_running"))
				end
				local said = if #why > 0 then table.concat(why, T.s("viewer.merge.why.join")) else T.s("viewer.merge.why.rules")
				return row(T.s("viewer.merge.not_yet_title"), T.s("viewer.merge.not_yet_body", { said = said }), "clock-circle", "warning"), false
			elseif m == "UNKNOWN" or m == "" then
				return row(T.s("viewer.merge.checking_title"), T.s("viewer.merge.checking_body"), "clock-circle", nil), false
			elseif m == "UNSTABLE" then
				return row(T.s("viewer.merge.ready_title"), T.s("viewer.merge.unstable_body"), "check-circle", "warning"), true
			end
			-- Ready: who approved it, and that nothing's in the way.
			local approvers: { string } = {}
			local seen: { [string]: boolean } = {}
			for _, r in d.reviews or {} do
				local login = r.author and r.author.login
				if login and string.upper(r.state or "") == "APPROVED" and not seen[login] then
					seen[login] = true
					table.insert(approvers, login)
				end
			end
			local parts: { string } = {}
			if #approvers > 0 then
				local names = if #approvers == 1 then approvers[1] else table.concat(approvers, T.s("viewer.merge.names_join"), 1, #approvers - 1) .. T.s("viewer.merge.names_last") .. approvers[#approvers]
				table.insert(parts, T.s("viewer.merge.approved_by", { names = names }))
			end
			if ((d.ci and d.ci.state) or "") == "passed" then
				table.insert(parts, T.s("viewer.merge.every_check"))
			end
			table.insert(parts, T.s("viewer.merge.no_conflicts", { base = base }))
			local text = table.concat(parts, T.s("viewer.merge.parts_join"))
			-- (Its first letter raised: the first part starts the sentence.)
			local first = utf8.char(utf8.codepoint(text, 1))
			return row(T.s("viewer.merge.ready_title"), string.upper(first) .. string.sub(text, #first + 1) .. T.s("viewer.merge.ready_end"), "check-circle", "success"), true
		end

		-- The merge section: where it stands, how it goes in, what becomes of
		-- its branch, and the button (the gallery's merge page); closing, and
		-- back to a draft, beside it. Merged or closed, what happened (and,
		-- closed, reopening).
		local function merge_section(d: Detail, narrow: boolean): Node
			local base = d.baseRefName or "main"
			local state = string.upper(d.state or "OPEN")
			local TITLED = { merge = true, close = true, reopen = true, draft = true, ready = true, update = true }
			-- What GitHub said no to, under where it was asked.
			local function failure(): Node
				return ui.show(function(): boolean
					return act_failed:get() ~= nil
				end, banner.Callout {
					tone = "danger",
					title = function(): string
						local f = act_failed:get()
						return if f then T.s(if TITLED[f.what] then `viewer.merge.couldnt.{f.what}` else "viewer.merge.couldnt.other") else ""
					end,
					body = function(): string
						local f = act_failed:get()
						return if f then f.why else ""
					end,
				})
			end
			if d.merged or state == "MERGED" then
				local by = d.mergedBy and d.mergedBy.login
				local when = ago(epoch(d.mergedAt))
				local body = if by and when ~= "" then T.s("viewer.merge.merged_by_when", { by = by, when = when }) elseif by then T.s("viewer.merge.merged_by", { by = by }) elseif when ~= "" then T.s("viewer.merge.merged_when", { when = when }) else T.s("viewer.merge.merged_plain")
				return ui.column {
					id = "github-pr-merge",
					w_full = true,
					gap = "$space.3",
					banner.Callout { tone = "success", icon = "git-merge", title = T.s("viewer.merge.merged_title", { number = d.number, base = base }), body = body },
					ui.show(function(): boolean
						return after_note:get() ~= nil
					end, ui.text {
						function(): string
							return after_note:get() or ""
						end,
						text_size = "$font.size.sm",
						color = "$text.tertiary",
						wrap = true,
					}),
				}
			elseif state == "CLOSED" then
				local when = ago(epoch(d.closedAt))
				local may = may_write(d) or d.viewerDidAuthor == true
				return ui.column {
					id = "github-pr-merge",
					w_full = true,
					gap = "$space.3",
					banner.Callout {
						tone = "neutral",
						icon = "close-circle",
						title = T.s("viewer.merge.closed_title"),
						body = if when ~= "" then T.s("viewer.merge.closed_when", { when = when }) else T.s("viewer.merge.closed_plain"),
						action = if may
							then {
								label = T.s("viewer.merge.reopen"),
								on_click = function()
									act("reopen", gh.reopen(d.id or ""))
								end,
							}
							else nil,
					},
					failure(),
				}
			end
			local row, can = standing(d)
			local writer = may_write(d)
			-- Under its own title (the Summary's third): where it stands…
			local parts: { Node } = { Settings.Section { title = T.s("viewer.merge.title"), Settings.Group { row } } }
			if writer then
				-- The ways this repository allows, the first chosen unless one
				-- already is.
				local n = d.commitCount or 0
				local options: { any } = {}
				local repo = d.repo or {}
				if repo.squash ~= false then
					table.insert(options, { value = "squash", label = T.s("viewer.merge.squash"), description = T.s("viewer.merge.squash_body", { n = n, base = base }) })
				end
				if repo.merge ~= false then
					table.insert(options, { value = "merge", label = T.s("viewer.merge.merge_commit"), description = T.s("viewer.merge.merge_commit_body") })
				end
				if repo.rebase ~= false then
					table.insert(options, { value = "rebase", label = T.s("viewer.merge.rebase"), description = T.s("viewer.merge.rebase_body", { base = base }) })
				end
				local allowed = false
				for _, o in options do
					allowed = allowed or o.value == method:peek()
				end
				if not allowed and options[1] then
					method:set(options[1].value)
				end
				local how: { any } = {
					ui.column {
						w_full = true,
						px = "$space.4",
						py = "$space.4",
						-- (Chosen ahead, while it can't go in yet: the button alone waits.)
						RadioGroup { value = method, label = T.s("viewer.merge.how"), options = options },
					},
				}
				-- (A fork's branch is the fork's to delete.)
				if not d.isCrossRepository and d.headRefName then
					table.insert(how, Settings.Row {
						label = T.s("viewer.merge.delete_branch"),
						description = T.s("viewer.merge.delete_branch_body", { branch = d.headRefName }),
						control = Switch { value = delete_after, label = T.s("viewer.merge.delete_branch") },
					})
				end
				table.insert(parts, Settings.Section { title = T.s("viewer.merge.how"), Settings.Group(how :: any) })
			end
			table.insert(parts, failure())
			-- The button, and closing (and back to a draft) beside it.
			local merge = if writer
				then Button {
					function(): string
						if acting:get() == "merge" then
							return T.s("viewer.merge.merging")
						end
						local m = method:get()
						return T.s(if m == "squash" then "viewer.merge.squash" elseif m == "rebase" then "viewer.merge.rebase" else "viewer.merge.merge_pr")
					end,
					disabled = function(): boolean
						return not can or acting:get() ~= nil
					end,
					w_full = narrow,
					on_click = function()
						local deleting = delete_after:peek() and not d.isCrossRepository
						act("merge", gh.merge(d.url or url, d, method:peek(), deleting), function(result: any)
							if deleting and type(result) == "table" and result.deleted == false then
								after_note:set(T.s("viewer.merge.branch_kept", { branch = d.headRefName, why = result.why or T.s("gh.said_no") }))
							end
						end)
					end,
					Icon { name = "git-merge", size = 16 },
				}
				else nil
			-- The button (the gallery's: alone at the end of the line, full
			-- width at a pane's), what's on its way beside it.
			if merge then
				local busy = ui.show(function(): boolean
					return acting:get() ~= nil
				end, ProgressRing { size = 16, tone = "neutral", label = T.s("viewer.merge.working") })
				if narrow then
					table.insert(parts, ui.column { w_full = true, gap = "$space.2", items = "center", busy, merge })
				else
					table.insert(parts, ui.row { w_full = true, justify = "end", gap = "$space.2", items = "center", busy, merge })
				end
			end
			return ui.column { id = "github-pr-merge", w_full = true, gap = "$space.4", table.unpack(parts) }
		end

		-- Summary: where it stands, and the way to say so. A failure is said
		-- once, by the checks block, which then comes first: it is what a
		-- reviewer needs before the description.
		local function summary(d: Detail, narrow: boolean): Node
			local list = checks_of(zeron, d)
			local any_failed = false
			for _, c in list do
				any_failed = any_failed or c.state == "failing"
			end
			local ci = (d.ci and d.ci.state) or ""
			local block = ui.column {
				id = "github-pr-checks",
				w_full = true,
				git.checks_block {
					list = list,
					open = checks_open,
					summary = if #list == 0 then T.s(if ci == "noChecks" then "checks.none" else "checks.unavailable") else nil,
					empty = T.s(if ci == "noChecks" then "checks.none_body" else "checks.unavailable_body"),
				},
			}
			local body = d.body or ""
			local description = Settings.Section {
				title = T.s("viewer.description"),
				git.frame {
					id = "github-pr-description",
					padded = true,
					if string.match(body, "%S") then prose(body, nil, { key = `pr:{url}:body`, view = page_ref, offset = scroll }) else ui.text { T.t("viewer.no_description"), text_size = "$font.size.md", color = "$text.tertiary" },
				},
			}
			return ui.column {
				w_full = true,
				gap = "$space.5",
				if any_failed then block else description,
				if any_failed then description else block,
				(function(): Node
					-- (The verdicts, when there are any, over the box: no hole in
					-- the list a section's children are read from.)
					local review: { any } = { title = T.s("viewer.review") }
					local said_so = verdicts(d)
					if said_so then
						table.insert(review, said_so)
					end
					-- (A verdict while it's open, on someone else's: GitHub
					-- takes none on your own.)
					local open = string.upper(d.state or "OPEN") == "OPEN"
					table.insert(review, CommentBox {
						send = comment_on_pr,
						anchor = "github-pr-review",
						narrow = narrow,
						verdict = if open and not d.viewerDidAuthor
							then function(event: string, body: string, failed_with: (why: string) -> ())
								if acting:peek() then
									failed_with(T.s("viewer.busy"))
									return
								end
								acting:set(event)
								gh.review(d.id or "", event, body):after(function()
									acting:set(nil)
									load(true)
								end, function(why: any)
									acting:set(nil)
									failed_with(reason(why))
								end)
							end
							else nil,
						busy = function(): string?
							return acting:get()
						end,
					})
					return Settings.Section(review :: any)
				end)(),
				merge_section(d, narrow),
			}
		end

		-- What a review did, as Zeron's viewer says it.
		local VERB: { [string]: string } = {
			APPROVED = "viewer.activity.verbs.approved",
			CHANGES_REQUESTED = "viewer.activity.verbs.changes",
			COMMENTED = "viewer.activity.verbs.reviewed",
			DISMISSED = "viewer.activity.verbs.dismissed",
		}
		local VERDICT: { [string]: string } = { APPROVED = "approved", CHANGES_REQUESTED = "changes" }

		local function events_of(d: Detail): { git.Event }
			local events: { git.Event } = {}
			-- Review threads' comments: where each sits (a link to it on
			-- GitHub), resolved or not, outdated, a reply (pull_request_detail.rs).
			for _, t in d.reviewThreads or {} do
				for _, c in t.comments or {} do
					local where = { `{t.path}:{t.line or t.originalLine or "?"}`, T.s(if t.isResolved then "viewer.activity.resolved" else "viewer.activity.unresolved") }
					if t.isOutdated then
						table.insert(where, T.s("viewer.activity.outdated"))
					end
					if c.replyTo then
						table.insert(where, T.s("viewer.activity.reply"))
					end
					local at = table.concat(where, T.s("viewer.activity.where_join"))
					local link = c.url
					table.insert(events, {
						kind = "comment",
						at = epoch(c.createdAt) or os.time(),
						who = who(c.author),
						body = c.body or "",
						content = prose(c.body or "", "sm"),
						where = at,
						where_label = T.s("viewer.activity.open_where", { at = at }),
						on_where = if link and link ~= ""
							then function()
								zeron.open_url(link)
							end
							else nil,
					})
				end
			end
			for _, c in d.comments or {} do
				table.insert(events, { kind = "comment", at = epoch(c.createdAt) or os.time(), who = who(c.author), body = c.body or "", content = if c.body and c.body ~= "" then prose(c.body, "sm") else nil })
			end
			for _, r in d.reviews or {} do
				local s = string.upper(r.state or "")
				table.insert(events, {
					kind = "review",
					at = epoch(r.submittedAt or r.createdAt) or os.time(),
					who = who(r.author),
					verdict = VERDICT[s] or "commented",
					verb = T.s(VERB[s] or "viewer.activity.verbs.reviewed"),
					body = r.body or "",
					content = if r.body and r.body ~= "" then prose(r.body, "sm") else nil,
				})
			end
			table.sort(events, function(a: git.Event, b: git.Event): boolean
				return a.at < b.at
			end)
			return events
		end

		-- Activity: what was said and decided, in order, and the box to add
		-- to it (production's dock under the list).
		local function activity(d: Detail): Node
			-- What was said, and what you've said that's on its way, at its end.
			local said_now = ui.presence(function(): string
				local n = 0
				for _, e in pending:get() do
					n += if e.kind == "comment" then 1 else 0
				end
				return tostring(n)
			end, function(): Node
				local events = events_of(d)
				for _, e in pending:peek() do
					if e.kind == "comment" then
						table.insert(events, { kind = "comment", at = e.at, who = me(), body = e.body, content = prose(e.body, "sm") })
					end
				end
				if #events == 0 then
					return EmptyState {
						icon = "chat-round-line",
						title = T.s("viewer.activity.empty_title"),
						body = T.s("viewer.activity.empty_body"),
						size = "sm",
					}
				end
				return git.activity(events, { branch = d.headRefName, base = d.baseRefName })
			end, { w_full = true })
			return ui.column {
				id = "github-pr-activity",
				w_full = true,
				gap = "$space.6",
				said_now,
				CommentBox { send = comment_on_pr },
			}
		end

		-- The header's end: ask again, and the pull request on GitHub.
		-- The header's actions: refresh, GitHub, and (yours to change) a
		-- menu of what else can be done to it: a draft marked ready or the
		-- reverse, closing, reopening.
		local function more_menu(d: Detail): Node?
			local p = d.repo and d.repo.permission
			local writer = p == "ADMIN" or p == "MAINTAIN" or p == "WRITE"
			if not (writer or d.viewerDidAuthor) then
				return nil
			end
			local state = string.upper(d.state or "OPEN")
			if d.merged or state == "MERGED" then
				return nil
			end
			local open = ui.use_state(false)
			local trigger = IconButton {
				icon = "more-horizontal",
				label = T.s("viewer.more"),
				size = "sm",
				tone = "quiet",
				lit = open,
				on_click = function()
					open:set(true)
				end,
			}
			local function item(icon: string, label: string, what: string, task: () -> any, danger: boolean?): Node
				-- (Built as the menu opens: whether something's on its way is
				-- read then.)
				return Menu.Row {
					icon = icon :: any,
					label = label,
					danger = danger,
					role = "menuitem",
					disabled = acting:peek() ~= nil,
					on_click = function()
						open:set(false)
						act(what, task())
					end,
				}
			end
			return ui.popover(Glass.style({
				open = open,
				anchor = trigger,
				label = T.s("viewer.more"),
				placement = { side = "bottom", align = "end", offset = 4 },
				w = ui.theme.get("zeron.menu.width") :: number?,
				p = 0,
				gap = 0,
				rounded = "$radius.lg",
				build = function(): Node
					local rows: { Node } = {}
					if state == "CLOSED" then
						table.insert(rows, item("restart", T.s("viewer.menu.reopen"), "reopen", function(): any
							return gh.reopen(d.id or "")
						end))
					else
						if d.isDraft then
							table.insert(rows, item("check-circle", T.s("viewer.menu.ready"), "ready", function(): any
								return gh.ready(d.id or "", true)
							end))
						else
							table.insert(rows, item("pen", T.s("viewer.menu.draft"), "draft", function(): any
								return gh.ready(d.id or "", false)
							end))
						end
						table.insert(rows, item("close-circle", T.s("viewer.menu.close"), "close", function(): any
							return gh.close(d.id or "")
						end, true))
					end
					return Menu.Group(rows :: any)
				end,
			}, "menu"))
		end

		local function actions(d: Detail): Node
			local more = more_menu(d)
			return ui.row {
				gap = "$space.1",
				items = "center",
				shrink = 0,
				IconButton {
					icon = "refresh",
					label = T.s("viewer.refresh"),
					size = "sm",
					tone = "quiet",
					on_click = refresh,
				},
				IconButton {
					icon = "arrow-up-right",
					label = T.s("viewer.open_github"),
					role = "link",
					size = "sm",
					tone = "quiet",
					on_click = function()
						zeron.open_url(d.url or url)
					end,
				},
				more,
			}
		end

		-- The page for its width (`narrow`, `split`) and its tab: the header,
		-- the tabs and what the tab shows. Code keeps to the panel's height
		-- and scrolls in its parts; the others scroll as one.
		local function page(d: Detail, narrow: boolean, split: boolean, fixed: boolean): Node
			local list = checks_of(zeron, d)
			local passed, failing, running = 0, false, false
			for _, c in list do
				passed += if c.state == "passing" then 1 else 0
				failing = failing or c.state == "failing"
				running = running or c.state == "running"
			end
			local events = events_of(d)
			local badges: { Node } = {}
			if #list > 0 then
				table.insert(badges, Badge.Checks { state = (if failing then "failing" elseif running then "running" else "passing") :: any, passed = passed, total = #list, size = "md" })
			end
			-- (Where its review stands, while there's one to give: merged or
			-- closed, a decision still to come means nothing.)
			if state_of(d) == "open" or state_of(d) == "draft" or review_of(d) ~= "pending" then
				table.insert(badges, Badge.Review { state = review_of(d) :: any, size = "md" })
			end
			local created = epoch(d.createdAt)
			local head = git.header({
				state = state_of(d),
				number = d.number,
				title = d.title,
				author = who(d.author),
				opened = if created then ago(created) else nil,
				branch = d.headRefName,
				base = d.baseRefName,
				added = d.additions or 0,
				removed = d.deletions or 0,
				actions = actions(d),
			}, narrow)
			-- Its words on the cards' words (the sections' text edge), the
			-- cards hanging past: the header, the first tab's label, the
			-- badges' words.
			local space1 = (ui.theme.get("space.1") :: number?) or 4
			local badge_pad = (ui.theme.get("zeron.space.tight") :: number?) or 6
			head = ui.div { id = "github-pr-head", w_full = true, px = Settings.inset(), head }
			local tabs = ui.div {
				id = "github-pr-tabs",
				w_full = true,
				git.tabs {
					value = tab,
					narrow = narrow,
					inset = Settings.inset(space1),
					trail_inset = Settings.inset(badge_pad),
					options = {
						{ value = "summary", label = T.s("viewer.tabs.summary") },
						{ value = "code", label = T.s("viewer.tabs.code"), count = #(d.files or {}) },
						{ value = "activity", label = T.s("viewer.tabs.activity"), count = #events },
					},
					badges = badges,
				},
			}
			-- The gallery's gutters: a side pane's tighter than the window's.
			local px = if narrow then "$space.4" else "$space.6"
			if fixed then
				return ui.column {
					w_full = true,
					h_full = true,
					px = px,
					pt = "$space.4",
					pb = "$space.4",
					gap = "$space.5",
					head,
					tabs,
					code(d, split),
				}
			end
			-- Summary and Activity come and go under the tabs.
			local content = ui.presence(function(): string
				return tab:get()
			end, function(t: string): Node
				return if t == "activity" then activity(d) else summary(d, narrow)
			end, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" })
			return ui.scroll {
				ref = page_ref,
				w_full = true,
				h_full = true,
				offset = scroll,
				fade = { 16, 16 },
				fade_overflow = true,
				-- (The pane's whole width, as the Code tab takes it and the
				-- gallery's pages do: the header and the tabs stand still as
				-- the tabs change.)
				ui.column {
					w_full = true,
					px = px,
					pt = "$space.4",
					pb = "$space.6",
					gap = "$space.5",
					head,
					tabs,
					content,
				},
			}
		end

		-- What the tab shows: the pull request as it's being asked for, what
		-- went wrong (and a way to ask again), or its page.
		local PAGES: { [string]: () -> Node } = {
			loading = function(): Node
				return ui.column {
					w_full = true,
					h_full = true,
					items = "center",
					justify = "center",
					gap = "$space.3",
					progress.ProgressRing { size = 20, tone = "neutral", label = T.s("viewer.loading") },
					ui.text { T.t("viewer.loading_pr"), text_size = "$font.size.sm", color = "$text.tertiary" },
				}
			end,
			failed = function(): Node
				return ui.column {
					w_full = true,
					p = "$space.4",
					banner.Callout {
						tone = "danger",
						title = T.s("viewer.failed_title"),
						body = function(): string
							return failed:get() or ""
						end,
						action = {
							label = T.s("viewer.try_again"),
							on_click = function()
								load(true)
							end,
						},
					},
				}
			end,
			-- The page for the width it has and the tab it shows, drawn again
			-- for each version of the pull request.
			ready = function(): Node
				return ui.presence(function(): string
					local w = width:get()
					local narrow = w > 0 and w < NARROW
					local split = w >= SPLIT
					-- (And once the diff is here: the files' statuses are its.)
					-- (The Code tab follows a version in place: its threads read the
					-- pull request as it changes, so a comment lands where it was
					-- written and nothing scrolls.)
					local code = tab:get() == "code"
					-- (And the language shown: its words are built in.)
					local l = locale:get()
					return `{if code then "code" else version:get()}|{narrow}|{split}|{diffs:get() ~= nil}|{if type(l) == "string" then l else "en"}`
				end, function(): Node
					local d = detail:peek()
					if not d then
						return ui.div {}
					end
					local w = width:peek()
					return page(d, w > 0 and w < NARROW, w >= SPLIT, tab:peek() == "code")
				end, { w_full = true, h_full = true })
			end,
		}

		return ui.column {
			class = "github-pr",
			w_full = true,
			h_full = true,
			-- (Its words can be selected and copied, as a chat's can.)
			selectable = true,
			on_resize = function(size)
				-- (Whole points: a sub-point wobble is no new width. And no
				-- width at all is no width: the panel closing lays its content
				-- out at 0, and the page, built for the last width it had,
				-- stays as it is. Resized on and on, as an edge is dragged,
				-- the width the page is built for (narrow, split: a different
				-- layout, built again whole) is taken once the resizing rests
				-- for a moment, never mid-drag: the text reflows live, the
				-- layout changes once.)
				local w = math.floor(size.w)
				if w <= 0 then
					return
				end
				resized_to = w
				if width:peek() == 0 then
					width:set(w)
					return
				end
				if settle_stop then
					settle_stop()
				end
				settle_stop = ui.after(220, function()
					settle_stop = nil
					if resized_to ~= width:peek() then
						width:set(resized_to)
					end
				end)
			end,
			ui.presence(function(): string
				if detail:get() then
					return "ready"
				end
				return if failed:get() then "failed" else "loading"
			end, function(key: string): Node
				return PAGES[key]()
			end, { w_full = true, h_full = true }),
			-- The design system's lightbox, for a picture clicked in what was
			-- said (each plugin's own: it draws over the window).
			Lightbox.Host(),
			-- A link's context menu in what was said (ds/menu_at: the
			-- design system's, this VM's own host).
			MenuAt.Host {},
		}
	end)
end
