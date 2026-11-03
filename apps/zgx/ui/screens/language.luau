-- The language Zeron speaks (Settings → General → Language): the Mac's
-- preferred one ("system", zgx's `ui.locale:set(nil)`), or one chosen,
-- kept across launches. It applies at once: every message (`ui.t`) in the
-- window follows the locale; what was made with `:get()` follows the next
-- time it's built. The app's alone (a plugin's VM never sets it).
--
-- The words are ui/locales (ds/locale loads them).

local M = {}

export type Language = { value: string, label: Message }

-- Each in its own words, but the system's (the current language's).
M.LANGUAGES = {
	{ value = "system", label = ui.t("language.system") },
	{ value = "en", label = ui.t("language.en") },
	{ value = "zh-Hans", label = ui.t("language.zh_hans") },
} :: { Language }

M.chosen = ui.persist("settings_language", "system")

-- The locale the app shows, for plugins' own words (the SDK's `zeron.t`):
-- a plugin's VM can't follow `ui.locale` as it changes.
local shown = ui.shared("zeron.locale", "en")

-- The locale the window asks for: none follows the Mac's.
-- (`current` read as the locale is set still says the old one: the one
-- chosen goes out now, and what "system" resolved to a moment later.)
ui.effect(function()
	local v = M.chosen:get()
	ui.locale:set(if v == "system" then nil else v)
	if v ~= "system" then
		shown:set(v)
		return
	end
	-- (zgx negotiates the system's in an effect of its own: read after it.)
	ui.after(0, function()
		if M.chosen:peek() == "system" then
			shown:set(ui.locale.current)
		end
	end)
end)

-- The choices as a select lists them, in the locale shown (a computed
-- reading their messages: they follow it).
function M.options(): { { value: string, label: string } }
	local out = {}
	for _, l in M.LANGUAGES do
		table.insert(out, { value = l.value, label = l.label:get() })
	end
	return out
end

return M
