-- A long working chat as a real agent's reads (chat_fixture's `long_chat`):
-- hundreds of turns, each a short prompt and a reply of markdown (bold
-- leads, lists, a table) cut by its work (a thought, a group of commands),
-- far taller than the transcript's estimate for a row not yet measured.
-- What a reader scrolls up through from its end.

local function tool(id: string, verb: string, target: string): any
	return { id = id, kind = "exec", verb = verb, target = target }
end

local function thought(id: string): any
	return {
		id = `g:{id}`,
		kind = "tools",
		summary = "Thought process",
		tools = {
			{ id = id, kind = "thought", verb = "Thought process", call = "Weighing which of the three fits the timing.", prose = true },
		},
	}
end

local function work(id: string, n: number): any
	local tools = {}
	for k = 1, n do
		table.insert(tools, tool(`{id}.{k}`, "Ran", `aws logs start-query --log-group-name /engine --start-time {k}`))
	end
	return { id = `g:{id}`, kind = "tools", summary = `Thought {n} times · ran {n} commands`, tools = tools }
end

local STATE = [[No, the restart did not fix it, and I was wrong to expect that it would. The cause is not built-up state inside the engine.

**State at 20:09 UTC:**
- **Lag:** live updates are about 85 minutes late, and the gap still grows.
- **CPU:** one core is still at 100% (51% of the host), the same as before the restart.
- **Data:** none is lost. Users' own edits save normally, and libraries load. Only changes made by others reach open tabs late.

**The likely cause:** the engine now serves 113,118 registered workspaces, three times the 36,319 on Oct 30. At 18:40, during today's peak, it crossed a scaling limit: some work that grows with the number of workspaces now takes all of one core. The restart did not change that number, so the new engine fell behind within 30 seconds.

**What I am doing now:**
1. **A helper agent reads the engine code** for the work that grows with workspace count, and for any setting that relieves it without new code. The answer is due in about 15 minutes.
2. **A fallback you can decide on now:** lower the Orbit rollout percentage (`feature-orbit-library`, now 100%). New sessions then load the library from the server, and the number of registered workspaces falls as sessions end. Open tabs keep Orbit until they reload. It is slow relief, not a fix, and it is your decision.

I recommend waiting the 15 minutes for the code analysis first. If it shows no setting we can change, then lower the rollout to 50% to take load off the engine. I do not change the flag without your word.]]

local SUSPECTS = [[The helper agent finished its code reading. It did not prove a single cause, but it explains the freezes, and it found settings we could change.

**Why the whole engine freezes (read in the code, high confidence):**
- The engine has 2 worker threads, one per CPU core.
- Routing holds two locks while it runs SQLite work on a worker thread.
- When one locked step becomes slow, the other thread waits on the same lock, so nothing else runs. That stops network calls, the metrics endpoint and the status line, which matches what we see.

**Three suspects for the slow step**, all of which grow with the number of active workspaces:

| Suspect | What it is | Fits the sudden jump at 18:40? |
|---|---|---|
| **A. Sharing graph walk** | Each change that adds or removes a row walks the whole connected group of shared rows. | Yes: one large share (for example a big folder shared with many people) can create a huge group at once, and a restart keeps it, because it is stored data. |
| **B. Delivery queue rescan** | When the 64 MiB delivery memory is full, the engine rescans every waiting workspace on every wake. More retries make longer scans. | Yes: crossing that memory limit switches it on suddenly. |
| **C. Activation scans** | Scans over all pending activations. | Less likely. |

I cannot tell A, B and C apart from here. The engine's own gauges would answer it in minutes, but only the monitor can read them, and I do not have access to its read token, which is a production secret.

**Options, all of which need your decision:**
1. **Raise the delivery memory** (`DELIVERY_RESERVE_BYTES` from 64 MiB to 256 MiB, and `MAX_ASSEMBLED_BYTES` from 512 MiB to 1 GiB). The host has 7 GiB for the engine, and the AWS README says up to 2 GiB is safe. It is an engine config change plus a stop-first restart (about a 30-second pause). It fixes suspect B. If A is the cause, it does not help, but it does no harm.
2. **Lower the Orbit rollout** from 100% to 50%. Fewer active workspaces reduce all three suspects, but slowly, as sessions end. Open tabs keep Orbit until they reload.
3. **Let me read the engine's gauges first.** I would start a one-time diagnostic task in AWS that can reach the engine's metrics port, read the three gauges and stop. It is a small new resource on the production cluster, so it needs your yes. It takes about 10 minutes and tells us exactly which fix is right.

**My recommendation:** do 3 and 1 together. The diagnostic task tells us whether 1 is enough, and 1 is low-risk anyway. If the gauges point to suspect A, we lower the rollout (2) while I find the large shared group in the data.

Reply with the numbers you approve, for example "1 and 3".]]

