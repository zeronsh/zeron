--!strict
-- Orchestrators' app-side module (`@app/builtin/orchestrators` while the
-- plugin is on): its parts by name, each required as it's first read, so a
-- hook reaching one part loads that part alone. It runs as the app's code
-- (init.luau).

local PARTS: { [string]: () -> any } = {
	proof = function(): any
		return require("./app/proof")
	end,
}

return setmetatable({}, {
	__index = function(self: any, name: string): any
		local load = PARTS[name]
		if not load then
			return nil
		end
		local part = load()
		rawset(self, name, part)
		return part
	end,
})
