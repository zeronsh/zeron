--!strict
-- Why something failed, in plain words: a refusal of Zeron's (the device
-- offline, the plugin not allowed there, another version) or the service's
-- own code (the push rejected, a change in the way), as a title and a
-- line. Where nothing better fits, git's own words are the line.

return function(zeron: any)
	-- The device a session's checkout is on, by name when it's known.
	local function device_name(status: any?): string
		local name = status and status.device
		return if type(name) == "string" and name ~= "" then name else zeron.s("device.unknown")
	end

	-- `{ title, body }` for `p` (repo.luau's Problem), the checkout's last
	-- status giving the device's name and the branch.
	return function(p: any, status: any?): { title: string, body: string, retry: boolean }
		local device = device_name(status)
		local args = { device = device, branch = (status and status.branch) or "", remote = (status and status.remote) or "origin", words = p.words or "" }
		local reason = p.reason
		if reason ~= "failed" then
			-- Zeron's refusals: the device, the plugin there.
			local known = {
				offline = true,
				timeout = true,
				not_installed = true,
				off = true,
				not_approved = true,
				version_mismatch = true,
				other_author = true,
				no_service = true,
				not_loaded = true,
				old_engine = true,
				interrupted = true,
			}
			if known[reason] then
				return {
					title = zeron.s(`reason.{reason}.title`, args),
					body = zeron.s(`reason.{reason}.body`, args),
					retry = reason == "offline" or reason == "timeout" or reason == "interrupted",
				}
			end
			return { title = zeron.s("reason.other.title"), body = p.words, retry = true }
		end
		local code = p.code
		local codes = {
			not_a_repo = false,
			no_checkout = false,
			rejected = true,
			auth = true,
			network = true,
			nothing_staged = false,
			no_identity = false,
			dirty = false,
			pull_conflicts = false,
			no_base = false,
			no_turn = false,
			detached = false,
			no_remote = false,
			no_upstream = false,
			bad_name = false,
			exists = false,
			timeout = true,
			no_message = false,
			no_ref = false,
			unsafe_dir = false,
			locked = true,
		}
		if code and codes[code] ~= nil then
			return {
				title = zeron.s(`code.{code}.title`, args),
				body = zeron.s(`code.{code}.body`, args),
				retry = codes[code] :: boolean,
			}
		end
		-- git's own words, its first line (the service cleans it), and what
		-- can be done; never the bare "git couldn't do it".
		local said = zeron.s("code.git.title", args)
		if type(p.words) == "string" then
			-- (Its first line that says something, without "fatal:", a
			-- capital to start: as the service cleans it, for words that
			-- came another way.)
			for line in string.gmatch(p.words .. "\n", "([^\n]*)\n") do
				local l = string.gsub(string.gsub(line, "^%s+", ""), "%s+$", "")
				if l ~= "" and not string.match(l, "^hint:") then
					l = string.gsub(string.gsub(string.gsub(l, "^fatal:%s*", ""), "^error:%s*", ""), "^warning:%s*", "")
					said = (string.gsub(l, "^%l", string.upper))
					break
				end
			end
		end
		return { title = said, body = zeron.s("code.git.body", args), retry = true }
	end
end