local APPROVED = [[You approved 1 (more delivery memory) and 3 (the diagnostic task). I do not change the rollout.

I start with the diagnostic task, because it is fast and tells us whether 1 is the right fix. The task uses the monitor's own task definition with a one-time command: it reads the engine's metrics port a few times, prints only the gauge lines we need, and exits.]]

local RUNNING = [[The diagnostic task `53f0a6da` is running. While it reads the gauges, I prepare option 1. First I check how the engine divides its memory budget, so the new values pass its startup checks.]]

local TRIGGER = [[I found the trigger.

| Workspace | What happened | Matches? |
|---|---|---|
| **`_FzNZss0Oeor59MXNkizE`** (23,443 documents, 23,443 permission rows) | Imported **711 documents between 18:40 and 18:50 UTC** | Yes: same 10 minutes as the CPU jump |
| `qxo2kmpZusxCpDX3Jdyce` (5,709 documents) | Has imported about 400 documents per 10 minutes since 17:00 | No: the engine was fine with it until 18:40 |

**How the import stalls the engine:** in this engine version, each new document in a workspace makes the engine walk that workspace's whole connected group of rows (documents and permissions, about 47,000 rows here). Each walk takes tens of seconds and holds the engine's locks, so everything else waits. The agent read this in the code. The timing match is my inference, but it is a strong one.

**How bad it is:** the engine is still inside that 10-minute burst. In 85 minutes it advanced only 75 seconds of database time. At this rate it needs about 11 hours to get past the burst, and live updates for all workspaces stay late until then. No data is lost.

**Options:**
1. **Patch the engine (the real fix).** Limit or index that walk, built from `7377c85`, the current production version, as the incident rules require. With tests this takes a few hours, then a stop-first deploy. I can start now.
2. **Skip the backlog: reset the engine's database position to "now".** Every open workspace (about 113,000) would then refill from the database, throttled by the fill service, probably over 20–40 minutes. This restores freshness fastest. But we have **never tried this in demand mode at production scale**, and an untested mass action is exactly what caused the Oct 30 incident. I would rehearse it on dev first (about 30 minutes), and still present the exact steps to you before production.
3. **Wait:** about 11 hours, and longer if that workspace imports more. It imported 20 more documents at 20:10.

**My recommendation:** start 1 and the dev rehearsal of 2 now, in parallel. If the patch is not ready and tested by the time the rehearsal passes, I bring you option 2 with the exact steps. Neither touches production until you approve it.

Reply "go" to start both.]]

local PROMPTS = {
	"Go, this will fix it right?",
	"three and one sounds like the most optional option\n\nrolling back at this point is not a good idea. we've released a feature for three weeks almost",
	"ok",
	"What does the monitor say now?",
}

local function reply(id: string, i: number, parts: { any }): any
	local text = {}
	for _, b in parts do
		if b.kind == "text" then
			table.insert(text, b.text)
		end
	end
	local all = table.concat(text, "\n\n")
	return { id = id, role = "assistant", text = all, copy = all, model = "Opus", at = `Nov 10, {i % 12 + 1}:{10 + i % 50} PM`, blocks = parts }
end

