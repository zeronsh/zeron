-- The world control_qa_fixture and control_qa_screen_fixture draw (see
-- control_qa_fixture's header for its entries): control_world, plus two
-- earlier dumps whose questions went to threads, the thread one was
-- answered in, and the Q&A marks as they start. Never the page itself.
--
--   require("@app/control_qa_world")()   once, before the screen is built

return function()
	local orchestration = require("@account/orchestration")
	local sessions = require("@account/sessions")
	local QA = require("@ds/orch_qa")

	local NOW = os.time()
	local function ago(s: number): number
		return NOW - s
	end

	local CAPY_ANSWER = [[In today's flow, essentially nothing reaches Inbox or Next. I checked the code.

**What can put a card there:**
- **Inbox:**
  - the Brain dump routing a fragment straight to a board card instead of a thread;
  - any card created without a state, such as an orchestrator adding an item with no state set.
- **Next:**
  - an orchestrator planning an item before starting its crew;
  - "Add to board" on a chat that isn't working.

**Why you've never seen one:** you have no orchestrators. The Brain dump's "act, don't shrug" rule sends every task to a new thread that starts immediately, and those cards are created straight in Now.

My suggestion: make the columns meaningful rather than remove them.
- **Inbox: captured, not started yet.** Fragments like "later", "someday" or "remind me" become Inbox cards instead of new threads.
- **Next: queued work.** Tasks you say should wait, or ones held back because a device is at its limit.

The simpler alternative is to hide Inbox and Next until they have cards. Which do you want?]]

	local REAL = "Are Inbox and Next sections in the board ever used? Under what conditions does that happen? Because I've never seen a card in Inbox or Next."
	local FLICKER = "Why does the map flicker on resize? Find the root cause and fix it."
	local SHIP = "When does 2.0 ship?"
	local RIGHT = "The new map is so much better, right?"
	local dp_qa_text = `{REAL} {FLICKER} {SHIP} {RIGHT}`
	local function span_of(text: string, part: string): { from: number, to: number }
		local at = string.find(text, part, 1, true) :: number
		return { from = at - 1, to = at - 1 + #part }
	end

	local MSI = "Is the Windows MSI signed yet?"
	local BUCKET = "what did the CI cache bucket end up being?"
	local dp_qa2_text = `{MSI} And {BUCKET}`

	local extra = {
		{
			id = "dp-qa2",
			text = dp_qa2_text,
			state = "routed",
			summary = "2 questions: Exnomic VPN, CI speed.",
			device = "dev-desk",
			created = ago(3 * 3600),
			updated = ago(3 * 3600 - 600),
			routes = {
				{ id = "rq-5", excerpt = MSI, span = span_of(dp_qa2_text, MSI), target = { kind = "orchestrator", id = "orc-vpn" },
					intent = "question", direct = true, message = "Is the Windows MSI signed yet?", state = "delivered", candidates = {} },
				{ id = "rq-6", excerpt = BUCKET, span = span_of(dp_qa2_text, BUCKET), target = { kind = "orchestrator", id = "orc-ci" },
					intent = "question", direct = true, message = "Which bucket did CI caching settle on?", state = "answered", candidates = {},
					answer = "zeron-ci-cache-us.", answer_full = "`zeron-ci-cache-us`, in us-east-2.", answered = ago(3 * 3600 - 600) },
			},
		},
		{
			id = "dp-qa",
			text = dp_qa_text,
			state = "routed",
			summary = "4 parts: 2 questions, 1 task, 1 note.",
			device = "dev-desk",
			created = ago(8 * 3600),
			updated = ago(8 * 3600 - 300),
			routes = {
				{ id = "rq-1", excerpt = REAL, span = span_of(dp_qa_text, REAL), target = { kind = "chat", id = "th-capy" },
					intent = "question", direct = true, message = "Are the Board's Inbox and Next columns ever used?", state = "answered", candidates = {},
					answer = "Essentially never in today's flow: every task starts a thread at once, so cards land in Now.",
					answer_full = CAPY_ANSWER, answered = ago(8 * 3600 - 240) },
				{ id = "rq-2", excerpt = FLICKER, span = span_of(dp_qa_text, FLICKER), target = { kind = "chat", id = "th-ui-control" },
					intent = "task", direct = false, message = "Find why the map flickers on resize and fix it.", state = "delivered", candidates = {} },
				{ id = "rq-3", excerpt = SHIP, span = span_of(dp_qa_text, SHIP), target = { kind = "orchestrator", id = "orc-orch" },
					intent = "question", direct = true, message = "When does 2.0 ship?", state = "answered", candidates = {},
					answer = "Friday, after the board lands.", answer_full = "**Friday**, once the board work lands.", answered = ago(7 * 3600) },
				{ id = "rq-4", excerpt = RIGHT, span = span_of(dp_qa_text, RIGHT), target = { kind = "note" },
					intent = "info", direct = false, message = "", state = "done", candidates = {} },
			},
		},
	}

	-- In time order with the sample's (newest first).
	local now = table.clone(orchestration.now:peek())
	local dumps = table.clone(now.dumps or {})
	for _, d in extra do
		table.insert(dumps, d)
	end
	table.sort(dumps, function(a: any, b: any): boolean
		return (a.created or 0) > (b.created or 0)
	end)
	now.dumps = dumps
	orchestration.now:set(now)

	-- The thread the real question was answered in.
	local s = table.clone(sessions.now:peek())
	local chats = table.clone(s.chats or {})
	table.insert(chats, { id = "th-capy", title = "Explaining Capy.ai Captains", device = "dev-desk", status = "idle",
		updated = ago(8 * 3600 - 240), created = ago(9 * 3600), agent = "claude", model = "claude-opus-5-5" })
	s.chats = chats
	-- At work: the engine state thread on a turn begun just now (after the
	-- question to it came); the VPN orchestrator on one begun before its
	-- question came, so not at it yet.
	local function at(list: { any }): { any }
		local out = {}
		for _, c in list do
			c = table.clone(c)
			if c.id == "th-engine-state" then
				c.status, c.turn_started = "working", ago(10)
			elseif c.id == "orc-vpn" then
				c.status, c.turn_started = "working", ago(4 * 3600)
			end
			table.insert(out, c)
		end
		return out
	end
	s.chats = at(s.chats)
	local kids = {}
	for parent, list in s.children or {} do
		kids[parent] = at(list)
	end
	s.children = kids
	sessions.now:set(s)

	-- Read on this device: one; put away: one.
	QA.marks:set({ read = { ["dp-qa/rq-3"] = true }, dismissed = { ["dp-qa2/rq-6"] = true } })
	-- Opened from the test (`fixture.qa_reset`): as it started.
	ui.command { id = "fixture.qa_reset", title = "Fixture: Q&A marks as they started", run = function()
		QA.marks:set({ read = { ["dp-qa/rq-3"] = true }, dismissed = { ["dp-qa2/rq-6"] = true } })
	end }
	-- A thread starts working that wasn't there before (`fixture.new_worker`).
	ui.command { id = "fixture.new_worker", title = "Fixture: a new thread starts working", run = function()
		local n = table.clone(sessions.now:peek())
		local list = table.clone(n.chats or {})
		table.insert(list, { id = "th-new-worker", title = "Brand new worker", device = "dev-desk", status = "working",
			updated = os.time(), created = os.time(), agent = "claude", model = "claude-opus-5-5" })
		n.chats = list
		sessions.now:set(n)
	end }
	-- The Brain dump routes the newest dump (a follow-up asked from an
	-- entry's overlay) as a question to the thread that answered it, as the
	-- engine aims a dump referencing a Q&A entry (`fixture.qa_route_follow_up`).
	ui.command { id = "fixture.qa_route_follow_up", title = "Fixture: route the follow-up", run = function()
		local n = table.clone(orchestration.now:peek())
		local dumps = table.clone(n.dumps)
		local d = table.clone(dumps[1])
		d.routes = {
			{ id = "rq-follow", excerpt = d.text, span = { from = 0, to = #d.text }, target = { kind = "chat", id = "th-capy" },
				intent = "question", direct = true, message = d.text, state = "delivered", candidates = {}, delivered = os.time() },
		}
		d.state = "routed"
		dumps[1] = d
		n.dumps = dumps
		orchestration.now:set(n)
	end }
	-- A thread answers (`fixture.qa_answer`): the waiting MSI question.
	ui.command { id = "fixture.qa_answer", title = "Fixture: answer the MSI question", run = function()
		local n = table.clone(orchestration.now:peek())
		local ds2 = {}
		for _, d in n.dumps do
			if d.id == "dp-qa2" then
				local d2 = table.clone(d)
				local rs = {}
				for _, r in d.routes do
					if r.id == "rq-5" then
						local r2 = table.clone(r)
						r2.state = "answered"
						r2.answer = "Signed this morning."
						r2.answer_full = "Yes: signed this morning with the **EV cert**; the MSI on the release page is the signed one."
						r2.answered = os.time()
						table.insert(rs, r2)
					else
						table.insert(rs, r)
					end
				end
				d2.routes = rs
				table.insert(ds2, d2)
			else
				table.insert(ds2, d)
			end
		end
		n.dumps = ds2
		orchestration.now:set(n)
	end }
end
