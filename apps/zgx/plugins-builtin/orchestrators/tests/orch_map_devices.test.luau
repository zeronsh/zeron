-- One Map machine per real machine (ds/orch_map_model `machines_of`,
-- `fleet`): a machine's engine registered again leaves old device rows of
-- the same name, which join the row in use, their chats with them; a
-- machine offline whose threads are all hidden leaves the Map. On the live
-- account the Map showed two "honeypot"s, one "Nothing running".

local model = require("@orchestrators/ds/orch_map_model")

local NOW = 1800000000
local DAY = 86400

local function dev(id: string, name: string, extra: any?): any
	local d = { id = id, name = name, platform = "linux", online = false, seen = NOW - 20 * DAY }
	for k, v in extra or {} do
		d[k] = v
	end
	return d
end

local function world(devices: { any }, chats: { [string]: any }, show: string?): any
	return { here = "here", devices = devices, chats = chats, show = show or "active", clock = NOW, projects = {}, now = {} }
end

local function names(f: any): string
	local out = {}
	for _, m in f.machines do
		table.insert(out, `{m.name}:{m.id}:{m.count}`)
	end
	return table.concat(out, ",")
end

test("map devices: rows of one name and platform are one machine, under the row in use", function()
	local devices = {
		dev("here", "honeypot-1", { here = true, online = true, seen = nil }),
		dev("old1", "honeypot", { seen = NOW - 17 * DAY }),
		dev("old2", "honeypot", { seen = NOW - 11 * DAY }),
		dev("prod", "honeypot", { online = true, seen = nil }),
	}
	local list, alias = model.machines_of(devices, "here")
	expect(#list).to_be(2)
	expect(alias.old1).to_be("prod")
	expect(alias.old2).to_be("prod")
	expect(alias.prod).to_be("prod")
	expect(alias.here).to_be("here")
	-- A chat left on an old row shows on the machine.
	local f = model.fleet(world(devices, { c1 = { id = "c1", device = "old1", status = "working", updated = NOW } }))
	expect(names(f)).to_be("honeypot-1:here:0,honeypot:prod:1")
end)

test("map devices: offline rows alone, the one seen last stands for them", function()
	local list, alias = model.machines_of({ dev("a", "X16", { seen = NOW - 30 * DAY }), dev("b", "x16", { seen = NOW - 2 * DAY }) }, "here")
	expect(#list).to_be(1)
	expect(list[1].id).to_be("b")
	expect(alias.a).to_be("b")
	expect(list[1].online).to_be(false)
end)

test("map devices: cloud sessions and other platforms stay apart", function()
	local list = model.machines_of({
		dev("c1", "Cloud session", { platform = "cloud", cloud = true }),
		dev("c2", "Cloud session", { platform = "cloud", cloud = true }),
		dev("w", "box", { platform = "windows" }),
		dev("l", "box"),
	}, "here")
	expect(#list).to_be(4)
end)

test("map devices: an offline machine whose threads are all hidden leaves the Map", function()
	local devices = {
		dev("here", "honeypot-1", { here = true, online = true, seen = nil }),
		dev("x16", "X16", { seen = NOW - 31 * DAY }),
		dev("desk", "DESKTOP", { platform = "windows", seen = NOW - 4 * DAY }),
		dev("m2a", "M2A", { platform = "macos", seen = NOW - DAY }),
	}
	local chats = {
		f1 = { id = "f1", device = "x16", status = "failed", updated = NOW - 30 * DAY },
		d1 = { id = "d1", device = "desk", status = "done", updated = NOW - 40 * DAY },
		w1 = { id = "w1", device = "m2a", status = "working", updated = NOW },
	}
	expect(names(model.fleet(world(devices, chats, "active")))).to_be("honeypot-1:here:0,M2A:m2a:1")
	-- In All, every one with a thread.
	expect(names(model.fleet(world(devices, chats, "all")))).to_be("honeypot-1:here:0,DESKTOP:desk:1,M2A:m2a:1,X16:x16:1")
end)
