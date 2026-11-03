--!strict
-- Notifications for your sessions' pull requests ("notifications"), a click
-- opening the session:
--   - a review requested of you;
--   - changes requested;
--   - its checks failing;
--   - merged, or closed without merging.
-- Every session's pull request is followed (watch.luau asks GitHub each
-- minute), not only the rows showing.
--
-- What each was is kept (`zeron.store`), and a pull request first seen (at
-- launch, a new session, a reload) is only noted: nothing old rings.

type Seen = { number: number, state: string, review: string?, asked: boolean?, ci: string? }

-- What changed, worth saying, between `was` and `now` (the same pull
-- request), or nil.
local function news(was: Seen, now: Seen): string?
	if now.state ~= was.state then
		if now.state == "merged" then
			return "merged"
		elseif now.state == "closed" then
			return "closed"
		end
		return nil
	end
	if now.state ~= "open" then
		return nil
	end
	if now.asked == true and was.asked ~= true then
		return "review_wanted"
	end
	if now.review == "changesRequested" and was.review ~= "changesRequested" then
		return "changes"
	end
	if now.ci == "failed" and was.ci ~= "failed" then
		return "failing"
	end
	return nil
end

return function(zeron: any, watch: any)
	local seen = zeron.store("pr_seen", {} :: { [string]: Seen })
	local watching: { [string]: () -> any } = {}

	-- Every session's pull request, watched as sessions come, and each one
	-- read (one effect: it follows both the sessions and what's heard).
	ui.effect(function()
		for id in zeron.sessions() do
			if not watching[id] then
				watching[id] = watch.pr(id)
			end
		end
		local before = seen:peek() or {}
		local after = table.clone(before)
		local changed = false
		for id, read in watching do
			local pr = read()
			if pr then
				local now: Seen = { number = pr.number, state = pr.state or "open", review = pr.review, asked = pr.requested_of_you, ci = pr.ci }
				local was = before[id]
				local same = was
					and was.number == now.number
					and was.state == now.state
					and was.review == now.review
					and was.asked == now.asked
					and was.ci == now.ci
				if not same then
					after[id] = now
					changed = true
					-- (First seen, or another pull request: noted, never said.)
					local said = if was and was.number == now.number then news(was, now) else nil
					if said then
						local session = zeron.context_of("session", id)
						zeron.notify({
							title = zeron.s("notices.title", { number = now.number, said = zeron.s(`notices.{said}`) }),
							body = session.title or pr.title or "",
							session = id,
						})
					end
				end
			end
		end
		if changed then
			seen:set(after)
		end
	end)
end
