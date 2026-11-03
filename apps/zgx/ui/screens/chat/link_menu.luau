-- A chat's links' context menus where they aren't the web's (ds/markdown's
-- `link_menu`; ds/links makes the entries):
--   a chat mentioned   Open, Copy link (its Zeron link, `zeron://open/…`)
--   a file in its folder   Open, Reveal in Finder (the chat on this Mac),
--                          Copy path (its whole path, on the chat's device)
--   a file outside it      Copy path
-- Anything else that isn't a file (a `#section`, a link still arriving) has
-- none: the message's own menu shows.
--
--   transcript { open_link = open, link_menu = LinkMenu(open, function() return store.session:peek() end) }

local ds = require("@ds")
local Links = require("@ds/links")
local FileLinks = require("@app/screens/chat/file_links")
local sessions = require("@account/sessions")
local files = require("@app/screens/files/api")

local function base_name(path: string): string
	return string.match(path, "([^/]+)/?$") or path
end

return function(open: (target: string) -> (), session: () -> any?): (target: string) -> { Links.Entry }?
	return function(target: string): { Links.Entry }?
		local chat = string.match(target, "^zeron%-chat:([%w%-_]+)$")
		if chat then
			return Links.chat {
				open = function()
					open(target)
				end,
				copy = function()
					sessions.link(chat):after(function(link: any)
						if type(link) == "string" then
							Links.copy(link)
						end
					end, function()
						ds.toast.show { title = ui.t("ds.links.link_not_ready"), tone = "danger" }
					end)
				end,
			}
		end
		if Links.web(target) then
			return nil
		end
		local s = session()
		local cwd = if s then s.cwd else nil
		local r = FileLinks.resolve(target, cwd)
		if not r then
			return {}
		end
		local path = r.path
		if r.outside then
			return Links.file { path = path }
		end
		local here = s ~= nil and (s.device == nil or s.device == sessions.now:peek().here)
		return Links.file {
			open = function()
				open(target)
			end,
			reveal = if here and s and cwd
				then function()
					files.reveal({ chat = s.id, device = s.device, root = cwd }, path):after(function() end, function()
						ds.toast.show { title = ui.t("files.workspace.not_revealed", { name = base_name(path) }), tone = "danger" }
					end)
				end
				else nil,
			path = if cwd then `{(string.gsub(cwd, "/+$", ""))}/{path}` else path,
		}
	end
end
