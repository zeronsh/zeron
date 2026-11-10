-- Cloud, as the host reads it from the engine (src/cloud.rs): whether the
-- account has Cloud and it's on, the credits left, and the GitHub
-- repositories Cloud can open. Settings → Cloud shows and changes it; the
-- new chat's Work in offers Cloud for a project whose repository it can
-- open (screens/chat/where).
--
--   cloud.now:get()   { phase = "unknown" | "loading" | "ready" | "failed",
--                       status?, repos?, repos_loading, failure? }
--   cloud.load()        Cloud's status, then its repositories while it's on
--   cloud.load_repos()  the repositories again (after choosing on GitHub);
--                       `repos_failure` says why one didn't come
--                       ("github_not_connected", "offline", "failed")
--   cloud.offered()     signed in and the account has Cloud (tracked)
--   cloud.on()          …and it's on (tracked)
--   cloud.reaches(name) the repository of that `owner/name`, if Cloud can
--                       open it (tracked)
--   cloud.usage(from, to)              a promise of the credits used per day
--   cloud.branches(repo, default?)     a promise of a repository's branches
--
-- A chat's machine (what's said of it: screens/chat/cloud_machine):
--
--   cloud.sessions:get()        the Cloud sessions, as last read
--   cloud.session_for(chat, device)  a chat's session (tracked); a side chat
--                               shares its parent's, by its machine's device
--   cloud.on_device(device)     whether that device is a Cloud machine
--   cloud.follow(on)            read the sessions while `on()` says a Cloud chat
--                               is showing: closely while one is settling or
--                               updating, and for a minute after that or after
--                               a send (`sending`), else rarely, never
--                               redrawing unless they changed
--   cloud.sending(id)           message `id` is on its way to a Cloud machine
--   cloud.machines()            the sizes on offer (tracked); none from an edge
--                               that predates choosing one
--   cloud.machine()             the size new chats start on (tracked)
--   cloud.set_machine(id)       choose it, shown at once, put back if refused
--   cloud.wake(chat, machine?)  wake a chat's machine (on that size)
--   cloud.resize(chat, machine) a machine that's up: to sleep, and back on it
--   cloud.fresh(chat, machine?) files expired: a fresh machine, same repository
--                               and branch
--   cloud.wake_state(chat)      { busy?, failure? } of the last wake (tracked)
--   cloud.clear_failure(chat)   that failure read
--
-- A status: { state, available, device?, credits?, error?, failed_action?,
-- awake?, max_awake?, machines?, machine? }. A repository: { name, private,
-- branch?, pushed, account?, accounts?, installation? }: `account`, the GitHub
-- login a new chat on it would use; `accounts`, every one that reaches it;
-- `installation`, the GitHub App's installation that gives the access. (None
-- from an older vault.) Every function here is a field, so a fixture answers in
-- the host's place (`ask_*`: the host's calls alone, what's done with their
-- answers staying as it is here).

local auth = require("@account/auth")

local M = {}

export type Status = {
	state: string,
	available: boolean,
	device: string?,
	credits: number?,
	error: string?,
	failed_action: string?,
	awake: number?,
	max_awake: number?,
	machines: { Machine }?,
	machine: string?,
}
-- A size of machine, as the edge offers it. `rate`: its credits relative to
-- the standard machine's (xlarge 5.56).
export type Machine = { id: string, name: string?, vcpu: number, memory_gb: number, disk_gb: number, rate: number }
export type Disk = { used: number, capacity: number, level: string }
export type Session = {
	chat: string,
	device: string,
	-- provisioning, starting, ready, sleeping, stopping, deleting, error,
	-- data_expired, off.
	state: string,
	space: string?,
	repo: string?,
	error: string?,
	failed_action: string?,
	machine: string?,
	stage: string?,
	disk: Disk?,
	wake_blocked: { reason: string?, needed: number? }?,
	-- When its files go, in Unix seconds, while it sleeps.
	expires: number?,
	-- When its machine last rebuilt its chat store from the server copy, in
	-- Unix seconds (nil from an older edge).
	restored: number?,
}
export type Repo = {
	name: string,
	private: boolean,
	branch: string?,
	clone_url: string?,
	pushed: number,
	account: string?,
	accounts: { string }?,
	installation: number?,
}
export type Now = {
	phase: string,
	status: Status?,
	repos: { Repo }?,
	repos_loading: boolean,
	repos_failure: string?,
	failure: string?,
	-- Why the size chosen for new chats didn't take.
	machine_failure: string?,
}

