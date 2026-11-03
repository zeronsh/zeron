-- This Mac's preferences, as Settings sets them, kept across launches: the
-- shipping app's device-local ones (crates/ui settings.rs, ui-settings.json),
-- read by whatever they change. The composer and the chat page read them
-- here:
--
--   local prefs = require("@app/screens/settings/prefs")
--   prefs.send_with:get()           "enter" | "mod_enter" (⌘↩ sends, ↩ breaks a line)
--   prefs.escape_stops:get()        Escape stops the running agent (no dialog or menu open)
--   prefs.close_panel_with_last_tab:get()  closing the panel's last tab closes it too
--   prefs.compact_transcript:get()  thinking and tool calls folded in a transcript
--   prefs.open_links:get()          "zeron" | "external": a web link in a chat opens in
--                                   the panel's browser tab, or the default browser
--   prefs.pull_requests:get()       "native" | "browser" | "external": where a PR opens
--   prefs.hide_emails:get()        email addresses drawn blurred (ds.PrivateText,
--                                   which reads the same state)
--   prefs.autosave:get()            an edited file saves itself, `autosave_delay` ms
--                                   after the typing stops (the files pane's viewer)
--   prefs.word_wrap:get()           the editor wraps long lines (⌥Z still toggles one)
--   prefs.show_all_files:get()      the file tree lists dotfiles and ignored files
--   prefs.code_font_size:get()      points of code: the editor, markdown's code, diffs
--                                   (not terminals); the gallery's 13 by default
--   prefs.conversation_width:get()  the transcript's and composer's widest, in points
--                                   (560–1200; the gallery's 660 by default)
--   prefs.sound(kind):get()         whether a session's chime plays ("done",
--                                   "request", "attention"; and `sounds`, all of them)
--
-- Banners are the engine's, so their three switches are its file's
-- (`notifications.get`/`.set`, see notifications.luau), not these.

local PrivateText = require("@ds/private_text")

local M = {}

M.send_with = ui.persist("settings_send_with", "enter")
M.escape_stops = ui.persist("settings_escape_stops", false)
-- Whether closing a panel's last tab closes the panel too (off: it stays
-- open on its home). Off by default.
M.close_panel_with_last_tab = ui.persist("settings_close_panel_with_last_tab", false)
M.compact_transcript = ui.persist("settings_compact_transcript", false)
M.pull_requests = ui.persist("settings_pull_requests", "native")
-- (The shipping app's `open_web_links_in_zeron`, on by default.)
M.open_links = ui.persist("settings_open_links", "zeron")
-- (The design system's own, so every PrivateText follows it.)
M.hide_emails = PrivateText.hidden

-- Files (crates/ui settings.rs `files_*`, the same defaults): autosave off,
-- after 900 ms; no wrapping; dotfiles and ignored files left out.
M.autosave = ui.persist("settings_files_autosave", false)
M.autosave_delay = ui.persist("settings_files_autosave_delay", 900)
M.word_wrap = ui.persist("settings_files_word_wrap", false)
M.show_all_files = ui.persist("settings_files_show_all", false)
-- (Choices are a select's options, read in the locale shown: they follow
-- it.)
function M.AUTOSAVE_DELAYS(): { { value: number, label: string } }
	local function ms(n: number): string
		return ui.t("settings.prefs.milliseconds", { n = n }):get()
	end
	local function s(n: string): string
		return ui.t("settings.prefs.seconds", { n = n }):get()
	end
	return {
		{ value = 300, label = ms(300) },
		{ value = 600, label = ms(600) },
		{ value = 900, label = ms(900) },
		{ value = 1500, label = s("1.5") },
		{ value = 3000, label = s("3") },
	}
end

-- Code's type size (crates/ui typography.rs CODE_FONT_SIZE_DEFAULT is 12.5;
-- the gallery's editor token is 13, and the gallery decides the look).
M.code_font_size = ui.persist("settings_code_font_size", 13)
M.CODE_FONT_SIZES = { 11, 12, 13, 14, 15, 16 }

-- How wide a conversation and its composer go (crates/ui settings.rs
-- TRANSCRIPT_WIDTH_*: 560 to 1200; the default is the gallery's measure,
-- `zeron.prose.measure`).
M.CONVERSATION_WIDTH = { min = 560, max = 1200, step = 20 }
M.conversation_width = ui.persist("settings_conversation_width", 660)

-- The chimes: all of them, then each (each counts only while all are on).
M.sounds = ui.persist("settings_sounds", true)
local each = {
	done = ui.persist("settings_sound_done", true),
	request = ui.persist("settings_sound_request", true),
	attention = ui.persist("settings_sound_attention", true),
}

function M.sound(kind: string): State<boolean>
	return each[kind] or M.sounds
end

-- Whether `kind`'s chime plays now.
function M.chimes(kind: string): boolean
	local one = each[kind]
	return M.sounds:peek() and one ~= nil and one:peek()
end

-- Each agent's completion in the composer (Zeron's Settings → Providers,
-- settings/completion.rs): "Use $ for skills" (`$` opens the skills; off,
-- it's ordinary text) and "Separate / commands" (skills kept out of the `/`
-- menu; off, `/` lists them with the commands). Both on unless turned off,
-- kept by agent id.
M.skill_completion = ui.persist("settings_skill_completion", {} :: { [string]: { dollar: boolean?, separate: boolean? } })

export type Completion = { dollar: boolean, separate: boolean }

-- An agent's completion settings (`get`: followed as they change).
function M.completion(harness: string?): Completion
	local one = if harness then M.skill_completion:get()[harness] else nil
	return {
		dollar = not (one and one.dollar == false),
		separate = not (one and one.separate == false),
	}
end

-- One of an agent's two turned on or off ("dollar" or "separate").
function M.set_completion(harness: string, key: "dollar" | "separate", on: boolean)
	local all = table.clone(M.skill_completion:peek())
	local was = all[harness]
	local one: { dollar: boolean?, separate: boolean? } = { dollar = was and was.dollar, separate = was and was.separate }
	if key == "dollar" then
		one.dollar = if on then nil else false
	else
		one.separate = if on then nil else false
	end
	all[harness] = if one.dollar == nil and one.separate == nil then nil else one
	M.skill_completion:set(all)
end

-- What each choice is called where it's chosen (each a select's options,
-- read in the locale shown).
function M.SEND_WITH(): { { value: string, label: string } }
	return {
		{ value = "enter", label = ui.t("settings.prefs.send_with.enter"):get() },
		{ value = "mod_enter", label = ui.t("settings.prefs.send_with.mod_enter"):get() },
	}
end
function M.OPEN_LINKS(): { { value: string, label: string } }
	return {
		{ value = "zeron", label = "Zeron" },
		{ value = "external", label = ui.t("settings.prefs.default_browser"):get() },
	}
end
function M.PULL_REQUESTS(): { { value: string, label: string } }
	return {
		{ value = "native", label = ui.t("settings.prefs.pull_requests.native"):get() },
		{ value = "browser", label = ui.t("settings.prefs.pull_requests.browser"):get() },
		{ value = "external", label = ui.t("settings.prefs.default_browser"):get() },
	}
end

return M
