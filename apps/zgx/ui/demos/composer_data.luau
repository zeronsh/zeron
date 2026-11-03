-- What the Composer page's states are made of: a checkout's files for `@`,
-- an agent's commands and skills for `/` and `$`, the cargo a message might
-- carry, a queue, a plan, what is running, a question and an approval, and
-- the notices. Real names from this checkout; nothing made up where the
-- real thing exists.

local Composer = require("@ds/composer")
local Cargo = require("@ds/composer_cargo")
local Dock = require("@ds/composer_dock")
local Ask = require("@ds/composer_ask")
local Mentions = require("@ds/composer_mentions")

type Cargo = Cargo.Item
type Notice = Composer.Notice

local M = {}

-- A time `ago` seconds before now.
local function ago(seconds: number): number
	return os.time() - seconds
end
M.ago = ago

local FILES: { Mentions.Item } = {
	{ value = "crates/ui/src/composer.rs", label = "composer.rs", detail = "crates/ui/src", file = true },
	{ value = "crates/ui/src/composer_dock.rs", label = "composer_dock.rs", detail = "crates/ui/src", file = true },
	{ value = "crates/ui/src/queue.rs", label = "queue.rs", detail = "crates/ui/src", file = true },
	{ value = "crates/ui/src/todo_panel.rs", label = "todo_panel.rs", detail = "crates/ui/src", file = true },
	{ value = "crates/ui/src/attachments.rs", label = "attachments.rs", detail = "crates/ui/src", file = true },
	{ value = "crates/ui/src/composer_markdown.rs", label = "composer_markdown.rs", detail = "crates/ui/src", file = true },
	{ value = "crates/ui/src/composer/", label = "composer", detail = "crates/ui/src", file = true, dir = true },
	{ value = "crates/engine/tests/message_queue.rs", label = "message_queue.rs", detail = "crates/engine/tests", file = true },
	{ value = "apps/zgx/ui/ds/composer.luau", label = "composer.luau", detail = "apps/zgx/ui/ds", file = true },
	{ value = "apps/zgx/ui/ds/composer_dock.luau", label = "composer_dock.luau", detail = "apps/zgx/ui/ds", file = true },
	{ value = "apps/zgx/ui/gallery.luau", label = "gallery.luau", detail = "apps/zgx/ui", file = true },
	{ value = "apps/zgx/ui/ds/themes/zeron-dark.json", label = "zeron-dark.json", detail = "apps/zgx/ui/ds/themes", file = true },
	{ value = "apps/zgx/README.md", label = "README.md", detail = "apps/zgx", file = true },
	{ value = "Cargo.toml", label = "Cargo.toml", detail = "", file = true },
}

local COMMANDS: { Mentions.Item } = {
	{ value = "model", label = "/model", detail = "Choose the agent, model and reasoning", icon = "command" },
	{ value = "new", label = "/new", detail = "Start a new chat", icon = "command" },
	{ value = "resume", label = "/resume", detail = "Pick up an earlier chat", icon = "command" },
	{ value = "diff", label = "/diff", detail = "Review the changes so far", icon = "command" },
	{ value = "files", label = "/files", detail = "Browse the checkout", icon = "command" },
	{ value = "terminal", label = "/terminal", detail = "Open a terminal", icon = "command" },
	{ value = "rename", label = "/rename", detail = "Rename this chat", icon = "command" },
	{ value = "stop", label = "/stop", detail = "Stop the agent", icon = "command" },
	{ value = "compact", label = "/compact", detail = "Fold the conversation so far into a summary", icon = "command" },
	{ value = "review", label = "/review", detail = "Review the branch for bugs · <what to look for>", icon = "command" },
	{ value = "commit", label = "/commit", detail = "Commit what is staged · <message>", icon = "command" },
}

local SKILLS: { Mentions.Item } = {
	{ value = "code-review", label = "Code Review", detail = "Review the diff for correctness bugs", icon = "magic-stick" },
	{ value = "simplify", label = "Simplify", detail = "Reuse, simplification and efficiency cleanups", icon = "magic-stick" },
	{ value = "release", label = "Release", detail = "Bump the version and tag a release", icon = "magic-stick" },
	{ value = "deploy-staging", label = "Deploy Staging", detail = "Deploy the edge workers to staging", icon = "magic-stick" },
	{ value = "capture", label = "Capture", detail = "Capture the gallery in dark and light", icon = "magic-stick" },
}

M.FILES = FILES
M.COMMANDS = COMMANDS
M.SKILLS = SKILLS
local MENTIONS: Mentions.Sources = { ["@"] = FILES, ["/"] = COMMANDS, ["$"] = SKILLS }
M.MENTIONS = MENTIONS

