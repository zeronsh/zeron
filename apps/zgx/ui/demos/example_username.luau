-- Setup, right after the account is made and before the card: choosing a
-- username. It's what the card is for: its address (zeron.sh/c/mara) is
-- the username, so there is no card without one, and it's chosen once.
-- The username is the whole screen, as the organization's name is on its
-- step: one field, the address it makes on the line under it (checked a
-- beat after the typing stops: being checked, free, taken, reserved, or
-- not a username yet), and the way on, open only once it's free.
--
-- What's typed is taken as it will be kept: lowercased (a username is
-- the same in any case), a leading @ and the spaces round it dropped. What
-- can't be kept is said on the line, not fixed in the field: the field
-- shows what was typed, the line what it would be. The rule is the
-- backend's: letters, digits, _ and -, starting with a letter or digit,
-- 3 to 20 characters.

local ds = require("@ds")
local kit = require("@demos/example_kit")
local Icon = require("@ds/icon")

local typed = ui.state("ex_un_typed", "mara")
local shown = ui.state("ex_un_state", "ready")
local check = ui.state("ex_un_check", "available")
local failed = ui.state("ex_un_failed", false)

-- Usernames already spoken for, and the ones nobody gets.
local TAKEN = { theo = true, priya = true, jonas = true, ada = true, graham = true, sam = true, lin = true }
local RESERVED = { admin = true, zeron = true, api = true, www = true, support = true, help = true, settings = true, login = true, signup = true, about = true, me = true }
local MIN, MAX = 3, 20

-- What was typed, as a username is kept: lowercase, no leading @, no
-- spaces round it. What's left is a username only if it's letters, digits
-- and _; the line says what's wrong otherwise.
local function normalise(raw: string): string
	local s = string.lower(raw)
	s = string.gsub(s, "^%s+", "")
	s = string.gsub(s, "%s+$", "")
	s = string.gsub(s, "^@", "")
	return s
end

-- What stands in the way of a username, before anyone is asked: "empty",
-- "invalid", "start", "short", "long", "reserved"; or nothing.
local function problem(u: string): string?
	if u == "" then
		return "empty"
	elseif string.find(u, "[^%w_%-]") then
		return "invalid"
	elseif not string.find(u, "^%w") then
		return "start"
	elseif #u < MIN then
		return "short"
	elseif #u > MAX then
		return "long"
	elseif RESERVED[u] then
		return "reserved"
	end
	return nil
end

-- Asked a beat after the typing stops; the last question asked is the one
-- answered. (The state switch's "checking" holds the question open.)
local attempt = 0
local function look_up(u: string)
	attempt += 1
	local mine = attempt
	local why = problem(u)
	if why then
		check:set(why)
		return
	end
	check:set("checking")
	if shown:peek() == "checking" then
		return
	end
	ui.after(600, function()
		if mine == attempt then
			check:set(if TAKEN[u] then "taken" else "available")
		end
	end)
end

-- What the address line says at its end: a glyph and a word in the ink of
-- the news, one at a time, in a slot that keeps one line's height whatever
-- it says.
local function status(kind: string, glyph: Node?, label: string, ink: string): Node
	return ui.show(function()
		return check:get() == kind
	end, ui.row {
		gap = "$space.1",
		items = "center",
		shrink = 0,
		color = ink,
		glyph,
		ui.text { label, text_size = "$font.size.sm", line_height = "$font.line.sm" },
	})
end

local function mark(name: Icon.Name): Node
	return ui.div { y = -0.5, ds.Icon { name = name, size = 12 } }
end

return {
	id = "ex_username",
	title = "Claim your username",
	about = "Right after the account is made, before the card: one field for the username, the address it makes on the line under it (checked a beat after the typing stops: free, taken, reserved, or not a username yet), and the way on, open once it's free. Chosen once, said calmly.",
	demo = function()
		ui.effect(function()
			local s = shown:get()
			failed:set(s == "failed")
			-- What each state shows typed.
			local samples: { [string]: string } = {
				ready = "mara",
				failed = "mara",
				empty = "",
				checking = "mara_okafor",
				taken = "theo",
				invalid = "mara okafor",
				short = "ma",
				reserved = "admin",
			}
			local text = samples[s]
			if text then
				typed:set(text)
			end
		end)
		ui.effect(function()
			look_up(normalise(typed:get()))
		end)
		local function handle(): string
			return normalise(typed:get())
		end
		local function free(): boolean
			return check:get() == "available"
		end
		local address = ui.row {
			w_full = true,
			items = "center",
			justify = "between",
			gap = "$space.3",
			ui.row {
				items = "center",
				min_w = 0,
				ui.text { "zeron.sh/c/", text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary" },
				ds.FadeText {
					handle,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = function()
						local c = check:get()
						return if c == "taken" or c == "reserved" or c == "invalid" or c == "start" then "$zeron.tone.danger.ink" else "$text.secondary"
					end,
				},
			},
			ui.row {
				items = "center",
				status("checking", ds.ProgressRing { size = 12, tone = "neutral", label = "Checking the username" }, "Checking", "$text.tertiary"),
				status("available", mark("check-circle"), "Available", "$zeron.tone.success.ink"),
				status("taken", mark("close-circle"), "Taken", "$zeron.tone.danger.ink"),
				status("reserved", mark("close-circle"), "Reserved", "$zeron.tone.danger.ink"),
				status("invalid", mark("info-circle"), "Letters, digits, _ and - only", "$zeron.tone.danger.ink"),
				status("start", mark("info-circle"), "Start with a letter or digit", "$zeron.tone.danger.ink"),
				status("short", nil, `At least {MIN} characters`, "$text.tertiary"),
				status("long", nil, `At most {MAX} characters`, "$text.tertiary"),
			},
		}
		return kit.stage {
			states = {
				{ value = "ready", label = "Available" },
				{ value = "empty", label = "Empty" },
				{ value = "checking", label = "Checking" },
				{ value = "taken", label = "Taken" },
				{ value = "invalid", label = "Invalid" },
				{ value = "short", label = "Short" },
				{ value = "reserved", label = "Reserved" },
				{ value = "failed", label = "Failed" },
			},
			state = shown,
			w = 680,
			h = 500,
			-- Between the account and the card: nothing to go back to, and
			-- no skipping (a username is the card; skipping was at sign-in).
			kit.step {
				continue = {
					label = function()
						return if failed:get() then "Try again" else "Continue"
					end,
					disabled = function()
						return not free() and not failed:get()
					end,
					on_click = function()
						if failed:peek() then
							shown:set("ready")
						end
					end,
				},
				kit.form {
					title = "Claim your username",
					-- Gone wrong, the line under the title says so, in the
					-- danger's ink, and the button offers another go: nothing
					-- else moves.
					body = function()
						return if failed:get()
							then `Couldn't save @{handle()}. Nothing was changed.`
							else "It's your card's address, and it's yours for good."
					end,
					tone = function()
						return if failed:get() then "$zeron.tone.danger.ink" else "$text.secondary"
					end,
					ui.column {
						w_full = true,
						gap = "$space.2",
						ds.TextField {
							value = typed,
							name = "Username",
							placeholder = "username",
							size = "lg",
							w_full = true,
							autofocus = true,
						},
						address,
					},
				},
			},
		}
	end,
}
