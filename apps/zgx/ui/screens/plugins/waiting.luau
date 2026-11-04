--!strict
-- What's waiting for you to allow it, said in the window's corner: a plugin
-- (new, or changed since you allowed it) and someone else's template.
--
-- Each notice follows the host's lists, never an event: it's up while the
-- plugin waits and goes however that ends, wherever you decided. Loading the
-- scripts again, or launching, asks the same and shows the same. Put away
-- with its ×, it stays away, this run and the next, until that plugin changes;
-- the Plugins page (or the Templates page) still has its Review.
--
-- Review only opens the consent window (the host's, on its own, which alone
-- can allow anything: src/plugins/consent.rs). Nothing here allows anything.

local toast = require("@ds/toast")
local plugins = require("@app/screens/plugins/state")
local templates = require("@app/screens/templates/state")

-- What you put away, by the notice's key (a plugin's holds its hash, so a
-- changed one speaks again).
local dismissed = ui.persist("dismissed", {} :: { [string]: boolean })

-- (Wrapped so each calls what the module holds as it's pressed.)
toast.action("plugins.review", function(id: string)
	plugins.review(id)
end)
toast.action("templates.review", function(name: string)
	templates.review(name)
end)

type Kind = {
	-- Which toasts are this kind's (`toast.sync`).
	group: string,
	-- How its notices' keys start.
	prefix: string,
	-- Whether its list can be believed yet.
	ready: () -> boolean,
	-- What's waiting now: `{ key, name, value }`.
	waiting: () -> { any },
	review: string,
	body: Message?,
}

local KINDS: { Kind } = {
	{
		group = "plugins-waiting",
		prefix = "plugin:",
		ready = function(): boolean
			return plugins.now:get().state == "ready"
		end,
		waiting = function(): { any }
			local out = {}
			for _, p in plugins.now:get().list or {} do
				if p.status == "pending" or p.status == "tampered" then
					table.insert(out, { key = `plugin:{p.id}@{p.hash or ""}`, name = p.name, value = p.id })
				end
			end
			return out
		end,
		review = "plugins.review",
	},
	{
		group = "templates-waiting",
		prefix = "template:",
		ready = function(): boolean
			return templates.now:get().state == "ready"
		end,
		waiting = function(): { any }
			local out = {}
			for _, p in templates.pending:get() do
				-- (Not one whose window opened as it came: you've just seen it.)
				if not p.here then
					table.insert(out, { key = `template:{p.pending}`, name = p.name, value = p.pending })
				end
			end
			return out
		end,
		review = "templates.review",
		body = ui.t("plugins.init.template_waiting_body"),
	},
}

-- What each kind's notices are now, from what's waiting and not put away.
local function notices(kind: Kind, left: { any }): { toast.Toast }
	local out: { toast.Toast } = {}
	for _, it in left do
		table.insert(out, {
			key = it.key,
			title = ui.t("plugins.init.waiting", { name = it.name }),
			body = kind.body,
			tone = "info",
			duration = 0,
			action = { label = ui.t("plugins.init.review"), run = kind.review, value = it.value, keep = true },
		})
	end
	return out
end

for _, kind in KINDS do
	ui.effect(function()
		if not kind.ready() then
			return
		end
		local waiting = kind.waiting()
		local away = dismissed:get()
		local left = {}
		for _, it in waiting do
			if not away[it.key] then
				table.insert(left, it)
			end
		end
		toast.sync(kind.group, notices(kind, left))
	end)
end

-- What you put away with its ×: remembered. What isn't waiting any more is
-- forgotten (allowed, then changed again, it speaks again).
ui.effect(function()
	local put = toast.put_away:get()
	local before = dismissed:peek()
	local after: { [string]: boolean } = {}
	for _, kind in KINDS do
		if kind.ready() then
			for _, it in kind.waiting() do
				if put[it.key] or before[it.key] then
					after[it.key] = true
				end
			end
		else
			-- (Its list not known yet: it keeps what it had.)
			for key in before do
				if string.sub(key, 1, #kind.prefix) == kind.prefix then
					after[key] = true
				end
			end
		end
	end
	local changed = false
	for key in after do
		changed = changed or not before[key]
	end
	for key in before do
		changed = changed or not after[key]
	end
	if changed then
		dismissed:set(after)
	end
	if next(put) ~= nil then
		toast.put_away:set({})
	end
end)

return {}
