-- Zeron's words in the language chosen: zgx's catalog (`ui.i18n`,
-- `ui.t`), ui/locales/<locale>.json with English (en.json) the source, so
-- every key is there. Screens use `ui.t` directly; the design system goes
-- through this module, because it's lent to plugins:
--
--   local locale = require("@self/locale")       -- (in ds; @ds/locale outside)
--   ui.text { locale.t("ds.file_tree.empty") }    -- a message: follows the locale
--   tooltip = locale.s("ds.badge.remove", { name = n })   -- a string, now
--
-- (Never `:get()` on what `t` gives: in a VM without the catalog it's a
-- plain string.)
--
-- The catalog is one per host, and zgx runs plugins' scripts before the
-- app's: Zeron's windows load ui/locales from the host itself (src/plugins
-- `lend_design_system`, zgx's `ScriptHost::i18n`), so a plugin's VM finds
-- it from its first load. A host without it (a bare test host), until the
-- app has loaded it, gives the English (locale_en.luau, made from en.json by
-- tools/i18n.py), formatted here. Once the app's loaded, every
-- VM's messages follow the locale. The language chosen (Settings →
-- General) is the app's (screens/language), never a plugin's.
--
-- Dates and times read the same way in every screen through these.

local M = {}

-- (Loaded already, by this host's app or an earlier load, a message is
-- made without complaint; otherwise this may be the app's VM, first: its
-- ui/locales is there to load. In a plugin's VM that fails, quietly.)
-- (A test's own VM has no `ui` at all: English, always.)
local has_ui = type(ui) == "table" or type(ui) == "userdata"
local ready = has_ui and pcall(function()
	return ui.t("language.system")
end)
if has_ui and not ready then
	ready = pcall(function()
		ui.i18n { dir = "locales", source = "en" }
	end)
end

-- Whether this VM can make messages: once it can, always.
local function catalog(): boolean
	if not ready and has_ui then
		ready = pcall(function()
			return ui.t("language.system")
		end)
	end
	return ready
end

-- English, formatted without a catalog: `{name}`, `{n, plural, one {#}
-- other {#}}` (and `=N`), `{x, select, … other {…}}`, `{n, number}`;
-- quoting as ICU has it.
local english: { [string]: string }? = nil
local function value(v: any): any
	if type(v) == "function" then
		return v()
	elseif type(v) == "table" or type(v) == "userdata" then
		local ok, got = pcall(function()
			return v:get()
		end)
		return if ok then got else tostring(v)
	end
	return v
end
local function format(msg: string, args: { [string]: any }, count: number?): string
	local out = {}
	local i, n = 1, #msg
	local function word(): string
		local w = string.match(msg, "^%s*([%w_.=-]+)%s*", i)
		if w then
			local _, e = string.find(msg, "^%s*[%w_.=-]+%s*", i)
			i = (e :: number) + 1
		end
		return w or ""
	end
	-- The branch's text up to its closing brace, raw.
	local function branch(): string
		local depth, from = 1, i
		while i <= n do
			local c = string.sub(msg, i, i)
			if c == "{" then
				depth += 1
			elseif c == "}" then
				depth -= 1
				if depth == 0 then
					local body = string.sub(msg, from, i - 1)
					i += 1
					return body
				end
			end
			i += 1
		end
		return string.sub(msg, from)
	end
	while i <= n do
		local c = string.sub(msg, i, i)
		if c == "'" and string.sub(msg, i + 1, i + 1) == "'" then
			table.insert(out, "'")
			i += 2
		elseif c == "'" and string.find(string.sub(msg, i + 1, i + 1), "[{}#|]") then
			local close = string.find(msg, "'", i + 1, true) or (n + 1)
			table.insert(out, string.sub(msg, i + 1, close - 1))
			i = close + 1
		elseif c == "#" and count ~= nil then
			table.insert(out, tostring(count))
			i += 1
		elseif c == "{" then
			i += 1
			local name = word()
			local v = value(args[name])
			if string.sub(msg, i, i) == "}" then
				i += 1
				table.insert(out, if v == nil then `\{{name}\}` else tostring(v))
			else
				i += 1 -- ,
				local kind = word()
				if kind == "plural" or kind == "select" or kind == "selectordinal" then
					i += 1 -- ,
					local chosen: string? = nil
					local other: string? = nil
					while true do
						local key = word()
						if key == "" then
							break
						end
						i += 1 -- {
						local body = branch()
						if key == "other" then
							other = body
						end
						if chosen == nil then
							if kind == "select" then
								if key == tostring(v) then
									chosen = body
								end
							elseif key == `={tostring(v)}` or (key == "one" and tonumber(v) == 1) then
								chosen = body
							end
						end
					end
					local _, e = string.find(msg, "^%s*}", i)
					i = (e or i) + 1
					local num = if kind ~= "select" then tonumber(v) else nil
					table.insert(out, format(chosen or other or "", args, num))
				else
					-- number, date: the value as it is.
					local close = string.find(msg, "}", i, true) or n
					i = close + 1
					table.insert(out, tostring(v))
				end
			end
		else
			table.insert(out, c)
			i += 1
		end
	end
	return table.concat(out)
end
local function english_of(key: string, args: { [string]: any }?): string
	if not english then
		local ok, words = pcall(require, "@self/locale_en")
		english = if ok then words else {}
	end
	local msg = (english :: { [string]: string })[key]
	if msg == nil then
		return key
	end
	return format(msg, args or {}, nil)
end

-- An ICU message formatted with `args` by English's plural rules (a
-- plugin's own words, plugins-sdk/zeron.luau `zeron.s`): `{name}`, plural
-- (`one`, `other`, `=N`), select, number.
function M.format(msg: string, args: { [string]: any }?): string
	return format(msg, args or {}, nil)
end

-- A message to show: it follows the locale. (English text, in a VM without
-- the catalog yet.)
function M.t(key: string, args: { [string]: any }?): Message | string
	if catalog() then
		return ui.t(key, args)
	end
	return english_of(key, args)
end

-- A message's text now (tracked: a function or computed reading it
-- follows the locale). For what wants a string: a tooltip, a sort.
function M.s(key: string, args: { [string]: any }?): string
	if catalog() then
		return ui.t(key, args):get()
	end
	return english_of(key, args)
end

-- How long it has worked: seconds, then minutes, then hours and minutes
-- ("42s", "5m", "1h 5m").
function M.elapsed(seconds: number): string
	local t = math.max(0, math.floor(seconds))
	if t < 60 then
		return M.s("time.short.seconds", { n = t })
	end
	local m = t // 60
	if m < 60 then
		return M.s("time.short.minutes", { n = m })
	end
	return M.s("time.short.hours_minutes", { h = m // 60, m = m % 60 })
end

-- How long ago, in a corner's room: now, then minutes, hours, days and
-- weeks ("5m", "3d"; never "Yesterday": a row's corner holds a few
-- characters).
function M.ago_short(seconds: number): string
	local t = math.max(0, math.floor(seconds))
	if t < 60 then
		return M.s("time.now")
	elseif t < 3600 then
		return M.s("time.short.minutes", { n = t // 60 })
	elseif t < 86400 then
		return M.s("time.short.hours", { n = t // 3600 })
	elseif t < 7 * 86400 then
		return M.s("time.short.days", { n = t // 86400 })
	end
	return M.s("time.short.weeks", { n = t // (7 * 86400) })
end

-- How long ago, in words: "just now", then minutes, hours, days ("yesterday"
-- for one), weeks, months, years ("3 hours ago").
function M.ago(seconds: number): string
	local t = math.max(0, math.floor(seconds))
	if t < 45 then
		return M.s("time.just_now")
	elseif t < 3600 then
		return M.s("time.ago.minutes", { n = math.max(1, t // 60) })
	elseif t < 86400 then
		return M.s("time.ago.hours", { n = t // 3600 })
	elseif t < 7 * 86400 then
		local d = t // 86400
		return if d == 1 then M.s("time.yesterday") else M.s("time.ago.days", { n = d })
	elseif t < 30 * 86400 then
		return M.s("time.ago.weeks", { n = t // (7 * 86400) })
	elseif t < 365 * 86400 then
		return M.s("time.ago.months", { n = t // (30 * 86400) })
	end
	return M.s("time.ago.years", { n = t // (365 * 86400) })
end

-- (A date's parts as a message's arguments: strings, so a year isn't
-- grouped as a number, "2,026".)
local function parts(at: number): { [string]: string }
	local d = os.date("*t", math.floor(at)) :: any
	return { m = tostring(d.month), day = tostring(d.day), year = tostring(d.year), wd = tostring(d.wday) }
end

-- A day: "Oct 29, 2026".
function M.date(at: number): string
	return M.s("time.date", parts(at))
end

-- A day this year: "Oct 29".
function M.month_day(at: number): string
	return M.s("time.month_day", parts(at))
end

-- A weekday's short name: "Thu".
function M.weekday(at: number): string
	return M.s("time.weekday_short", { d = parts(at).wd })
end

-- A clock time on a 12-hour clock: "3:45 PM".
function M.clock(at: number): string
	local d = os.date("*t", math.floor(at)) :: any
	local h = d.hour % 12
	return M.s("time.clock_12", {
		h = tostring(if h == 0 then 12 else h),
		mm = string.format("%02d", d.min),
		half = if d.hour < 12 then "am" else "pm",
	})
end

-- Exactly when: "Oct 29, 2026 at 14:05".
function M.exact(at: number): string
	local d = os.date("*t", math.floor(at)) :: any
	return M.s("time.date_time", { date = M.date(at), time = string.format("%02d:%02d", d.hour, d.min) })
end

return M