local function trigger(): { any }
	return {
		thought("tt"),
		{ id = "a", kind = "text", text = APPROVED },
		work("w1", 4),
		{ id = "b", kind = "text", text = RUNNING },
		work("w2", 14),
		{ id = "c", kind = "text", text = TRIGGER },
	}
end

-- `turns` turns of history, then the three the reader scrolls up through:
-- the suspects (a reply), the person's two-paragraph answer, the trigger (a
-- reply cut by its work), the newest at the end. `sent`: up to the suspects
-- alone, the answer to be sent from the composer (`ANSWER`) and the trigger
-- to stream in (`stages`).
-- A message another chat sent, long enough to fold behind Show more.
local LANE = "Render graph lane: received. On GO: land f0eeab7f0 first, then tell the perf lane the SHA. Plan, in order: (a) resolve in Final's input: read the MSAA colour per sample (texture_multisampled), weight each sample by 1/(1+luma) before averaging and invert after; a test on a high-contrast edge vs today's LDR path and vs a 16x supersampled reference; I'll send you the numbers. (b) then the bloom chain at half resolution, its mips kept between frames, and a test that a still scene costs nothing after the first frame. (c) last, the tone map as its own pass, so the perf lane can time each step apart."

local function chat(turns: number, sent: boolean?, cross: boolean?): { any }
	local out: { any } = {}
	local bodies = { STATE, SUSPECTS, TRIGGER }
	for i = 1, turns do
		table.insert(out, { id = `lu{i}`, role = "user", text = PROMPTS[i % #PROMPTS + 1], at = "Nov 10, 1:00 PM", copy = PROMPTS[i % #PROMPTS + 1] })
		table.insert(out, reply(`la{i}`, i, {
			thought(`t{i}`),
			{ id = "a", kind = "text", text = bodies[i % #bodies + 1] },
			work(`w{i}`, 1 + i % 4),
		}))
		if cross then
			-- Another chat's word, then a short reply of work and a line.
			table.insert(out, { id = `lx{i}`, role = "user", text = LANE, at = "Nov 10, 3:30 PM", copy = LANE, sender = { chat = "lane", title = "zgx 3D: render graph and post chain (R0)" } })
			table.insert(out, reply(`ly{i}`, i, {
				work(`wy{i}`, 3),
				{ id = "a", kind = "text", text = "Ten lanes are building up unlanded local work in the meantime." },
			}))
		end
	end
	table.insert(out, { id = "orbit-go", role = "user", text = PROMPTS[1], at = "Nov 10, 12:56 PM", copy = PROMPTS[1] })
	table.insert(out, reply("orbit-suspects", 0, {
		work("ws", 9),
		thought("ts"),
		{ id = "a", kind = "text", text = SUSPECTS },
	}))
	if sent then
		return out
	end
	table.insert(out, { id = "orbit-answer", role = "user", text = PROMPTS[2], at = "Nov 10, 1:13 PM", copy = PROMPTS[2] })
	table.insert(out, reply("orbit-trigger", 1, trigger()))
	return out
end

-- The trigger reply as a host streams it: its blocks so far at each step,
-- the text block being written last and given in part (`part_text`).
type Stage = { blocks: { any }, part: string?, part_text: string? }
local function stages(): { Stage }
	local all = trigger()
	local out: { Stage } = {}
	for n = 1, #all do
		local b: any = all[n]
		local done = table.move(all, 1, n - 1, 1, {})
		if b.kind == "text" then
			local text = b.text :: string
			local step = 40
			local at = step
			while true do
				local cut = string.sub(text, 1, math.min(at, #text))
				local blocks = table.clone(done)
				table.insert(blocks, { id = b.id, kind = "text", text = "" })
				local stage: Stage = { blocks = blocks, part = b.id, part_text = cut }
				table.insert(out, stage)
				if at >= #text then
					break
				end
				at += step
			end
		else
			local blocks = table.clone(done)
			table.insert(blocks, b)
			local stage: Stage = { blocks = blocks }
			table.insert(out, stage)
		end
	end
	return out
end

return {
	chat = chat,
	trigger = trigger,
	stages = stages,
	answer = PROMPTS[2],
}
