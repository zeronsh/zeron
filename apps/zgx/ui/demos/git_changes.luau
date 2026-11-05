-- The changed files of a commit or a pull request as one list, drawn for
-- the worst case: a thousand files. Zeron's own Changes panel is a single
-- virtualized list at line granularity (changes.rs: every file header,
-- hunk header and line a row of its own; a folded file keeps no body
-- rows), and this does the same with `ui.list`, so a thousand files cost
-- what the visible slice costs. The list stands in a card of its own with
-- what there is as the card's head (the count, the lines, a commit's
-- stage-all box) and the filter under it; in the list, the folders as
-- headers that fold, each file a row; at a pane's width a file opens its
-- diff in place under its row (one scroll surface: the list never pushes
-- anything else away), and at the window's width a file is chosen for the
-- diff beside the list. The card's scrollbar is the app's rail, over the
-- rows while they scroll and gone otherwise.
--
-- `Changes.panel { files, filter, lead, extra, narrow, folded, open,
-- chosen, on_choose, box, after, on_comment, reveal, offset, on_frame }`
-- is the card; `Changes.list { … }` the list alone; `Changes.files(many)`
-- the files, four or a thousand.
--
-- The list itself is the design system's (ds/git_changes), which the GitHub
-- plugin draws from too; this file is the journey's files (four, or a
-- thousand) and each open file's diff (badge.luau's two hunks).

local Changes = require("@ds/git_changes")
local git = require("@demos/git_parts")

local M = {}

-- Whether the pages show a thousand files (the demo's switch, in the Git
-- window beside the width's).
M.many = git.many

M.HEAD, M.GROUP, M.FILE, M.HUNK, M.LINE = Changes.HEAD, Changes.GROUP, Changes.FILE, Changes.HUNK, Changes.LINE

export type File = Changes.File
export type ListProps = Changes.ListProps
export type Panel = Changes.Panel

M.totals = Changes.totals
M.count = Changes.count

-- A thousand files, the same every time: real-looking paths over the
-- repository's own folders, each with a status and its lines.
local DIRS = {
	"crates/ui/src",
	"crates/ui/src/settings",
	"crates/proto/src",
	"crates/engine/src",
	"crates/update/src",
	"apps/zgx/ui/ds",
	"apps/zgx/ui/demos",
	"apps/zgx/ui/screens",
	"edge/src",
	"edge/src/rooms",
	"apps/ios/Zeron/Views",
	"apps/android/app/src/main",
	"docs",
}
local EXT: { [string]: string } = {
	["crates/ui/src"] = "rs",
	["crates/ui/src/settings"] = "rs",
	["crates/proto/src"] = "rs",
	["crates/engine/src"] = "rs",
	["crates/update/src"] = "rs",
	["apps/zgx/ui/ds"] = "luau",
	["apps/zgx/ui/demos"] = "luau",
	["apps/zgx/ui/screens"] = "luau",
	["edge/src"] = "ts",
	["edge/src/rooms"] = "ts",
	["apps/ios/Zeron/Views"] = "swift",
	["apps/android/app/src/main"] = "kt",
	["docs"] = "md",
}
local NAMES = {
	"badge", "tokens", "sidebar", "button", "dialog", "select", "list", "scroll", "markdown", "comment",
	"activity", "progress", "tabs", "text_field", "toast", "theme", "icon", "layout", "people", "status",
	"session", "composer", "transcript", "shell", "settings", "onboarding", "changes", "pull_request",
	"mesh", "relay", "room", "sync", "update", "auth", "device", "project", "search", "keymap", "fonts", "glass",
}

export type File = { path: string, dir: string, name: string, status: string, add: number, del: number }

local function thousand(): { File }
	local out: { File } = {}
	for i = 1, 1000 do
		local dir = DIRS[(i * 7) % #DIRS + 1]
		local base = NAMES[(i * 13) % #NAMES + 1]
		local variant = i // #NAMES
		local name = `{if variant == 0 then base else `{base}_{variant}`}.{EXT[dir]}`
		local r = (i * 31) % 100
		local status = if r < 72 then "modified" elseif r < 92 then "added" else "deleted"
		local add = if status == "deleted" then 0 else (i * 37) % 400 + 1
		local del = if status == "added" then 0 else (i * 17) % 120 + 1
		table.insert(out, { path = `{dir}/{name}`, dir = dir, name = name, status = status, add = add, del = del })
	end
	return out
end

local FEW: { File } = {}
for _, f in git.FILES do
	local dir, name = string.match(f.path, "^(.*)/([^/]+)$")
	table.insert(FEW, { path = f.path, dir = dir or "", name = name or f.path, status = f.status, add = f.add, del = f.del })
end
local MANY = thousand()

-- The files: four, or a thousand.
function M.files(many: boolean): { File }
	return if many then MANY else FEW
end

-- Every open file's diff is badge.luau's, as the pages have it.
local function with_lines<T>(props: T): T
	local p = table.clone(props :: any)
	if not p.lines then
		p.lines = function(): { git.Line }
			return git.HUNK
		end
	end
	return p :: any
end

function M.list(props: ListProps): Node
	return Changes.list(with_lines(props))
end

function M.panel(props: Panel): Node
	return Changes.panel(with_lines(props))
end

return M
