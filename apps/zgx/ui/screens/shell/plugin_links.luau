-- A web link a built-in plugin can show itself: a pull request's, in the GitHub
-- plugin's tab beside the chat, while that plugin is on and its "Open pull request
-- links in Zeron" setting is. Anything else goes where links go.
--
--   plugin_links.take(session, url)   true when a plugin's tab took it
--   plugin_links.unclaimed(open)      runs `open` with no tab taking its link
--
-- A link a plugin opens itself (`zeron.open_url`) is never taken: its pull
-- request viewer's Open on GitHub would only bring that viewer forward.

local boundary = require("@app/screens/shell/boundary")

local M = {}

-- (While a link a plugin opened goes where links go.)
local passing = false

function M.unclaimed(open: () -> ())
	passing = true
	local ok, why = pcall(open)
	passing = false
	if not ok then
		error(why, 0)
	end
end

local settings = ui.shared("plugins.settings", {} :: { any })

-- A pull request's address, whatever page of it (its files, a comment), and its
-- number; nil for any other address.
function M.pull_request(url: string): (string?, number?)
	local base, number, rest = string.match(url, "^(https://github%.com/[%w%-%._]+/[%w%-%._]+/pull/(%d+))(.*)$")
	if not base or not (rest == "" or string.find(rest, "^[/?#]")) then
		return nil, nil
	end
	return base, tonumber(number)
end

local function on(): boolean
	for _, entry in settings:peek() do
		if entry.plugin == "github" then
			return entry.values ~= nil and entry.values.open_pr_links == true
		end
	end
	return false
end

function M.take(session: any, url: string): boolean
	if passing or not session or not on() then
		return false
	end
	local address, number = M.pull_request(url)
	if not address then
		return false
	end
	local panel: any = boundary.load("@app/screens/panel")
	if not panel or not panel.open_plugin_tab then
		return false
	end
	panel.open_plugin_tab(session, {
		plugin = "github",
		kind = "github-pr",
		title = `#{number}`,
		icon = "pull-request",
		data = { url = address, number = number, session = session.id },
	})
	return true
end

return M
