-- A web link a built-in plugin can show itself: a pull request's, in the GitHub
-- plugin's tab beside the chat, while that plugin is on and its "Open pull request
-- links in Zeron" setting is. Anything else goes where links go.
--
--   plugin_links.take(session, url)   true when a plugin's tab took it

local boundary = require("@app/screens/shell/boundary")

local M = {}

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
	if not session or not on() then
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
