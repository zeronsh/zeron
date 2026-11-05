-- Where a files pane or a terminal works, and a path's name: plain
-- functions of what they're given (screens/files/api is the host's calls
-- alone, which fixtures stand in for wholesale).

local control = require("@ds/control")
local api = require("@app/screens/files/api")

type Target = api.Target

local M = {}

-- Where a pane or a terminal works: the session's chat on its device (in
-- its folder), or a project's folder with none open. Each may be a value,
-- a function or a state of one, or an id alone.
function M.target_of(project: unknown, session: unknown): Target?
	local s: any, p: any = control.value(session), control.value(project)
	if type(s) == "string" then
		s = { id = s }
	end
	if type(p) == "string" then
		p = { id = p }
	end
	if type(s) == "table" and s.id then
		return {
			chat = s.id,
			device = s.device or (if type(p) == "table" then p.device else nil),
			root = s.cwd or (if type(p) == "table" then p.path else nil),
		}
	elseif type(p) == "table" and p.id then
		return { space = p.id, device = p.device, root = p.path }
	end
	return nil
end

-- A path's last part (a file's name).
function M.base_name(path: string): string
	return string.match(path, "[^/]+$") or path
end

return M