-- The cargo a message might carry.
local IMAGE_SHELL: Cargo = { id = "c-shell", kind = "image", name = "Screenshot 2026-10-30 at 11.42.07.png", src = "assets/composer/shell.png", meta = "PNG · 868 × 617" }
local IMAGE_TRANSCRIPT: Cargo = { id = "c-transcript", kind = "image", name = "Screenshot 2026-10-30 at 11.43.52.png", src = "assets/composer/transcript.png", meta = "PNG · 868 × 617" }
local IMAGE_ART: Cargo = { id = "c-art", kind = "image", name = "plate-1-night.jpg", src = "assets/art/plate-1-night.jpg", meta = "JPEG · 1.2 MB" }
local IMAGE_FOREST: Cargo = { id = "c-forest", kind = "image", name = "forest-back.jpg", src = "assets/cards/forest/back.webp", meta = "JPEG · 640 KB" }
local FILE_DOCK: Cargo = { id = "c-dock", kind = "file", name = "composer_dock.rs", meta = "Rust · 48 KB" }
local FILE_QUEUE: Cargo = { id = "c-queue", kind = "file", name = "message_queue.rs", meta = "Rust · 12 KB" }
local FILE_THEME: Cargo = { id = "c-theme", kind = "file", name = "zeron-dark.json", meta = "JSON · 9 KB" }
local FILE_LOG: Cargo = { id = "c-log", kind = "file", name = "crash-2026-10-29.log", meta = "Log · 3 KB" }
local PASTED: Cargo = { id = "c-paste", kind = "text", name = "Pasted text", meta = "412 lines · 18 KB" }
local PASTED_SHORT: Cargo = { id = "c-paste-2", kind = "text", name = "Pasted text", meta = "36 lines · 2 KB" }
local ANNOTATION_1: Cargo = {
	id = "c-ann-1",
	kind = "annotation",
	name = "Annotation 1",
	element = "button#send.pill.primary",
	meta = "button#send.pill.primary",
	page = "localhost:5173/chat/zgx-composer",
	box = { 1180, 842, 28, 28 },
	html = '<button id="send" class="pill primary" aria-label="Send message">\n  <svg class="icon" aria-hidden="true">…</svg>\n</button>',
}
local ANNOTATION_2: Cargo = {
	id = "c-ann-2",
	kind = "annotation",
	name = "Annotation 2",
	element = "div.tray.queue",
	meta = "div.tray.queue",
	page = "localhost:5173/chat/zgx-composer",
	box = { 452, 690, 544, 118 },
	html = '<div class="tray queue" role="region" aria-label="Queued messages">\n  <header class="tray-head">Queued <span>2</span></header>\n  <ol class="rows">…</ol>\n</div>',
}

M.IMAGE_SHELL = IMAGE_SHELL
M.IMAGE_TRANSCRIPT = IMAGE_TRANSCRIPT
M.IMAGE_ART = IMAGE_ART
M.IMAGE_FOREST = IMAGE_FOREST
M.FILE_DOCK = FILE_DOCK
M.FILE_QUEUE = FILE_QUEUE
M.FILE_THEME = FILE_THEME
M.FILE_LOG = FILE_LOG
M.PASTED = PASTED
M.PASTED_SHORT = PASTED_SHORT
M.ANNOTATION_1 = ANNOTATION_1
M.ANNOTATION_2 = ANNOTATION_2

-- Cargo too many or too large: twenty-two files, and one over the limit.
function M.many(): { Cargo }
	local out: { Cargo } = { IMAGE_SHELL, IMAGE_TRANSCRIPT, FILE_DOCK, FILE_QUEUE, FILE_THEME, FILE_LOG }
	local names = { "composer.rs", "queue.rs", "todo_panel.rs", "attachments.rs", "shell.rs", "pickers.rs", "dictation.rs", "notice.rs", "loaders.rs", "state.rs", "theme.rs", "motion.rs", "transcript.rs", "comments.rs", "sidebar.rs", "settings.rs" }
	for i, n in names do
		local file: Cargo = { id = `c-many-{i}`, kind = "file", name = n, meta = `Rust · {12 + (i * 7) % 60} KB` }
		table.insert(out, file)
	end
	return out
end

-- A queue of three, the second with a picture.
function M.queue(): { Dock.Queued }
	return {
		{ id = "q1", text = "Then run the full test suite and paste any failures here" },
		{ id = "q2", text = "Open a draft PR against main once the tests pass", cargo = { IMAGE_TRANSCRIPT } },
		{ id = "q3", text = "Also bump the version to v0.2.130 and tag it" },
	}
end

-- A plan of seven, the third in progress.
function M.todos(): { Dock.Todo }
	return {
		{ id = "t1", text = "Read composer_dock.rs for the tray order", state = "done" },
		{ id = "t2", text = "Port the queue's rows and their actions", state = "done" },
		{ id = "t3", text = "Port the plan's folding: one before, the current, one after", state = "doing" },
		{ id = "t4", text = "Fold the running tray when nothing runs", state = "pending" },
		{ id = "t5", text = "Capture every state in dark and light", state = "pending" },
		{ id = "t6", text = "Check frames while typing a long draft", state = "pending" },
		{ id = "t7", text = "Write up what Zeron's composer has that ours doesn't", state = "pending" },
	}
end

function M.todos_done(): { Dock.Todo }
	local out = M.todos()
	for _, t in out do
		t.state = "done"
	end
	return out
end