M.now = ui.state("account_cloud", { phase = "unknown", repos_loading = false } :: Now)
-- A read still out when the scripts reload never answers these ones (its
-- promise was the old scripts'): asked again from here.
do
	local was = M.now:peek()
	if was.phase == "loading" or was.repos_loading then
		local next = table.clone(was) :: any
		next.phase = if was.status then "ready" else "unknown"
		next.repos_loading = false
		M.now:set(next)
	end
end

local function patch(fields: { [string]: any })
	local next = table.clone(M.now:peek()) :: any
	for k, v in fields do
		next[k] = v
	end
	M.now:set(next)
end

-- The sessions as last read, and a wake's progress by chat.
M.sessions = ui.state("account_cloud_sessions", {} :: { Session })
M.wakes = ui.state("account_cloud_wakes", {} :: { [string]: { busy: boolean?, failure: string? } })
-- (As above: a wake out when the scripts reload never answers these ones.)
do
	local was = M.wakes:peek()
	if next(was) ~= nil then
		local next_wakes = {}
		for chat, w in was do
			if w.failure then
				next_wakes[chat] = { failure = w.failure }
			end
		end
		M.wakes:set(next_wakes)
	end
end

-- Cloud is on: a Cloud of the account's to run on (not being turned off).
function M.is_on(s: Status?): boolean
	return s ~= nil and s.available and s.device ~= nil and s.state ~= "off" and s.state ~= "deleting"
end

function M.offered(): boolean
	local s = M.now:get().status
	return auth.state:get() == "signed_in" and s ~= nil and s.available
end

function M.on(): boolean
	return M.offered() and M.is_on(M.now:get().status)
end

function M.reaches(name: string?): Repo?
	if not name or not M.on() then
		return nil
	end
	local wanted = string.lower(name)
	for _, r in M.now:get().repos or {} do
		if string.lower(r.name) == wanted then
			return r
		end
	end
	return nil
end

-- Take a status (a read, or an action's answer); its repositories follow
-- while Cloud is on.
function M.take(s: Status)
	local was_on = M.is_on(M.now:peek().status)
	patch { phase = "ready", status = s, failure = nil :: any }
	if M.is_on(s) then
		if not was_on or M.now:peek().repos == nil then
			M.load_repos()
		end
	elseif M.now:peek().repos ~= nil then
		patch { repos = nil :: any }
	end
end

function M.load()
	if M.now:peek().phase == "unknown" or M.now:peek().phase == "failed" then
		patch { phase = "loading" }
	end
	ui.call("cloud.status", nil):after(function(s: any)
		M.take(s)
	end, function(e: any)
		-- (What was known stays: a failed re-read says so beside it.)
		if M.now:peek().status then
			patch { failure = e.message }
		else
			patch { phase = "failed", failure = e.message }
		end
	end)
end

-- (`done`: called once the read lands, either way.)
function M.load_repos(done: (() -> ())?)
	patch { repos_loading = true }
	ui.call("cloud.repos", nil):after(function(list: any)
		patch { repos = list or {}, repos_loading = false, repos_failure = nil :: any }
		if done then
			done()
		end
	end, function(e: any)
		-- (A failed read keeps what was known, but GitHub gone is gone.)
		local word = if type(e) == "table" and type(e.message) == "string" then e.message else "failed"
		if word == "github_not_connected" then
			patch { repos = {}, repos_loading = false, repos_failure = word }
		else
			patch { repos_loading = false, repos_failure = word }
		end
		if done then
			done()
		end
	end)
end

-- The host's calls, each a field a fixture answers.
function M.ask_sessions(): any
	return ui.call("cloud.sessions", nil)
end
function M.ask_wake(args: { [string]: any }): any
	return ui.call("cloud.wake", args)
end
function M.ask_sleep(args: { [string]: any }): any
	return ui.call("cloud.sleep", args)
end
function M.ask_retry(args: { [string]: any }): any
	return ui.call("cloud.retry", args)
end
function M.ask_set_machine(machine: string): any
	return ui.call("cloud.set_machine", { machine = machine })
end

function M.machines(): { Machine }
	local s = M.now:get().status
	return (s and s.machines) or {}
end

function M.machine(): string?
	local s = M.now:get().status
	return s and s.machine
end

function M.on_device(device: string?): boolean
	return device ~= nil and string.sub(device, 1, 6) == "cloud-"
end

function M.session_for(chat: string?, device: string?): Session?
	local list = M.sessions:get()
	if chat then
		for _, s in list do
			if s.chat == chat then
				return s
			end
		end
	end
	if device then
		for _, s in list do
			if s.device == device then
				return s
			end
		end
	end
	return nil
end

-- Two reads the same? (A read that changed nothing draws nothing.)
local function same(a: any, b: any): boolean
	if type(a) ~= "table" or type(b) ~= "table" then
		return a == b
	end
	for k, v in a do
		if not same(v, b[k]) then
			return false
		end
	end
	for k in b do
		if a[k] == nil then
			return false
		end
	end
	return true
end

local READ_GAP_MS = 1500
local reading = false
local read_at = -READ_GAP_MS

-- States that settle on their own, followed closely; and a machine up that
-- installs a release before it answers, or a wake far slower than normal
-- that the edge is kicking (its stages, taken back when it's done).
local SETTLING = { provisioning = true, starting = true, stopping = true, deleting = true }
local SETTLING_EVERY, RESTING_EVERY = 3000, 60000
local function holds(s: Session): boolean
	return SETTLING[s.state] == true or s.stage == "updating" or s.stage == "slow"
end

-- Followed closely this long (seconds) after a message went to a Cloud
-- machine, and after a machine last read as settling or updating: a woken
-- machine reads ready once its engine is up, and only then says whether it
-- updates first.
local WATCH = 60
local held_at = -math.huge
local sends_seen: { [string]: boolean } = {}
local WATCH_TICK = ui.state("account_cloud_watch", 0)

local function note_holding(list: { Session })
	for _, s in list do
		if holds(s) then
			held_at = os.time()
			return
		end
	end
end

-- (Several panes may follow at once, and a wake reads as it lands: one read
-- out at a time, none straight after another.)
function M.load_sessions(force: boolean?)
	if reading or (not force and ui.now() - read_at < READ_GAP_MS) then
		return
	end
	reading, read_at = true, ui.now()
	M.ask_sessions():after(function(list: any)
		reading = false
		local next_list = list or {}
		note_holding(next_list)
		if not same(M.sessions:peek(), next_list) then
			M.sessions:set(next_list)
		end
	end, function()
		-- (What was known stays: a poll that failed says nothing.)
		reading = false
	end)
end

-- One session's answer, in the list.
local function take_session(s: Session)
	note_holding({ s })
	local list = table.clone(M.sessions:peek())
	for i, was in list do
		if was.chat == s.chat then
			list[i] = s
			M.sessions:set(list)
			return
		end
	end
	table.insert(list, s)
	M.sessions:set(list)
end

-- A message (by its id) is on its way to a Cloud machine: the sessions are
-- followed closely for a while (WATCH), once per message.
function M.sending(id: string)
	if sends_seen[id] then
		return
	end
	sends_seen[id] = true
	held_at = math.max(held_at, os.time())
	-- (Said while a row is drawn: the follow hears of it after.)
	ui.after(0, function()
		WATCH_TICK:set(WATCH_TICK:peek() + 1)
	end)
end

function M.follow(on: () -> boolean)
	local settling = ui.computed(function(): boolean
		for _, s in M.sessions:get() do
			if holds(s) then
				return true
			end
		end
		return false
	end)
	ui.effect(function()
		if not on() then
			return
		end
		if M.now:peek().status == nil then
			M.load()
		end
		M.load_sessions()
		WATCH_TICK:get()
		local close = settling:get()
		local left = held_at + WATCH - os.time()
		if not close and left >= 0 then
			-- Still in a send's or a wake's watch: closely until it's over.
			close = true
			ui.after(left * 1000 + 100, function()
				WATCH_TICK:set(WATCH_TICK:peek() + 1)
			end)
		end
		-- (A timer made in an effect belongs to its run. A read alone draws
		-- nothing: the window still rests between them.)
		ui.every(if close then SETTLING_EVERY else RESTING_EVERY, function()
			M.load_sessions()
		end)
	end)
end

function M.wake_state(chat: string?): { busy: boolean?, failure: string? }
	return (chat and M.wakes:get()[chat]) or {}
end

local function set_wake(chat: string, w: { busy: boolean?, failure: string? }?)
	M.wakes:set_key(chat, w :: any)
end

function M.clear_failure(chat: string)
	if M.wakes:peek()[chat] then
		set_wake(chat, nil)
	end
end

local function word_of(e: any): string
	return if type(e) == "table" and type(e.message) == "string" then e.message else "failed"
end

-- One action on a chat's machine: busy until it lands, its session taken, or
-- why not kept. (Refused for its size: what it needs may have changed, so the
-- sessions are read again either way.)
local function act(chat: string, ask: () -> any)
	set_wake(chat, { busy = true })
	ask():after(function(s: any)
		set_wake(chat, nil)
		if s then
			take_session(s)
		end
		M.load_sessions(true)
	end, function(e: any)
		set_wake(chat, { failure = word_of(e) })
		M.load_sessions(true)
	end)
end

local function is_busy(chat: string): boolean
	local w = M.wakes:peek()[chat]
	return w ~= nil and w.busy == true
end

function M.wake(chat: string, machine: string?)
	if not is_busy(chat) then
		act(chat, function(): any
			return M.ask_wake({ chat = chat, machine = machine })
		end)
	end
end

function M.fresh(chat: string, machine: string?)
	if not is_busy(chat) then
		act(chat, function(): any
			return M.ask_retry({ chat = chat, machine = machine })
		end)
	end
end

-- A machine that's up won't change size (the edge's `machine_awake`): it's put
-- to sleep, and a wake on the new size is taken as it stops.
function M.resize(chat: string, machine: string)
	if is_busy(chat) then
		return
	end
	set_wake(chat, { busy = true })
	M.ask_sleep({ chat = chat }):after(function()
		set_wake(chat, nil)
		M.wake(chat, machine)
	end, function(e: any)
		set_wake(chat, { failure = word_of(e) })
	end)
end

function M.set_machine(id: string)
	local status = M.now:peek().status
	if not status or status.machine == id then
		return
	end
	local was = status.machine
	local shown = table.clone(status) :: any
	shown.machine = id
	patch { status = shown, machine_failure = nil :: any }
	M.ask_set_machine(id):after(function(s: any)
		if s then
			M.take(s)
		end
	end, function(e: any)
		local back = table.clone(M.now:peek().status or status) :: any
		back.machine = was
		patch { status = back, machine_failure = word_of(e) }
	end)
end

function M.usage(from: string, to: string): any
	return ui.call("cloud.usage", { from = from, to = to })
end

function M.branches(repo: string, default: string?): any
	return ui.call("cloud.branches", { repo = repo, default = default })
end

-- The logical Cloud device's id, while Cloud is on (where a Cloud project
-- lives).
function M.device(): string?
	local s = M.now:get().status
	return if M.on() and s then s.device else nil
end

-- Signed out, nothing of the account's Cloud stays for the next one.
function M.forget()
	M.now:set { phase = "unknown", repos_loading = false }
	M.sessions:set({})
	M.wakes:set({})
end

return M
