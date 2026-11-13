-- Where a new chat works, as the shipping app's new-chat location picker has
-- it (crates/ui pickers.rs, composer.rs; crates/proto view.rs
-- `checkout_plan`): the device it runs on, the project there (or none), a
-- local checkout or a worktree, and the ref.
--
-- - Devices: your computers, this one first; one whose presence has gone
--   quiet (no beat in 70 s) is shown, dimmed, with when it was last seen,
--   and can't be picked. Then Cloud, while it's on and can open the
--   project's GitHub repository (account/cloud): the chat runs on a machine
--   of its own that clones the repository, from the ref chosen among
--   GitHub's branches (`ListCloudBranches`). On Cloud, the projects are
--   those Cloud can open, on whichever device.
-- - Projects: the chosen device's, searchable, "No project" among them.
--   Picking another device brings that device's projects (the one last used
--   there, else its first); picking a project on another device moves the
--   device with it. A project no longer there stays named, warned and
--   dimmed, and the chat can't start in it.
-- - Refs: the project's branches from git where it is (`ListRefs`, forwarded
--   to its device), the current first; the chip shows the one chosen, else
--   the project's current branch.
-- - Remembered: the last device, and the last project on each device.
-- - Sent: a local checkout runs in the project's folder (or, for a ref
--   checked out in a worktree of its own, in that worktree), the ref named
--   on the chat; a worktree is made off the ref as the run starts (HEAD if
--   the refs never came). A folder with no git runs in place, whichever
--   checkout was picked. On Cloud, the chat is made there (`cloud`) and runs
--   in its machine's checkout.
--
-- `Where.bind { device, project, checkout, branch, active, slot?, remember? }`
-- (in a component): the composer's choices, and while `active` (a new chat,
-- or a side chat before its first message) it keeps them in step; it gives
-- back what the composer lists and how to send.

local sessions = require("@account/sessions")
local cloud = require("@account/cloud")
local route = require("@app/screens/shell/route")
local store = require("@app/screens/chat/store")

local M = {}

