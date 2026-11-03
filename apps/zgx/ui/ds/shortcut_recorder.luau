--!strict
-- A key combination's recorder, as a settings row's control: a button
-- showing the keys ("⌃⌥Space"); pressed, it listens for a new combination
-- (a letter, a digit, Space, Tab, Enter, an arrow, Home/End/Page or F1–F20,
-- with Control, Option or Command held, as the shipping app takes them).
-- Escape, or leaving it, puts it back as it was; Delete takes it off where
-- the setting may have none (`clearable`). Combinations are written as
-- Zeron writes them: `ctrl+alt+space`, "" for none.
--
--   ShortcutRecorder { value = function() return combo end, label = "Global shortcut",
--     on_change = function(combo) … end }
--
-- `on_listening` hears it start and stop listening (a global shortcut let
-- go meanwhile, so pressing it records rather than fires). While it
-- listens it takes every key, the menus' and commands' included
-- (`capture_keys`), so ⌘J records as ⌘J.

local Button = require("@self/button")
local control = require("@self/control")
local locale = require("@self/locale")

export type Props = {
	-- The combination now ("" for none), followed.
	value: () -> string,
	on_change: (combo: string) -> (),
	-- What the combination is for, in its accessible name (a message too).
	label: (string | Message)?,
	clearable: boolean?,
	on_listening: ((listening: boolean) -> ())?,
}

local NAMED: { [string]: string } = {
	[" "] = "space",
	Tab = "tab",
	Enter = "enter",
	ArrowUp = "up",
	ArrowDown = "down",
	ArrowLeft = "left",
	ArrowRight = "right",
	Home = "home",
	End = "end",
	PageUp = "pageup",
	PageDown = "pagedown",
}

local M = {}

-- A combination from a key pressed, or nil.
function M.combo_of(e: KeyEvent): string?
	if not (e.ctrl or e.alt or e.meta) then
		return nil
	end
	local key = NAMED[e.key]
	if not key and string.match(e.key, "^F%d+$") then
		local n = tonumber(string.sub(e.key, 2)) or 0
		key = if n >= 1 and n <= 20 then string.lower(e.key) else nil
	end
	if not key and e.code then
		-- (The physical key, so ⌥ doesn't turn a letter into a symbol.)
		local letter = string.match(e.code, "^Key(%a)$") or string.match(e.code, "^Digit(%d)$")
		key = if letter then string.lower(letter) else nil
	end
	if not key and string.match(e.key, "^%w$") then
		key = string.lower(e.key)
	end
	if not key then
		return nil
	end
	local parts = {}
	if e.ctrl then
		table.insert(parts, "ctrl")
	end
	if e.alt then
		table.insert(parts, "alt")
	end
	if e.shift then
		table.insert(parts, "shift")
	end
	if e.meta then
		table.insert(parts, "cmd")
	end
	table.insert(parts, key)
	return table.concat(parts, "+")
end

-- A combination as its keys show it, or "None" (in the locale shown: read
-- in a function, it follows).
function M.shown(combo: string): string
	return if combo == "" then locale.s("ds.shortcut_recorder.none") else ui.shortcut_text(combo)
end

M.Recorder = ui.component(function(props: Props): Node
	local listening = ui.use_state(false)
	local button = ui.ref()
	-- Stopped: focus back on the button when it was the keys that stopped it
	-- (a combination, Escape), not when focus went elsewhere.
	local function stop(refocus: boolean)
		listening:set(false)
		if props.on_listening then
			props.on_listening(false)
		end
		if refocus then
			local el = button:peek()
			if el then
				el:focus()
			end
		end
	end
	local function heard(e: KeyEvent): boolean
		if e.key == "Escape" then
			stop(true)
			return true
		end
		if props.clearable and (e.key == "Backspace" or e.key == "Delete") and not (e.ctrl or e.alt or e.meta) then
			props.on_change("")
			stop(true)
			return true
		end
		local combo = M.combo_of(e)
		if combo then
			props.on_change(combo)
			stop(true)
		end
		return true
	end
	return ui.row {
		relative = true,
		Button {
			ref = button,
			tone = "secondary",
			size = "sm",
			label = function(): string
				if listening:get() then
					return locale.s("ds.shortcut_recorder.press")
				end
				local name = control.text(props.label) or locale.s("ds.shortcut_recorder.name")
				return locale.s("ds.shortcut_recorder.change", { name = name, keys = M.shown(props.value()) })
			end,
			on_click = function()
				listening:set(true)
				if props.on_listening then
					props.on_listening(true)
				end
			end,
			ui.text {
				function(): string
					return if listening:get() then locale.s("ds.shortcut_recorder.press_waiting") else M.shown(props.value())
				end,
			},
		},
		-- While it listens, what has the keys: every key, a menu's or a
		-- command's (⌘J) included, comes here rather than running
		-- (`capture_keys`), and only while it listens, so a recorder merely
		-- focused leaves the window's shortcuts working. Over the button,
		-- unseen; leaving it (a click elsewhere) stops listening.
		ui.show(listening, ui.div {
			capture_keys = true,
			focusable = true,
			focus = { outline = 0, border = 0 },
			label = function(): string
				return locale.s("ds.shortcut_recorder.press")
			end,
			absolute = true,
			left = 0,
			top = 0,
			right = 0,
			bottom = 0,
			pointer_events = "none",
			on_mount = function(el)
				el:focus()
			end,
			on_key = heard,
			on_blur = function()
				if listening:peek() then
					stop(false)
				end
			end,
		}),
	} :: any
end)

return M
