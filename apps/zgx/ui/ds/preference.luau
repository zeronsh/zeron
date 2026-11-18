-- A preference the person sets in Zeron that the design system follows
-- (Settings → Appearance's frosted glass, accent, text size and fonts, and
-- reduce motion; General's hide email addresses), one value for the whole
-- window.
--
-- The app's own VM keeps it, as it always has (`ui.persist`: a state its
-- settings set, kept across launches), and shares it as it changes
-- (`zeron.preference.<key>`). The design system is lent to each plugin's
-- VM too, a copy of its own (src/plugins `lend_design_system`), and there
-- the preference is that shared state: the app's value, read only (zgx
-- refuses a plugin's `:set` of what the app shares read only). So what a
-- plugin draws with the design system follows the person's choice at once,
-- and no plugin changes it. (Each key is shared with plugins by the host:
-- src/plugins/grants.rs `PREFERENCES`.)
--
--   M.frosted = preference.kept("frosted", true)

local M = {}

-- Whether this is the app's own VM: a plugin's can't read what the app
-- doesn't share with plugins (zgx refuses it), and it never shares this.
M.in_app = pcall(ui.shared, "ds.host", true)

-- The preference `key`, as a state: the app's own, set by its settings,
-- and shared; in a plugin's VM, the app's value, read only (its default
-- where a host shares none).
function M.kept(key: string, default: any): any
	local name = `zeron.preference.{key}`
	if M.in_app then
		local state = ui.persist(key, default)
		local shared = ui.shared(name, state:peek())
		ui.effect(function()
			local now = state:get()
			if shared:peek() ~= now then
				shared:set(now)
			end
		end)
		return state
	end
	local ok, shared = pcall(ui.shared, name, default)
	if ok then
		return shared
	end
	return ui.state(`ds_preference_{key}`, default)
end

return M