M.NO_PROJECT = "none"
-- The device Cloud is picked as (the engine's name for Cloud's own: its
-- agents and models are Cloud's).
M.CLOUD = "cloud"
-- A device is out of reach once its presence heartbeat (every 15 s) has been
-- quiet this long (the shipping app's `DEVICE_ONLINE_WINDOW_SECS`).
local ONLINE_WINDOW = 70

-- The last device a new chat ran on, and the last project on each device
-- ("none": no project), kept across launches.
M.remembered = ui.persist("chat_where", {
	device = nil :: string?,
	projects = {} :: { [string]: string },
})

export type Choices = {
	device: State<string?>,
	project: State<string?>,
	checkout: State<string?>,
	branch: State<string?>,
	active: () -> boolean,
	-- Another composer's choices (a side chat's, by its id): its refs kept
	-- apart from the chat page's. Nil: the chat page's own.
	slot: string?,
	-- Whether its device and project are remembered for the next new chat
	-- (the chat page's are; a side chat's, made where its parent is, aren't).
	remember: boolean?,
}

-- What a send does: where the chat is made (createChat), and where and how
-- its first run goes.
export type Plan = {
	-- The project (nil: none), else the device it's made on.
	project: string?,
	device: string?,
	-- The ref named on the chat from its first frame.
	branch: string?,
	-- Where it runs; nil, the device's own folder for it.
	cwd: string?,
	-- A worktree made off `branch` (HEAD with none) as it starts.
	worktree: boolean,
	-- On a Cloud machine of its own, cloning the project's repository.
	cloud: boolean?,
}

-- How long ago, in words ("2 min ago").
local function ago(seconds: number): string
	local s = math.max(0, math.floor(seconds))
	-- (To the nearest of its unit.)
	local function near(unit: number): number
		return math.max(1, math.floor(s / unit + 0.5))
	end
	if s < 60 then
		return ui.t("chat.where.ago.just_now"):get()
	elseif s < 3570 then
		return ui.t("chat.where.ago.minutes", { n = near(60) }):get()
	elseif s < 84600 then
		return ui.t("chat.where.ago.hours", { n = near(3600) }):get()
	end
	return ui.t("chat.where.ago.days", { n = near(86400) }):get()
end

-- The refs loaded: for which project (and where), how it went, the list.
type Refs = { key: string, state: "idle" | "loading" | "ready" | "error" | "away", list: { store.Ref } }

function M.bind(c: Choices)
	-- Presence is judged as time passes, not only as the sessions change.
	local clock = ui.state("chat_where_clock", ui.now() / 1000)
	-- (Only while it's a new chat's: a session's composer has no devices to
	-- judge. The tick belongs to the effect's run, so it stops with it.)
	ui.effect(function()
		if not c.active() then
			return
		end
		clock:set(ui.now() / 1000)
		ui.every(15000, function()
			clock:set(ui.now() / 1000)
		end)
	end)
	local function online(d: sessions.Device): boolean
		if d.here then
			return true
		end
		-- (Never seen: not up, as the host says.)
		if d.seen then
			return clock:get() - d.seen <= ONLINE_WINDOW
		end
		return d.online == true
	end

	local function computers(): { sessions.Device }
		return sessions.computers(sessions.now:get().devices)
	end
	local function device_row(id: string?): sessions.Device?
		for _, d in computers() do
			if d.id == id then
				return d
			end
		end
		return nil
	end
	local function project_row(id: string?): sessions.Project?
		if id == nil or id == M.NO_PROJECT then
			return nil
		end
		for _, p in sessions.now:get().projects do
			if p.id == id then
				return p
			end
		end
		return nil
	end
	-- A project that lives on Cloud alone (New project's Cloud: a
	-- repository, on the account's Cloud device, no folder anywhere).
	local function lives_on_cloud(p: sessions.Project?): boolean
		return p ~= nil and string.sub(p.device, 1, 6) == "cloud-"
	end
	-- The repository Cloud would clone for a project, if Cloud can open it.
	-- (One on Cloud runs there even before Cloud's repositories are read, as
	-- the shipping app's does: the engine takes the repository from it.)
	local function cloud_repo(p: sessions.Project?): cloud.Repo?
		if not p then
			return nil
		end
		local found = cloud.reaches(p.github)
		if not found and lives_on_cloud(p) and p.github and cloud.on() then
			return { name = p.github :: string, private = false, pushed = 0 }
		end
		return found
	end
	local function on_cloud(): boolean
		return c.device:get() == M.CLOUD
	end
	local function on_device(device: string?): { sessions.Project }
		local out = {}
		for _, p in sessions.now:get().projects do
			-- (On Cloud: those it can open, on whichever device.)
			if if device == M.CLOUD then cloud_repo(p) ~= nil else p.device == device then
				table.insert(out, p)
			end
		end
		return out
	end
	-- Cloud is offered for the project chosen (or, on Cloud already, while
	-- it can open any).
	local function cloud_offered(): boolean
		if not cloud.on() then
			return false
		end
		local p = project_row(c.project:get())
		if p then
			return cloud_repo(p) ~= nil
		end
		return on_cloud() and #on_device(M.CLOUD) > 0
	end

	-- The devices offered: yours, this one first; one out of reach dimmed,
	-- saying when it was last seen.
	local devices = ui.computed(function(): { any }
		local out = {}
		for _, d in computers() do
			local up = online(d)
			table.insert(out, {
				value = d.id,
				label = d.name,
				you = d.here or nil,
				offline = not up or nil,
				disabled = not up or nil,
				description = if up
					then nil
					elseif d.seen then ui.t("chat.where.device.offline_seen", { ago = ago(clock:get() - d.seen) }):get()
					else ui.t("chat.where.device.offline"):get(),
			})
		end
		if cloud_offered() then
			local repo = cloud_repo(project_row(c.project:get()))
			table.insert(out, {
				value = M.CLOUD,
				label = "Cloud",
				cloud = true,
				description = if repo
					then ui.t("chat.where.device.cloud_from", { repo = repo.name }):get()
					else ui.t("chat.where.device.cloud"):get(),
			})
		end
		return out
	end)

	-- The projects offered: the chosen device's. The one chosen, if it's no
	-- longer there, stays named (warned, dimmed) so the choice reads as it is.
	local projects = ui.computed(function(): { any }
		local out = {}
		for _, p in on_device(c.device:get()) do
			local l = route.look(p.id) or { hue = route.hue(p.id) }
			table.insert(out, { value = p.id, label = p.name, hue = l.hue, src = l.src, emoji = l.emoji, plain = not p.git or nil })
		end
		local chosen = c.project:get()
		if chosen and chosen ~= M.NO_PROJECT then
			local found = false
			for _, p in out do
				if p.value == chosen then
					found = true
				end
			end
			if not found then
				local p = project_row(chosen)
				table.insert(out, {
					value = chosen,
					label = if p then p.name else ui.t("chat.where.project.missing"):get(),
					warn = true,
					disabled = true,
					description = if p then ui.t("chat.where.project.elsewhere"):get() else ui.t("chat.where.project.gone"):get(),
				})
			end
		end
		return out
	end)

	-- Kept in step while it's a new chat's.
	-- A project on another device moves the device with it.
	ui.effect(function()
		if not c.active() then
			return
		end
		local p = project_row(c.project:get())
		-- A project on Cloud alone runs on Cloud.
		if lives_on_cloud(p) then
			if cloud_repo(p) and c.device:peek() ~= M.CLOUD then
				c.device:set(M.CLOUD)
			end
			return
		end
		-- (On Cloud, a project Cloud can open stays there.)
		if c.device:peek() == M.CLOUD and cloud_repo(p) then
			return
		end
		if p and p.device ~= c.device:peek() and device_row(p.device) then
			c.device:set(p.device)
		end
	end)
	-- Cloud no longer offered (turned off, or the repository out of its
	-- reach): back to the project's device.
	ui.effect(function()
		if not c.active() or not on_cloud() or cloud.now:get().phase ~= "ready" then
			return
		end
		if not cloud_offered() then
			local p = project_row(c.project:peek())
			c.device:set(if p and device_row(p.device) then p.device else sessions.now:peek().here)
		end
	end)
	-- No device yet, or one no longer yours: the one last used, else this one.
	ui.effect(function()
		if not c.active() then
			return
		end
		local now = sessions.now:get()
		local d = c.device:get()
		if d and (device_row(d) or d == M.CLOUD) then
			return
		end
		local last = M.remembered:peek().device
		local pick = if last and device_row(last) then last else now.here
		if pick and pick ~= d then
			c.device:set(pick)
		end
	end)
	-- Another device: its projects. The project stays if it's there; else
	-- the one last used there, else its first, else none.
	local device_was: string? = c.device:peek()
	ui.effect(function()
		local d = c.device:get()
		if not c.active() or d == device_was then
			device_was = d
			return
		end
		device_was = d
		local chosen = c.project:peek()
		local here = on_device(d)
		local function there(id: string?): boolean
			if id == M.NO_PROJECT then
				return true
			end
			for _, p in here do
				if p.id == id then
					return true
				end
			end
			return false
		end
		if chosen ~= nil and there(chosen) then
			return
		end
		local last = if d then M.remembered:peek().projects[d] else nil
		local pick = if last and there(last) then last elseif here[1] then here[1].id else M.NO_PROJECT
		c.project:set(pick)
	end)
	-- None chosen yet (a new chat the route gave no project): the one last
	-- used on the device, else its first, else none.
	ui.effect(function()
		local d = c.device:get()
		-- (Only once the sessions have come: before, every device has no
		-- projects, and none would be picked for good.)
		if not c.active() or c.project:get() ~= nil or not d or sessions.now:get().state ~= "ready" then
			return
		end
		local here = on_device(d)
		local last = M.remembered:peek().projects[d]
		local pick = M.NO_PROJECT
		if last == M.NO_PROJECT then
			pick = last
		elseif last then
			for _, p in here do
				if p.id == last then
					pick = last
				end
			end
		end
		if pick == M.NO_PROJECT and last ~= M.NO_PROJECT and here[1] then
			pick = here[1].id
		end
		c.project:set(pick)
	end)
	-- Remembered: the device, and the project on it.
	ui.effect(function()
		if not c.active() or c.remember == false then
			return
		end
		local d, p = c.device:get(), c.project:get()
		-- (Cloud is chosen afresh each time: its projects are its own.)
		if not d or not device_row(d) or sessions.now:get().state ~= "ready" then
			return
		end
		local r = M.remembered:peek()
		local projects = r.projects
		local keep_project = p ~= nil and (p == M.NO_PROJECT or project_row(p) ~= nil)
		if r.device == d and (not keep_project or projects[d] == p) then
			return
		end
		local next = table.clone(projects)
		if keep_project then
			next[d] = p :: string
		end
		M.remembered:set { device = d, projects = next }
	end)

	-- The project's refs, from git where it is: loaded as the project (or
	-- where it is) changes; the chip's ref the one chosen, else the current.
	local refs = ui.state(if c.slot then `chat_where_refs.{c.slot}` else "chat_where_refs", { key = "", state = "idle", list = {} } :: Refs)
	local project_was: string? = c.project:peek()
	-- The refs this build asked for (a load's answer comes to the build
	-- that asked).
	local asked: string? = nil
	ui.effect(function()
		local id = c.project:get()
		if not c.active() then
			return
		end
		-- Another project: its own ref, chosen afresh.
		if id ~= project_was then
			project_was = id
			if c.branch:peek() ~= nil then
				c.branch:set(nil)
			end
		end
		local p = project_row(id)
		-- On Cloud: GitHub's branches of the repository, the default first.
		local repo = if on_cloud() then cloud_repo(p) else nil
		if p and repo then
			local key = `cloud|{repo.name}`
			local was = refs:peek()
			if was.key == key and (was.state == "ready" or was.state == "error" or (was.state == "loading" and asked == key)) then
				return
			end
			asked = key
			refs:set { key = key, state = "loading", list = {} }
			cloud.branches(repo.name, repo.branch):after(function(list: any)
				if refs:peek().key == key then
					refs:set { key = key, state = "ready", list = list or {} }
				end
			end, function()
				if refs:peek().key == key then
					refs:set { key = key, state = "error", list = {} }
				end
			end)
			return
		end
		if not p or not p.git then
			if refs:peek().key ~= "" then
				refs:set { key = "", state = "idle", list = {} }
			end
			return
		end
		local d = device_row(p.device)
		local key = `{p.id}|{p.device}|{p.path}`
		if d and not online(d) then
			if refs:peek().key ~= key or refs:peek().state ~= "away" then
				refs:set { key = key, state = "away", list = {} }
			end
			return
		end
		local was = refs:peek()
		-- Loaded, or being loaded by this composer: as it is. (A load another
		-- build of it began, before a reload, may never answer it: asked
		-- again.)
		if was.key == key and (was.state == "ready" or was.state == "error" or (was.state == "loading" and asked == key)) then
			return
		end
		asked = key
		refs:set { key = key, state = "loading", list = if was.key == key then was.list else {} }
		local here = sessions.now:peek().here
		-- (A host with no way to ask, which raises or gives nothing: as if git
		-- didn't answer.)
		local ok, answer = pcall(store.refs, p.path, if p.device ~= here then p.device else nil)
		if not ok or answer == nil then
			refs:set { key = key, state = "error", list = {} }
			return
		end
		answer:after(function(list: any)
			if refs:peek().key == key then
				refs:set { key = key, state = "ready", list = list or {} }
			end
		end, function()
			if refs:peek().key == key then
				refs:set { key = key, state = "error", list = {} }
			end
		end)
	end)
	-- The ref shown: the one chosen while it's among them, else the current.
	ui.effect(function()
		local r = refs:get()
		if not c.active() or r.state ~= "ready" or #r.list == 0 then
			return
		end
		local chosen = c.branch:peek()
		for _, ref in r.list do
			if ref.name == chosen then
				return
			end
		end
		local pick = r.list[1]
		for _, ref in r.list do
			if ref.current then
				pick = ref
			end
		end
		c.branch:set(pick.name)
	end)

	-- The branches as the ref list shows them: each its name, tagged
	-- current, or checked out in a worktree of its own.
	local branches = ui.computed(function(): { any }
		local out = {}
		for _, ref in refs:get().list do
			table.insert(out, {
				value = ref.name,
				label = ref.name,
				icon = "git-branch",
				tag = if ref.current
					then ui.t(if on_cloud() then "chat.where.branch.default" else "chat.where.branch.current"):get()
					elseif ref.worktree then ui.t("chat.where.branch.worktree"):get()
					else nil,
			})
		end
		return out
	end)
	local function branches_empty(query: string): any
		local r = refs:peek()
		if query ~= "" then
			return { title = ui.t("chat.where.branches_empty.no_matches"), hint = ui.t("chat.where.branches_empty.no_matches_hint", { query = query }) }
		elseif r.state == "loading" then
			return { title = ui.t("chat.where.branches_empty.loading") }
		elseif r.state == "away" then
			return { title = ui.t("chat.where.branches_empty.offline"), hint = ui.t("chat.where.branches_empty.offline_hint") }
		elseif r.state == "error" then
			if on_cloud() then
				return { title = ui.t("chat.where.branches_empty.error"), hint = ui.t("chat.where.branches_empty.error_cloud_hint") }
			end
			return { title = ui.t("chat.where.branches_empty.error"), hint = ui.t("chat.where.branches_empty.error_hint") }
		end
		return { title = ui.t("chat.where.branches_empty.none"), hint = ui.t("chat.where.branches_empty.none_hint") }
	end

	-- Why a new chat can't start where it's set to, if it can't.
	local blocked = ui.computed(function(): string?
		if not c.active() then
			return nil
		end
		local id = c.project:get()
		if id and id ~= M.NO_PROJECT then
			local p = project_row(id)
			if not p then
				return ui.t("chat.where.blocked.project_gone"):get()
			end
		end
		if on_cloud() and (id == nil or id == M.NO_PROJECT) then
			return ui.t("chat.where.blocked.cloud_needs_project"):get()
		end
		return nil
	end)
	-- What a send does, from the choices as they stand.
	local function plan(): Plan
		local p = project_row(c.project:peek())
		if not p then
			return { project = nil, device = c.device:peek(), branch = nil, cwd = nil, worktree = false }
		end
		local name = c.branch:peek()
		-- On Cloud: made there, run in its machine's checkout of the
		-- repository (its folder comes back as the chat is made).
		if c.device:peek() == M.CLOUD and cloud_repo(p) then
			return { project = p.id, device = nil, branch = name, cwd = nil, worktree = false, cloud = true }
		end
		-- (Only a repository can make a worktree: in a plain folder the host
		-- would refuse the run, so the pick waits for the next repository.)
		if c.checkout:peek() == "worktree" and p.git then
			return { project = p.id, device = p.device, branch = name, cwd = p.path, worktree = true }
		end
		-- A local checkout: the folder as it is; a ref checked out in a
		-- worktree of its own runs there.
		for _, ref in refs:peek().list do
			if ref.name == name and ref.worktree then
				return { project = p.id, device = p.device, branch = name, cwd = ref.worktree, worktree = false }
			end
		end
		return { project = p.id, device = p.device, branch = name, cwd = p.path, worktree = false }
	end

	return {
		devices = devices,
		projects = projects,
		branches = branches,
		branches_empty = branches_empty,
		blocked = blocked,
		plan = plan,
		refs = refs,
	}
end

return M
