-- Setup, this device's second step: syncing. Signed out, it says what an
-- account gives, in the real step's three lines (onboarding.rs
-- render_sync), and that nothing leaves this Mac until you log in; the way
-- on is Log in, which opens the browser, and the step can be skipped.
-- While the browser has it, the line under the list says so and offers a
-- way out; if it didn't finish, the same line says that and Log in becomes
-- Try again. Signed in (now, or at the start), the list becomes your
-- devices, each with whether it's there now (Settings → Devices' facts:
-- its name, its system, online or when it was last seen), so syncing is
-- shown by what it reaches.
--
-- The list keeps one size whatever it holds, and the line under it one
-- line, so nothing moves from one state to the next. Signed in, the steps
-- count four (GitHub joins them); signed out, three.

local ds = require("@ds")
local kit = require("@demos/example_kit")
local control = require("@ds/control")
local setup = require("@demos/setup_parts")

local shown = ui.state("ex_sync_state", "out")

local function signed_in(): boolean
	local s = shown:get()
	return s == "in" or s == "first"
end

-- What an account gives, the real step's words.
local GIVES = {
	{ "On every device", "Pick up a chat on your phone or another computer, right where you left it." },
	{ "Run agents anywhere", "Start work on any of your machines from whichever one you're at." },
	{ "Zeron Cloud", "Hand chats to cloud machines that keep going while your computer sleeps." },
}

-- Your devices (fictional): this Mac, a Linux box, a phone.
local DEVICES: { { name: string, system: string, here: boolean?, online: boolean?, seen: string? } } = {
	{ name = setup.DEVICE, system = "macOS", here = true, online = true },
	{ name = "build-box-02", system = "Linux", online = true },
	{ name = "Mara's iPhone", system = "iOS", seen = "2 hours ago" },
}

-- The list's one height: three rows of a name and a line (the settings
-- row's 12 above and below, its two lines and the hair between) and the
-- hairlines and edge round them.
local ROW = 12 + 18 + 2 + 16 + 12
local LIST = 3 * ROW + 2 + 2

local function gives(): Node
	local rows: { Child } = {}
	for _, g in GIVES do
		table.insert(rows, ds.Settings.Row { label = g[1], description = g[2] })
	end
	return ds.Settings.Group(rows)
end

local function devices(only_this: boolean): Node
	local rows: { Child } = {}
	for _, d in DEVICES do
		if only_this and not d.here then
			continue
		end
		table.insert(rows, ds.Settings.Row {
			label = d.name,
			badge = if d.here then ds.Badge { "This device", size = "sm" } else nil,
			description = if d.online then d.system else `{d.system} · Last seen {d.seen}`,
			control = if d.online then ds.Badge { "Online", tone = "success" } else ds.Badge { "Offline" },
		})
	end
	return ds.Settings.Group(rows)
end

-- The line under the list: one line, its words by the state, a glyph or
-- a ring before them where there's news.
local function line(): Node
	local function text(): string
		local s = shown:get()
		if s == "waiting" then
			return "Waiting for you to finish in the browser."
		elseif s == "failed" then
			return "Couldn't log in. The browser didn't finish, and nothing was changed."
		elseif s == "first" then
			return "Sign in on another device to see it here."
		elseif s == "in" then
			return `Signed in as {setup.EMAIL}`
		end
		return "Not ready? Everything stays on this Mac until you log in."
	end
	return ui.row {
		w_full = true,
		-- (As tall as the Cancel it can hold, in every state.)
		h = control.H.sm,
		shrink = 0,
		px = ui.raw(17, "a row's 16 inset, past the card's 1 edge: the line starts on the rows' text"),
		gap = "$space.2",
		items = "center",
		ui.show(function()
			return shown:get() == "waiting"
		end, ds.ProgressRing { size = 12, tone = "neutral", label = "Logging in" }),
		ui.text {
			text,
			grow = 1,
			min_w = 0,
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			color = function()
				return if shown:get() == "failed" then "$zeron.tone.danger.ink" else "$text.tertiary"
			end,
		},
		ui.show(function()
			return shown:get() == "waiting"
		end, ds.Button {
			"Cancel",
			tone = "ghost",
			size = "sm",
			on_click = function()
				shown:set("out")
			end,
		}),
	}
end

return {
	id = "ex_sync",
	title = "Sync your work",
	about = "This device's second setup step. Signed out: what an account gives, that nothing leaves this Mac until you log in, Log in or Skip; the browser sign-in waiting, and one that didn't finish, on the line under the list. Signed in: your devices, each online or when it was last seen, so syncing is shown by what it reaches.",
	demo = function()
		local function step(key: string): Node
			local inside = key == "in" or key == "first"
			return kit.step {
				step = 2,
				count = if inside then 4 else 3,
				back = kit.back {},
				aside = if inside then nil else kit.quiet { label = "Skip for now" },
				continue = {
					label = function()
						local s = shown:get()
						return if s == "failed" then "Try again" elseif signed_in() then "Continue" else "Log in"
					end,
					disabled = function()
						return shown:get() == "waiting"
					end,
					on_click = function()
						local s = shown:peek()
						if s == "out" or s == "failed" then
							shown:set("waiting")
						end
					end,
				},
				kit.form {
					wide = true,
					title = "Sync your work",
					body = function()
						return if signed_in()
							then "Your chats, projects and settings stay in step on all of these."
							else "Log in to keep chats, projects and settings in step everywhere."
					end,
					ui.column {
						w_full = true,
						h = LIST,
						shrink = 0,
						ui.presence(function(): string
							local s = shown:get()
							return if s == "in" or s == "first" then s else "out"
						end, function(k: string): Node
							return if k == "out" then gives() else devices(k == "first")
						end, { w_full = true, enter = { opacity = { 0, 1 }, duration = 140 }, exit = { opacity = 0, duration = 80 }, mode = "wait" }),
					},
					line(),
				},
			}
		end
		return kit.stage {
			states = {
				{ value = "out", label = "Signed out" },
				{ value = "waiting", label = "Logging in" },
				{ value = "failed", label = "Failed" },
				{ value = "in", label = "Signed in" },
				{ value = "first", label = "First device" },
			},
			state = shown,
			w = 680,
			h = 500,
			-- The dots count four once signed in: built again when that changes.
			ui.presence(function(): string
				return if signed_in() then "in" else "out"
			end, step, { w_full = true, h_full = true }),
		}
	end,
}
