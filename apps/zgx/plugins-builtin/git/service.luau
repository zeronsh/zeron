--!strict
-- Git's service: run by Zeron's engine on each device that has the plugin,
-- with no window open. It is where a session's checkout is (this Mac, or a
-- devbox), and it does every piece of git work there with that device's
-- own `git` ("commands": ["git"]), for the plugin's windows on this device
-- or another of yours. Nothing here is Zeron's: it is git, asked plainly.
--
-- The work tree's state, as one id: a snapshot of every file in it (the
-- tracked ones and the new ones git doesn't ignore) written as a tree into
-- the repository's own objects, through an index of the plugin's own
-- (`GIT_INDEX_FILE`, beside the repository's, never the repository's
-- index). Diffs are between trees, so "what changed" is the same question
-- whatever it's measured from: the last commit (Working tree), where the
-- branch left its base (Branch), where a chat's turn started (Last turn),
-- or a commit's parent (History).
--
-- A failure is `error("<code>: <words>", 0)`: the window reads the code
-- (`not_a_repo`, `rejected`, `conflicts`, `dirty`, …) to say it plainly,
-- and git's own words where nothing better fits.

local zeron = require("./zeron")("git")

-- Every run: git's words untranslated (they are read), no prompt for a
-- password (a push waits for none), and `status` taking no lock an agent's
-- git could trip over.
local ENV = { LC_ALL = "C", GIT_TERMINAL_PROMPT = "0", GIT_OPTIONAL_LOCKS = "0" }
-- What git prints and the plugin reads: never its colours or a pager.
local BASE = { "-c", "core.quotepath=off", "-c", "color.ui=false", "-c", "core.pager=cat" }
-- A file's diff is sent whole up to this; past it, cut (and said).
local MAX_DIFF = 600 * 1024
-- The most files a list sends (a thousand reads; a vendored tree doesn't).
local MAX_FILES = 5000

local function merged(a: { [string]: string }, b: { [string]: string }?): { [string]: string }
	local out = table.clone(a)
	for k, v in b or {} do
		out[k] = v
	end
	return out
end

type Ran = { status: number, stdout: string, stderr: string }

-- `git -C cwd …`, as it ended.
local function run(cwd: string, args: { string }, opts: { stdin: string?, timeout: number?, env: { [string]: string }? }?): Ran
	local all = table.clone(BASE)
	table.insert(all, 1, cwd)
	table.insert(all, 1, "-C")
	for _, a in args do
		table.insert(all, a)
	end
	local o = opts or {}
	return zeron.run("git", all, { stdin = o.stdin, timeout = o.timeout or 30, env = merged(ENV, o.env) }):await()
end

-- git's own words for a failure, on one line.
local function said(r: Ran): string
	local text = if r.stderr ~= "" then r.stderr else r.stdout
	text = string.gsub(text, "^%s+", "")
	text = string.gsub(text, "%s+$", "")
	return if text == "" then `git stopped ({r.status})` else text
end

-- git's words as a person reads them: its first line that says something
-- (past "hint:" lines), without "fatal:"/"error:", a capital to start.
local function first_line(words: string): string
	for line in string.gmatch(words .. "\n", "([^\n]*)\n") do
		local l = string.gsub(string.gsub(line, "^%s+", ""), "%s+$", "")
		if l ~= "" and not string.match(l, "^hint:") then
			l = string.gsub(l, "^fatal:%s*", "")
			l = string.gsub(l, "^error:%s*", "")
			l = string.gsub(l, "^warning:%s*", "")
			return (string.gsub(l, "^%l", string.upper))
		end
	end
	return words
end

-- A failure of git's with the code the window has words for: a name git
-- doesn't know here, a folder it won't work in, a lock left behind; else
-- `git:` and its own first line.
local function classified(words: string): string
	if string.find(words, "not a git repository", 1, true) then
		return "not_a_repo: " .. first_line(words)
	end
	local name = string.match(words, "ambiguous argument '([^']+)'")
		or string.match(words, "bad revision '([^']+)'")
		or string.match(words, "Not a valid object name ([^%s]+)")
		or string.match(words, "invalid reference: ([^%s]+)")
	if name then
		return "no_ref: " .. name
	end
	if string.find(words, "dubious ownership", 1, true) then
		return "unsafe_dir: " .. first_line(words)
	end
	if string.find(words, "index.lock", 1, true) then
		return "locked: " .. first_line(words)
	end
	return "git: " .. first_line(words)
end

-- `git -C cwd …`'s output, or a failure with a code.
local function git(cwd: string, args: { string }, opts: any?): string
	local r = run(cwd, args, opts)
	if r.status == -1 then
		error("timeout: git took too long", 0)
	end
	if r.status ~= 0 then
		error(classified(said(r)), 0)
	end
	return r.stdout
end

local short_refs: (decorated: string) -> ({ string }, { string })

local function trim(s: string): string
	return (string.gsub(string.gsub(s, "^%s+", ""), "%s+$", ""))
end

local function check_cwd(cwd: any): string
	if type(cwd) ~= "string" or cwd == "" or string.sub(cwd, 1, 1) ~= "/" then
		error("no_checkout: This chat has no folder to look in", 0)
	end
	return cwd
end

-- One piece of work at a time per checkout: the plugin's index is one file,
-- and git's own writes take the repository's lock.
local held: { [string]: boolean } = {}
local function exclusive<T>(cwd: string, fn: () -> T): T
	while held[cwd] do
		zeron.sleep(0.02):await()
	end
	held[cwd] = true
	local ok, value = pcall(fn)
	held[cwd] = nil
	if not ok then
		error(value, 0)
	end
	return value
end

-- What's known of a checkout and slow to ask: its git folder and the
-- empty tree's id (sha-1 and sha-256 repositories differ).
type Known = { dir: string, empty: string, base: string?, base_at: number?, who: string?, urls: { [string]: string }? }
local known: { [string]: Known } = {}
local function repo(cwd: string): Known
	local k = known[cwd]
	if k then
		return k
	end
	local dir = trim(git(cwd, { "rev-parse", "--absolute-git-dir" }))
	local empty = trim(git(cwd, { "hash-object", "-t", "tree", "--stdin" }, { stdin = "" }))
	k = { dir = dir, empty = empty }
	known[cwd] = k
	return k
end

-- Whether a revision names something here (one run for several).
local function exists(cwd: string, names: { string }): { [string]: string }
	local r = run(cwd, { "cat-file", "--batch-check" }, { stdin = table.concat(names, "\n") .. "\n" })
	local out: { [string]: string } = {}
	local i = 0
	for line in string.gmatch(r.stdout, "[^\n]+") do
		i += 1
		local sha = string.match(line, "^(%x+) ")
		if sha and names[i] then
			out[names[i]] = sha
		end
	end
	return out
end

-- The work tree as a tree: every file git would add, written into the
-- repository's objects through the plugin's own index (seeded from the
-- repository's, so only what changed is read again).
local function snapshot(cwd: string, head: string?): string
	local r = repo(cwd)
	local index = r.dir .. "/zeron-git.index"
	local own = { GIT_INDEX_FILE = index }
	local seeded = if head
		then run(cwd, { "read-tree", "--index-output=" .. index, "-m", head })
		else { status = 1, stdout = "", stderr = "" }
	if seeded.status ~= 0 then
		-- (No commit yet, or the index mid-merge: from nothing, or the last
		-- commit, read whole once.)
		git(cwd, if head then { "read-tree", head } else { "read-tree", "--empty" }, { env = own })
	end
	git(cwd, { "add", "-A", "--", ":/" }, { env = own, timeout = 120 })
	return trim(git(cwd, { "write-tree" }, { env = own }))
end

-- Where the branch's base is: what origin calls its own head, else the
-- usual names. Asked once a minute at most.
local function base_of(cwd: string): string?
	local r = repo(cwd)
	if r.base_at and os.time() - r.base_at < 60 then
		return r.base
	end
	local base: string? = nil
	-- origin's own head first, if the branch it names is still here (a
	-- remote whose default was renamed and pruned leaves it naming none),
	-- else the usual names.
	local sym = run(cwd, { "symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD" })
	local candidates = { "origin/main", "origin/master", "main", "master" }
	if sym.status == 0 and trim(sym.stdout) ~= "" then
		table.insert(candidates, 1, trim(sym.stdout))
	end
	local found = exists(cwd, candidates)
	for _, c in candidates do
		if found[c] then
			base = c
			break
		end
	end
	r.base, r.base_at = base, os.time()
	-- (Who commits here, and where each remote is, asked with it.)
	r.who = trim(run(cwd, { "config", "user.name" }).stdout)
	local urls: { [string]: string } = {}
	for line in string.gmatch(run(cwd, { "remote", "-v" }).stdout, "[^\n]+") do
		local name, url = string.match(line, "^(%S+)%s+(%S+)%s+%(fetch%)")
		if name then
			urls[name] = url
		end
	end
	r.urls = urls
	return base
end

-- A short checksum of text (what status said of the index): the window
-- asks for the files again when it moves.
local function checksum(text: string): string
	local h = 5381
	for i = 1, #text do
		h = (h * 33 + string.byte(text, i)) % 4294967291
	end
	return string.format("%08x%d", h, #text)
end

-- -- status ---------------------------------------------------------------

export type File = { path: string, old: string?, status: string, x: string?, y: string?, stage: string?, add: number?, del: number?, rev: string? }

-- `git status --porcelain=v2 -z`: the branch's lines and a file each.
local function read_status(out: string): (any, { File })
	local head: any = { ahead = 0, behind = 0 }
	local files: { File } = {}
	local parts = string.split(out, "\0")
	local i = 1
	while i <= #parts do
		local line = parts[i]
		i += 1
		if line == "" then
			continue
		end
		local kind = string.sub(line, 1, 1)
		if kind == "#" then
			local oid = string.match(line, "^# branch%.oid (.+)$")
			local name = string.match(line, "^# branch%.head (.+)$")
			local up = string.match(line, "^# branch%.upstream (.+)$")
			local a, b = string.match(line, "^# branch%.ab %+(%d+) %-(%d+)$")
			if oid then
				head.oid = if oid == "(initial)" then nil else oid
			elseif name then
				head.branch = if name == "(detached)" then nil else name
			elseif up then
				head.upstream = up
			elseif a then
				head.ahead, head.behind = tonumber(a), tonumber(b)
			end
		elseif kind == "1" then
			local xy, path = string.match(line, "^1 (..) %S+ %S+ %S+ %S+ %S+ %S+ (.+)$")
			if xy and path then
				table.insert(files, { path = path, x = string.sub(xy, 1, 1), y = string.sub(xy, 2, 2), status = "modified" })
			end
		elseif kind == "2" then
			local xy, path = string.match(line, "^2 (..) %S+ %S+ %S+ %S+ %S+ %S+ %S+ (.+)$")
			local old = parts[i]
			i += 1
			if xy and path then
				table.insert(files, { path = path, old = old, x = string.sub(xy, 1, 1), y = string.sub(xy, 2, 2), status = "renamed" })
			end
		elseif kind == "u" then
			local path = string.match(line, "^u .. %S+ %S+ %S+ %S+ %S+ %S+ %S+ %S+ (.+)$")
			if path then
				table.insert(files, { path = path, x = "U", y = "U", status = "conflict" })
			end
		elseif kind == "?" then
			table.insert(files, { path = string.sub(line, 3), x = "?", y = "?", status = "added" })
		end
	end
	for _, f in files do
		if f.status == "conflict" then
			f.stage = "none"
		elseif f.x == "?" then
			f.stage = "none"
		elseif f.x ~= "." and f.y == "." then
			f.stage = "all"
		elseif f.x ~= "." then
			f.stage = "some"
		else
			f.stage = "none"
		end
		if f.status == "modified" then
			local c = if f.x ~= "." then f.x else f.y
			if c == "A" then
				f.status = "added"
			elseif c == "D" then
				f.status = "deleted"
			end
		end
	end
	return head, files
end

local function status_now(cwd: string): any
	local out = git(cwd, { "status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all" }, { timeout = 60 })
	local head, files = read_status(out)
	head.index = checksum(out)
	local states = exists(cwd, { "MERGE_HEAD", "REBASE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD" })
	head.state = if states.REBASE_HEAD
		then "rebase"
		elseif states.MERGE_HEAD then "merge"
		elseif states.CHERRY_PICK_HEAD then "cherry_pick"
		elseif states.REVERT_HEAD then "revert"
		else nil
	local conflicts = 0
	for _, f in files do
		if f.status == "conflict" then
			conflicts += 1
		end
	end
	head.conflicts = conflicts
	head.changes = #files
	return head, files
end

-- The branch's state, and the work tree's as one id (`tree`): the window
-- asks again for the files only when it moves.
local function status(cwd: string): any
	return exclusive(cwd, function(): any
		local head, _files = status_now(cwd)
		head.tree = snapshot(cwd, head.oid)
		head.device = zeron.device.name
		local base = base_of(cwd)
		head.base = base
		head.who = repo(cwd).who
		if base and head.oid then
			local counted = run(cwd, { "rev-list", "--left-right", "--count", `{base}...HEAD` })
			local behind, ahead = string.match(counted.stdout, "^(%d+)%s+(%d+)")
			if ahead then
				head.base_ahead, head.base_behind = tonumber(ahead), tonumber(behind)
			end
		end
		if head.upstream then
			head.pushed = head.ahead == 0
		else
			head.pushed = false
		end
		local remotes = run(cwd, { "remote" })
		head.remote = string.match(remotes.stdout, "^([^\n]+)")
		-- (The upstream's remote, else the first: where a push goes.)
		local remote = (head.upstream and string.match(head.upstream, "^([^/]+)/")) or head.remote
		head.remote_url = remote and (repo(cwd).urls or {})[remote] or nil
		return head
	end)
end

-- -- what changed --------------------------------------------------------

local STATUS_OF: { [string]: string } = { A = "added", D = "deleted", M = "modified", R = "renamed", C = "added", T = "modified", U = "conflict" }

-- `git diff --raw --numstat -z a b`: each file's status and its lines.
local function read_diff(out: string): { File }
	local parts = string.split(out, "\0")
	local files: { File } = {}
	local by_path: { [string]: File } = {}
	local i = 1
	while i <= #parts do
		local p = parts[i]
		i += 1
		if string.sub(p, 1, 1) == ":" then
			local src, dst, letter = string.match(p, "^:%S+ %S+ (%x+) (%x+) (%a)")
			-- (Its content's id: what it is now, or was, if it's gone.)
			local rev = if dst and not string.match(dst, "^0+$") then dst else src
			local f: File
			if letter == "R" or letter == "C" then
				local old, new = parts[i], parts[i + 1]
				i += 2
				f = { path = new, old = if letter == "R" then old else nil, status = STATUS_OF[letter] or "modified", rev = rev }
			else
				local path = parts[i]
				i += 1
				f = { path = path, status = STATUS_OF[letter or "M"] or "modified", rev = rev }
			end
			table.insert(files, f)
			by_path[f.path] = f
		elseif p ~= "" then
			-- numstat: "add\tdel\tpath", or "add\tdel\t" then old, new.
			local a, d, path = string.match(p, "^([%d%-]+)\t([%d%-]+)\t(.*)$")
			if a then
				if path == "" then
					i += 1
					path = parts[i]
					i += 1
				end
				local f = by_path[path]
				if f then
					f.add = tonumber(a) or 0
					f.del = tonumber(d) or 0
					if a == "-" then
						f.binary = true :: any
					end
				end
			end
		end
	end
	return files
end

local function diff_files(cwd: string, a: string, b: string): { File }
	local out = git(cwd, { "diff", "--raw", "--numstat", "-z", "-M", "--no-ext-diff", a, b }, { timeout = 60 })
	return read_diff(out)
end

-- The turns this device saw start, by chat: where each one's work tree
-- stood (kept on this device; the newest 200).
local turns = zeron.store("turns", {})

local function changes(args: any): any
	local cwd = check_cwd(args.cwd)
	local scope = args.scope or "working"
	return exclusive(cwd, function(): any
		local r = repo(cwd)
		local head, files = status_now(cwd)
		local tree = snapshot(cwd, head.oid)
		local from = head.oid or r.empty
		local out: any = { scope = scope, b = tree, head = head }
		if scope == "working" then
			-- Each file's staging as status says, its lines as the trees do.
			local lines = diff_files(cwd, from, tree)
			local by_path: { [string]: File } = {}
			for _, f in lines do
				by_path[f.path] = f
			end
			for _, f in files do
				local d = by_path[f.path]
				f.add = if d then d.add else 0
				f.del = if d then d.del else 0
				if d and (d :: any).binary then
					(f :: any).binary = true
				end
				-- (Its content and its staging: a file staged anew is a new row.)
				f.rev = `{if d then d.rev or "" else ""}{f.x}{f.y}`
				f.x, f.y = nil, nil
			end
			out.a = from
			out.files = files
		elseif scope == "branch" then
			local base = args.base or base_of(cwd)
			if not base then
				error("no_base: There's no branch to compare with", 0)
			end
			-- (One asked for that isn't here: said so, not as no history.)
			if not exists(cwd, { base })[base] then
				error(`no_ref: {base}`, 0)
			end
			local mb = run(cwd, { "merge-base", base, "HEAD" })
			if mb.status ~= 0 then
				error(`no_base: {base} and this branch share no history`, 0)
			end
			out.a = trim(mb.stdout)
			out.base = base
			out.files = diff_files(cwd, out.a, tree)
		elseif scope == "turn" then
			local turn = (turns:get() or {})[args.chat or ""]
			if not turn or turn.cwd ~= cwd then
				error("no_turn: No turn has started here since Zeron was set up", 0)
			end
			out.a = turn.tree
			out.at = turn.at
			out.files = diff_files(cwd, turn.tree, tree)
		elseif scope == "commit" then
			local sha = args.sha
			assert(type(sha) == "string" and string.match(sha, "^%x+$"), "commit needs its sha")
			local parents = string.split(trim(git(cwd, { "rev-list", "--parents", "-n", "1", sha })), " ")
			out.a = if parents[2] then parents[2] else r.empty
			out.b = parents[1]
			out.files = diff_files(cwd, out.a, out.b)
		else
			error(`bad_scope: {scope}`, 0)
		end
		if #out.files > MAX_FILES then
			out.total = #out.files
			local kept = {}
			for i = 1, MAX_FILES do
				kept[i] = out.files[i]
			end
			out.files = kept
		end
		return out
	end)
end

-- One file's patch between two trees (what `changes` said they were).
local function diff(args: any): any
	local cwd = check_cwd(args.cwd)
	assert(type(args.a) == "string" and type(args.b) == "string" and type(args.path) == "string", "diff needs a, b and path")
	local paths = { args.path }
	if type(args.old) == "string" and args.old ~= args.path then
		table.insert(paths, 1, args.old)
	end
	local cmd = { "diff", "--no-ext-diff", "-M", args.a, args.b, "--" }
	for _, p in paths do
		table.insert(cmd, p)
	end
	local text = git(cwd, cmd, { timeout = 60 })
	local cut = #text > MAX_DIFF
	if cut then
		text = string.sub(text, 1, MAX_DIFF)
		text = string.match(text, "^(.*)\n") or text
	end
	return { text = text, cut = cut }
end

-- -- history and branches ------------------------------------------------

local SEP, END = "\x1f", "\x1e"

local function history(args: any): any
	local cwd = check_cwd(args.cwd)
	local limit = math.clamp(tonumber(args.limit) or 200, 1, 1000)
	local skip = math.max(0, tonumber(args.skip) or 0)
	local head = run(cwd, { "rev-parse", "-q", "--verify", "HEAD" })
	if head.status ~= 0 then
		return { commits = {}, more = false }
	end
	-- This branch, and where it goes and came from: its upstream and base;
	-- or (`all`) every branch, remote and tag.
	local refs = { "HEAD" }
	if args.all then
		refs = { "HEAD", "--branches", "--remotes", "--tags" }
	else
		local up = run(cwd, { "rev-parse", "-q", "--abbrev-ref", "--symbolic-full-name", "@{upstream}" })
		if up.status == 0 and trim(up.stdout) ~= "" then
			table.insert(refs, trim(up.stdout))
		end
		local base = base_of(cwd)
		if base and base ~= refs[2] then
			table.insert(refs, base)
		end
	end
	local cmd = {
		"log",
		"--topo-order",
		-- (Refs whole, so a local branch with a slash in its name is told
		-- from a remote's: said back short, the remote ones listed.)
		"--decorate=full",
		`--format=%H{SEP}%P{SEP}%an{SEP}%ae{SEP}%at{SEP}%D{SEP}%s{END}`,
		"-n",
		tostring(limit + 1),
		"--skip",
		tostring(skip),
	}
	-- A search: commits whose message says it, in any case. (Who made one,
	-- and its id, the window matches among those it has: git's --author
	-- and --grep together want both.)
	local query = if type(args.query) == "string" then trim(args.query) else ""
	if query ~= "" then
		table.insert(cmd, "-i")
		table.insert(cmd, "--fixed-strings")
		table.insert(cmd, `--grep={query}`)
	end
	for _, r in refs do
		table.insert(cmd, r)
	end
	table.insert(cmd, "--")
	local out = git(cwd, cmd, { timeout = 60 })
	local commits = {}
	for rec in string.gmatch(out, "([^\x1e]+)\x1e") do
		rec = string.gsub(rec, "^\n", "")
		local f = string.split(rec, SEP)
		if #f >= 7 then
			local refs_of, remotes = short_refs(f[6])
			table.insert(commits, {
				sha = f[1],
				parents = if f[2] == "" then {} else string.split(f[2], " "),
				author = f[3],
				email = f[4],
				at = tonumber(f[5]) or 0,
				refs = refs_of,
				remotes = remotes,
				subject = f[7],
			})
		end
	end
	local more = #commits > limit
	if more then
		table.remove(commits)
	end
	return { commits = commits, more = more, refs = refs }
end

local function branches(args: any): any
	local cwd = check_cwd(args.cwd)
	local out = git(cwd, {
		"for-each-ref",
		"--sort=-committerdate",
		`--format=%(refname){SEP}%(refname:short){SEP}%(committerdate:unix){SEP}%(upstream:short){SEP}%(upstream:track){SEP}%(HEAD){SEP}%(contents:subject)`,
		"refs/heads",
		"refs/remotes",
	})
	local list = {}
	local locals: { [string]: boolean } = {}
	for line in string.gmatch(out, "[^\n]+") do
		local f = string.split(line, SEP)
		local full, short = f[1], f[2]
		if string.match(full, "/HEAD$") then
			continue
		end
		local remote = string.sub(full, 1, 13) == "refs/remotes/"
		local ahead = tonumber(string.match(f[5] or "", "ahead (%d+)") or "0")
		local behind = tonumber(string.match(f[5] or "", "behind (%d+)") or "0")
		if not remote then
			locals[short] = true
		end
		table.insert(list, {
			name = short,
			remote = remote,
			at = tonumber(f[3]) or 0,
			upstream = if f[4] ~= "" then f[4] else nil,
			ahead = ahead,
			behind = behind,
			current = f[6] == "*",
			subject = f[7],
		})
	end
	-- A remote branch with a local one of its name is that one.
	local out_list = {}
	for _, b in list do
		if b.remote then
			local local_name = string.match(b.name, "^[^/]+/(.+)$")
			if local_name and locals[local_name] then
				continue
			end
			b.local_name = local_name
		end
		table.insert(out_list, b)
	end
	return { branches = out_list }
end

-- One commit whole: its message's body, who made and committed it and
-- when, its parents, and what it adds and takes in lines.
-- `%D` as `--decorate=full` writes it, said as git's short names ("HEAD ->
-- main", "origin/main", "tag: v1"), and which of them are a remote's.
short_refs = function(decorated: string): ({ string }, { string })
	local refs, remotes = {}, {}
	for name in string.gmatch(decorated, "[^,]+") do
		name = trim(name)
		if name ~= "" then
			local head = string.match(name, "^HEAD %-> (.+)$")
			local full = head or name
			local short = full
			local remote = false
			if string.sub(full, 1, 11) == "refs/heads/" then
				short = string.sub(full, 12)
			elseif string.sub(full, 1, 13) == "refs/remotes/" then
				short = string.sub(full, 14)
				remote = true
			elseif string.sub(full, 1, 15) == "tag: refs/tags/" then
				short = "tag: " .. string.sub(full, 16)
			end
			table.insert(refs, if head then `HEAD -> {short}` else short)
			if remote then
				table.insert(remotes, short)
			end
		end
	end
	return refs, remotes
end

local function commit_of(args: any): any
	local cwd = check_cwd(args.cwd)
	local sha = args.sha
	assert(type(sha) == "string" and string.match(sha, "^%x+$"), "commit needs its sha")
	local out = git(cwd, { "show", "-s", `--format=%H{SEP}%P{SEP}%an{SEP}%ae{SEP}%at{SEP}%cn{SEP}%ce{SEP}%ct{SEP}%D{SEP}%s{SEP}%b`, sha })
	local f = string.split(out, SEP)
	local stat = run(cwd, { "diff-tree", "--root", "--no-commit-id", "-r", "--numstat", "-M", sha })
	local adds, dels, files = 0, 0, 0
	for a, d in string.gmatch(stat.stdout, "([%d%-]+)\t([%d%-]+)\t[^\n]*") do
		files += 1
		adds += tonumber(a) or 0
		dels += tonumber(d) or 0
	end
	return {
		sha = f[1],
		parents = if (f[2] or "") == "" then {} else string.split(f[2], " "),
		author = f[3],
		email = f[4],
		at = tonumber(f[5]) or 0,
		committer = f[6],
		committer_email = f[7],
		committed = tonumber(f[8]) or 0,
		refs = f[9],
		subject = f[10],
		body = trim(f[11] or ""),
		files = files,
		adds = adds,
		dels = dels,
	}
end

-- -- doing things --------------------------------------------------------

local function paths_of(args: any): { string }
	local list = args.paths
	assert(type(list) == "table" and #list > 0, "say which files")
	for _, p in list do
		assert(type(p) == "string" and p ~= "", "each file is a path")
	end
	return list
end

local function with_paths(cmd: { string }, paths: { string }): { string }
	local out = table.clone(cmd)
	table.insert(out, "--")
	for _, p in paths do
		table.insert(out, ":(top,literal)" .. p)
	end
	return out
end

local function stage(args: any): any
	local cwd = check_cwd(args.cwd)
	return exclusive(cwd, function(): any
		if args.all then
			git(cwd, { "add", "-A", "--", ":/" })
		else
			git(cwd, with_paths({ "add", "-A" }, paths_of(args)))
		end
		return true
	end)
end

local function unstage(args: any): any
	local cwd = check_cwd(args.cwd)
	return exclusive(cwd, function(): any
		local has_head = run(cwd, { "rev-parse", "-q", "--verify", "HEAD" }).status == 0
		if args.all then
			if has_head then
				git(cwd, { "reset", "-q" })
			else
				git(cwd, { "rm", "-r", "-q", "--cached", "--", ":/" })
			end
		elseif has_head then
			git(cwd, with_paths({ "restore", "--staged" }, paths_of(args)))
		else
			git(cwd, with_paths({ "rm", "-r", "-q", "--cached" }, paths_of(args)))
		end
		return true
	end)
end

-- Puts files back as the last commit has them: changes made to one it has,
-- and a new one gone. What the window confirmed first.
local function discard(args: any): any
	local cwd = check_cwd(args.cwd)
	local wanted: { [string]: boolean } = {}
	for _, p in paths_of(args) do
		wanted[p] = true
	end
	return exclusive(cwd, function(): any
		local head, files = status_now(cwd)
		local restore, remove, clean = {}, {}, {}
		for _, f in files do
			if not wanted[f.path] then
				continue
			end
			if f.x == "?" then
				table.insert(clean, f.path)
			elseif f.status == "renamed" and f.old then
				table.insert(restore, f.old)
				table.insert(remove, f.path)
			elseif f.x == "A" or not head.oid then
				table.insert(remove, f.path)
			else
				table.insert(restore, f.path)
			end
		end
		if #restore > 0 then
			git(cwd, with_paths({ "restore", "--source=HEAD", "--staged", "--worktree" }, restore))
		end
		if #remove > 0 then
			git(cwd, with_paths({ "rm", "-f", "-q", "-r" }, remove))
		end
		if #clean > 0 then
			git(cwd, with_paths({ "clean", "-f", "-q" }, clean))
		end
		return { restored = #restore, removed = #remove + #clean }
	end)
end

local function commit(args: any): any
	local cwd = check_cwd(args.cwd)
	local message = args.message
	if type(message) ~= "string" or trim(message) == "" then
		error("no_message: A commit needs a message", 0)
	end
	return exclusive(cwd, function(): any
		local cmd = { "commit", "-q", "--cleanup=strip", "-F", "-" }
		if args.amend then
			table.insert(cmd, "--amend")
		end
		local r = run(cwd, cmd, { stdin = message, timeout = 180 })
		if r.status ~= 0 then
			local words = said(r)
			if string.find(words, "nothing to commit", 1, true) or string.find(words, "nothing added to commit", 1, true) or string.find(words, "no changes added", 1, true) then
				error("nothing_staged: Nothing is staged to commit", 0)
			end
			if string.find(words, "Please tell me who you are", 1, true) or string.find(words, "user.email", 1, true) then
				error("no_identity: git doesn't know who you are on " .. zeron.device.name, 0)
			end
			error(classified(words), 0)
		end
		local sha = trim(git(cwd, { "rev-parse", "HEAD" }))
		return { sha = sha }
	end)
end

local function push(args: any): any
	local cwd = check_cwd(args.cwd)
	return exclusive(cwd, function(): any
		local branch = trim(run(cwd, { "symbolic-ref", "-q", "--short", "HEAD" }).stdout)
		if branch == "" then
			error("detached: There's no branch to push: HEAD is a commit of its own", 0)
		end
		local up = run(cwd, { "rev-parse", "-q", "--abbrev-ref", "--symbolic-full-name", "@{upstream}" })
		local cmd: { string }
		if up.status == 0 and trim(up.stdout) ~= "" then
			cmd = { "push", "--porcelain" }
		else
			local remote = string.match(run(cwd, { "remote" }).stdout, "^([^\n]+)")
			if not remote then
				error("no_remote: This repository has nowhere to push to", 0)
			end
			cmd = { "push", "--porcelain", "-u", remote, "HEAD" }
		end
		local r = run(cwd, cmd, { timeout = 240 })
		if r.status == -1 then
			error("timeout: The push took too long", 0)
		end
		if r.status ~= 0 then
			local words = said(r)
			-- (`--porcelain` says a ref's fate on stdout, the hints on stderr.)
			local all = r.stdout .. "\n" .. r.stderr
			if string.find(all, "[rejected]", 1, true) or string.find(all, "non-fast-forward", 1, true) or string.find(all, "fetch first", 1, true) or string.find(all, "Updates were rejected", 1, true) then
				error("rejected: " .. words, 0)
			end
			if string.find(words, "Authentication failed", 1, true) or string.find(words, "Permission denied", 1, true) or string.find(words, "could not read Username", 1, true) then
				error("auth: " .. words, 0)
			end
			if string.find(words, "Could not resolve host", 1, true) or string.find(words, "unable to access", 1, true) then
				error("network: " .. words, 0)
			end
			error(classified(words), 0)
		end
		return { branch = branch }
	end)
end

-- Pulls with a rebase (your commits on top of what arrived); a pull that
-- stops on conflicts is undone, so nothing is left half done.
local function pull(args: any): any
	local cwd = check_cwd(args.cwd)
	return exclusive(cwd, function(): any
		local r = run(cwd, { "pull", "--rebase", "--autostash", "--no-edit" }, { timeout = 240 })
		if r.status ~= 0 then
			local words = said(r)
			local states = exists(cwd, { "REBASE_HEAD" })
			if states.REBASE_HEAD or string.find(words, "CONFLICT", 1, true) then
				run(cwd, { "rebase", "--abort" })
				error("pull_conflicts: " .. words, 0)
			end
			if string.find(words, "no tracking information", 1, true) then
				error("no_upstream: This branch isn't on a remote yet", 0)
			end
			error(classified(words), 0)
		end
		return true
	end)
end

local function fetch(args: any): any
	local cwd = check_cwd(args.cwd)
	local r = run(cwd, { "fetch", "--prune", "--quiet" }, { timeout = 240 })
	if r.status ~= 0 then
		error(classified(said(r)), 0)
	end
	local k = known[cwd]
	if k then
		k.base_at = nil
	end
	return true
end

local function switch(args: any): any
	local cwd = check_cwd(args.cwd)
	local name = args.branch
	if type(name) ~= "string" or trim(name) == "" then
		error("bad_name: Name the branch", 0)
	end
	name = trim(name)
	local ok = run(cwd, { "check-ref-format", "--branch", name })
	if ok.status ~= 0 then
		error(`bad_name: "{name}" can't be a branch's name`, 0)
	end
	-- (`start`: where a new branch starts, a commit; HEAD without one.)
	local start = if type(args.start) == "string" and string.match(args.start, "^%x+$") then args.start else nil
	return exclusive(cwd, function(): any
		local cmd = if args.create then { "switch", "-c", name } else { "switch", name }
		if args.create and start then
			table.insert(cmd, start)
		end
		local r = run(cwd, cmd, { timeout = 60 })
		if r.status ~= 0 then
			local words = said(r)
			if string.find(words, "would be overwritten", 1, true) then
				error("dirty: " .. words, 0)
			end
			if string.find(words, "already exists", 1, true) then
				error(`exists: There's a branch {name} already`, 0)
			end
			error(classified(words), 0)
		end
		local k = known[cwd]
		if k then
			k.base_at = nil
		end
		return { branch = name }
	end)
end

-- A chat's turn started here (its window saw it go to work): where the
-- work tree stands now, for Last turn. A second word within a few seconds
-- for the same chat (another window, the same turn) keeps the first.
local function turn_start(args: any): any
	local cwd = check_cwd(args.cwd)
	local chat = args.chat
	assert(type(chat) == "string" and chat ~= "", "turn_start needs the chat")
	local now = os.time()
	local kept = turns:get() or {}
	local was = kept[chat]
	if was and was.cwd == cwd and now - (was.at or 0) < 15 then
		return was
	end
	local tree = exclusive(cwd, function(): string
		local head = run(cwd, { "rev-parse", "-q", "--verify", "HEAD" })
		return snapshot(cwd, if head.status == 0 then trim(head.stdout) else nil)
	end)
	kept = table.clone(turns:get() or {})
	kept[chat] = { cwd = cwd, tree = tree, at = now }
	-- (The newest 200.)
	local ids = {}
	for id in kept do
		table.insert(ids, id)
	end
	if #ids > 200 then
		table.sort(ids, function(a, b)
			return (kept[a].at or 0) > (kept[b].at or 0)
		end)
		for i = 201, #ids do
			kept[ids[i]] = nil
		end
	end
	turns:set(kept)
	return kept[chat]
end

-- What the branch adds over its base, in lines: the Branch scope's own
-- question (its base, the work tree as it stands), the files left out.
-- For the panel home's branch row. Nil totals: no base to measure from.
local function branch_totals(args: any): any
	local cwd = check_cwd(args.cwd)
	return exclusive(cwd, function(): any
		local head = run(cwd, { "rev-parse", "-q", "--verify", "HEAD" })
		local oid = if head.status == 0 then trim(head.stdout) else nil
		local base = base_of(cwd)
		if not base or not oid then
			return { base = base }
		end
		local mb = run(cwd, { "merge-base", base, "HEAD" })
		if mb.status ~= 0 then
			return { base = base }
		end
		local tree = snapshot(cwd, oid)
		local out = git(cwd, { "diff", "--numstat", "-z", "-M", "--no-ext-diff", trim(mb.stdout), tree }, { timeout = 60 })
		local adds, dels = 0, 0
		for a, d in string.gmatch(out, "(%d+)\t(%d+)\t") do
			adds += tonumber(a) :: number
			dels += tonumber(d) :: number
		end
		return { base = base, adds = adds, dels = dels }
	end)
end

-- Reading only, safe to run again; the rest change the checkout.
zeron.serve("status", function(args: any): any
	return status(check_cwd(args.cwd))
end, { safe = true })
zeron.serve("changes", changes, { safe = true })
zeron.serve("diff", diff, { safe = true })
zeron.serve("history", history, { safe = true })
zeron.serve("branches", branches, { safe = true })
zeron.serve("commit_info", commit_of, { safe = true })
zeron.serve("branch_totals", branch_totals, { safe = true })
zeron.serve("stage", stage)
zeron.serve("unstage", unstage)
zeron.serve("discard", discard)
zeron.serve("commit", commit)
zeron.serve("push", push)
zeron.serve("pull", pull)
zeron.serve("fetch", fetch)
zeron.serve("switch", switch)
zeron.serve("turn_start", turn_start)
