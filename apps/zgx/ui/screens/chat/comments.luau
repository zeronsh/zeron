-- Review comments left on a file (the files pane's editor, a block of its
-- preview) or on a diff (a plugin's: Git's Changes, through
-- `zeron.compose_comment`), carried on the composer and sent after the
-- words as Zeron's composer sends them (crates/ui/src/comments.rs
-- `with_comments`):
--
--   <the words, or "Address the review comments below." with none>
--
--   Review comments (each cites the workspace file and line it belongs to):
--   - src/app.rs:12: <its words, a further line set in two>
--
-- When every comment is on a diff, the diff's header, and each its side
-- (L: the line in the original file, R: in the changed one):
--
--   Comments on the diff (each cites the file and line it belongs to; L = …):
--   - src/app.rs:12 (R): <its words>
--
-- A range of lines cites its first and last (`src/app.rs:12-14 (R): …`), and
-- the code it's on follows its words, fenced, set in two as a further line
-- is (production's reader takes it as part of the words).
--
-- The host parts the block back off the sent message (src/chat.rs
-- `split_comments`) and shows it as one chip ("2 comments").

local M = {}

M.HEADER = "Review comments (each cites the workspace file and line it belongs to):"
M.DIFF_HEADER = "Comments on the diff (each cites the file and line it belongs to; L = line number in the original file, R = in the changed file):"
M.ONLY = "Address the review comments below."

-- The most of a comment's code that goes with it.
local SNIPPET_LINES, SNIPPET_BYTES = 40, 4000

export type Comment = {
	file: string,
	line: number,
	body: string,
	-- A range's last line (more than `line`).
	to: number?,
	-- On a diff: "L" (the original file's line) or "R" (the changed one's).
	side: string?,
	-- The code it's on, as the lines read.
	snippet: string?,
}

-- Where a comment is, as it's cited: the project's path and the line (or
-- the range's first and last).
function M.place(c: Comment): string
	if c.to and c.to > c.line then
		return `{c.file}:{c.line}-{c.to}`
	end
	return `{c.file}:{c.line}`
end

-- The code, at most so much of it, fenced (a fence longer than any run of
-- backticks in it).
local function fenced(code: string): string
	local lines: { string } = {}
	local bytes = 0
	for line in string.gmatch(code .. "\n", "([^\n]*)\n") do
		if #lines >= SNIPPET_LINES or bytes + #line > SNIPPET_BYTES then
			table.insert(lines, "…")
			break
		end
		table.insert(lines, line)
		bytes += #line + 1
	end
	while #lines > 0 and lines[#lines] == "" do
		table.remove(lines)
	end
	local longest = 2
	for run in string.gmatch(code, "`+") do
		longest = math.max(longest, #run)
	end
	local fence = string.rep("`", longest + 1)
	return `{fence}\n{table.concat(lines, "\n")}\n{fence}`
end

-- `text` with `comments` after it (unchanged with none).
function M.with_comments(text: string, comments: { Comment }): string
	if #comments == 0 then
		return text
	end
	local diff = true
	for _, c in comments do
		if c.side ~= "L" and c.side ~= "R" then
			diff = false
		end
	end
	local lines: { string } = {}
	for _, c in comments do
		local body = string.match(c.body, "^%s*(.-)%s*$") or c.body
		if c.snippet and string.match(c.snippet, "%S") then
			body = `{body}\n{fenced(c.snippet)}`
		end
		body = string.gsub(body, "\n", "\n  ")
		local separator = if c.side == "L" or c.side == "R" then ` ({c.side}): ` else ": "
		table.insert(lines, `- {M.place(c)}{separator}{body}`)
	end
	local words = if text == "" then M.ONLY else text
	return `{words}\n\n{if diff then M.DIFF_HEADER else M.HEADER}\n{table.concat(lines, "\n")}`
end

return M