-- What is running: a test run, a dev server, a subagent.
function M.tasks(): { Dock.Task }
	return {
		{ id = "k1", kind = "terminal", name = "cargo test -p zeron-ui", detail = "composer::queue", since = ago(134) },
		{ id = "k2", kind = "process", name = "vite dev", detail = ":5173", since = ago(1812) },
		{ id = "k3", kind = "agent", name = "Reviewing the diff", detail = "14 files read", since = ago(41) },
		-- A described command: its name, then the command itself, long.
		{
			id = "k4",
			kind = "process",
			name = "Build with the webkitgtk feature",
			detail = "cd ~/dev/zeron/apps/zgx && cargo build --release -j 4 --features webkitgtk 2>&1 | tee build.log",
			since = ago(420),
		},
	}
end

local QUESTION: Ask.Question = {
	header = "Branch",
	text = "Which branch should this rebase onto?",
	options = {
		{ value = "main", label = "main", detail = "The default branch · 12 commits ahead" },
		{ value = "release", label = "release/v0.2.130", detail = "Tagged yesterday · the release train" },
		{ value = "keep", label = "Keep it where it is", detail = "zeron/zgx-onboarding-design-system" },
	},
}

local QUESTION_MULTI: Ask.Question = {
	header = "Scope",
	text = "Which parts of the composer should the review cover?",
	multi = true,
	page = 2,
	pages = 3,
	options = {
		{ value = "cargo", label = "The cargo strip", detail = "Chips, uploads, the fold" },
		{ value = "trays", label = "The trays", detail = "Queue, plan, what is running" },
		{ value = "mentions", label = "Mentions", detail = "@ files, / commands, $ skills" },
		{ value = "voice", label = "Dictation", detail = "The track and its clock" },
	},
}

local APPROVAL: Ask.Approval = {
	title = "Run this command?",
	command = "cargo test --workspace -- --nocapture",
	cwd = "~/.zeron/worktrees/comet-native/swift-delta",
	detail = "Runs every crate's tests; about four minutes.",
}

-- The notices.
local NOTICES: { [string]: Notice } = {
	offline = { tone = "warning", text = "Offline — messages will send when you're back online." },
	reconnecting = { tone = "info", spinner = true, text = "Reconnecting — messages will send once the connection recovers." },
	sending = { tone = "neutral", spinner = true, text = "Sending…" },
	error = { tone = "danger", text = "Send failed: devbox didn't answer." },
	too_large = { tone = "danger", text = "Zeron.app.zip is too large (24 MB max)." },
	too_many = { tone = "warning", text = "Up to 20 files at a time. 2 were left out." },
	restored = { tone = "neutral", text = "Draft restored from Tuesday." },
	archived = { tone = "neutral", text = "This chat is archived." },
}

M.QUESTION = QUESTION
M.QUESTION_MULTI = QUESTION_MULTI
M.APPROVAL = APPROVAL
M.NOTICES = NOTICES

-- Drafts.
M.DRAFT_SHORT = "Port the queue tray's reorder, then fold the plan when every step is done."
M.DRAFT_MENTIONS = "Compare @queue.rs with @composer_dock.luau and $code-review the result, then /commit it once @message_queue.rs passes."
M.DRAFT_MULTI = "Three things, in this order:\n\n1. Port the queue's rows and their **Send next** action\n2. Fold the plan tray once every step is done\n3. Capture both themes with `zgx inspect snapshot`\n\nDon't touch the footer."
M.DRAFT_STEER = "Actually, keep the footer as it is — only change the trays."

function M.long_draft(): string
	local lines = {
		"# Composer review notes",
		"",
		"Everything below is what I saw clicking through the states this afternoon, in the order I hit them. Nothing is sorted by severity yet.",
		"",
	}
	local seen = {
		"The queue tray's rows keep their place as a row leaves; the others slide up over 220 ms, which reads right.",
		"The plan folds to its current step with one neighbour either side; `3 earlier` opens the rest in place.",
		"A chip arriving scales in from 90%; leaving, it fades while the strip closes up behind it.",
		"The mention popover sits at the caret and follows it as the line wraps; ↑↓ move, Enter takes, Esc puts it away until the token changes.",
		"Dictating, the track unrolls from the microphone to the attach button and the clock turns amber in the last ten seconds.",
		"Offline is a line above the trays, not a banner: a dot and a sentence.",
		"The footer still folds a step at a time at narrow widths: Plan goes first, then the mode's name, then the branch.",
		"An approval is the pill, not a dialog: the command in its own face, Deny · Always allow · Allow on the controls' line.",
	}
	for i = 1, 24 do
		table.insert(lines, `- {seen[(i - 1) % #seen + 1]}`)
	end
	table.insert(lines, "")
	table.insert(lines, "```sh")
	table.insert(lines, "zgx inspect --app 35721 frames 10 --since mark")
	table.insert(lines, "```")
	table.insert(lines, "")
	table.insert(lines, "Open questions: whether the running tray should fold itself when the last task ends, and whether *Send now* should keep the row's cargo when the turn is interrupted.")
	return table.concat(lines, "\n")
end

return M
