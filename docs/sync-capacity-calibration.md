# Sync capacity calibration after the scheduler audit

Status: the working branch now applies **28 clients per profile and 32 process-wide chat sockets**, retaining four dials, eight HTTP requests and 50 ms dial spacing. The measurements below predate active-writer rotation and recommend a workload of **20 independent main agent chats plus eight additional viewed chats**. They do not measure 20 main agents plus arbitrary subagent trees.

The target is for the whole desktop process, not 20 agents per profile. The client cap is not an agent-launch limiter: exceeding the recommended agent count can still exhaust resources before sync admission can protect the process.

## Method

Measured locally on an Apple M4 Pro, 14 logical CPUs and 48 GiB RAM, using a debug build. Only temporary profiles and loopback servers were used. The benchmark lowered its own soft file-descriptor limit to 256 and padded its initial descriptor use to 48. It did not change the desktop process or the OS limit.

The fixture assembles EngineCore, creates chats through RPC, and dispatches actual SessionsEngine runs. Each synthetic harness has a child process with three pipes and its own WebSocket connection using the real MCP Zeron client. Text crosses the session journal, event folding, the 120 ms document coalescer, the durable outbox and ChatClient. Eight additional transcript subscriptions remain open. This exercises the real engine path with synthetic agents; it does not run paid model providers, render the desktop UI or reproduce every external tool workload.

Each chat starts with 1 MiB of transcript text (approximately 2.14 MiB serialized checkpoint); each active agent has a 1 MiB historical journal. The relay runs in another process, imposes 20 ms response delays and sends checkpoint bodies in 16 KiB chunks with 2 ms spacing. Each round emits approximately 1 KiB per agent; every fifth round also reads transcripts through MCP, renews wakes and launches Git. Measurements verify that all round markers reach SessionDoc and that the outbox drains. All chat sockets are then dropped together to measure recovery. A separate 128-job cold backlog checks document materialization at saturation.

The short matrix is closed-loop: rounds target a 200 ms interval but wait for the cohort to reach the document and drain its outbox. Its latency includes MCP, journal, coalescing, observation and acknowledgement time; it is not a pure network RTT or a promise of 5 Hz at every tested size. A separate sustained open-loop run sends every 200 ms independently of confirmations. Descriptor peaks are sampled and can miss very short transients; observed EMFILE spawn failures remain decisive even if the sampled peak is below 256.

## Capacity matrix

Every row includes eight additional viewed chats. Successful rows had no steady reconnects, no queued active documents, no missing generated markers, no acknowledgement timeouts, no Git failures and complete recovery after the deliberate disconnect.

| Agent chats | Runs | Sampled peak descriptors | Cohort completion p95 | Assessment |
| --- | --- | --- | --- | --- |
| 16 | 1 | 171 | 404 ms | Ample margin |
| 20 | 3 | 190–194 | 435–450 ms | Recommended target |
| 22 | 1 | 214 | 491 ms | Less margin; not the default recommendation |
| 24 | 3 | 216–223 | 491–523 ms | Works in this fixture, only 33 descriptors free at the worst sampled peak |
| 28 | 1 | 244 | 579 ms | Only 12 descriptors free; unsuitable as a safe default |
| 32 | 1 completed overload run | 253 | Not meaningful | 29/32 agents started; EMFILE errors |
| 40 | 1 | 256 | Not meaningful | 28/40 agents started; EMFILE errors |
| 48 | 1 | 256 | Not meaningful | 31/48 agents started; EMFILE errors |

The benchmark deliberately records overload instead of failing its test executable. An exit code of zero is not a capacity pass; startup shortfall, timeouts and recovery misses determine the outcome. Missing expected markers in overloaded cases came from agents that never started, rather than evidence that accepted document edits were discarded. The earlier incomplete 32-agent attempt and the initial fixture-debugging pilots are excluded from this table.

## Additional checks at the recommendation

- Sustained open-loop run: 20 agents emitted 300 rounds each at 200 ms intervals, independently of confirmations (6,000 text events over 60 seconds), with 1,200 MCP transcript reads during streaming and 60 Git probes. Peak 197 descriptors, cohort completion p95 666 ms, MCP p95 179 ms, Git p95 14 ms and reconnect p95 1.24 s. Zero queued active documents, steady reconnects, missing markers, acknowledgement timeouts or Git failures. Peak engine RSS was approximately 2.55 GiB; this is a debug/headless measurement with observation overhead, not a full desktop or real-provider memory estimate.
- Two profiles, 20 agents total and eight views, using the proposed 28-per-profile / 32-global limits: peak 198 descriptors, cohort p95 436 ms, no missing agents or markers, no queued active documents, all sockets released at shutdown. Cold jobs can materialize in spare per-profile slots; the global socket budget still remains bounded.
- Four times larger transcript histories and journals: peak 192 descriptors, approximately 1.85 GiB engine RSS, cohort p95 1.71 s, cold-start p95 11.5 s and reconnect p95 1.25 s. No missing generated markers, acknowledgement timeouts or Git failures. Larger histories affect latency and memory even when the descriptor budget is healthy.
- At a fully protected single-profile cap, adding 128 cold jobs caused zero additional document loads. The jobs intentionally stayed durable until capacity became available; this does not claim that a fully occupied host can drain unlimited background work.
- MCP reads released their quiet subscriptions: after each run only the eight intentional views remained. Shutdown released every chat socket permit.

## Interpretation

The earlier headless DocHost estimate omitted real session journals and per-agent MCP connections. It should not justify 32 active agents under a 256-descriptor limit. The full-engine measurements support 20 as a prudent starting target, retaining about 58 descriptors even in the tested two-profile case. Twenty-four is an aggressive alternative that needs a larger resource allowance or additional measurements with the user's actual tools before being treated as a general operating target.

The journal cache currently retains at most 16 files and clears the cache when admitting another one. Reopening rescans historical journal content. That pre-existing behavior was included in these measurements and can increase CPU/latency above 16 sessions; this change does not alter journal persistence. A future optimization should preserve sequence/torn-write recovery without repeatedly reading the entire history, rather than simply retaining arbitrarily many open files.

These measurements do not guarantee zero queuing under arbitrary loads. The current scheduler uses explicit user focus for admission at capacity and lets active writers yield their sync transport while their local execution and durable publication continue. Overdue documents receive service turns, including live subagent documents. See [the rotation policy](sync-resource-resilience.md#focus-and-service-turns).

A live subagent transcript requests its own client even when its panel is closed. The capacity matrix did not spawn subagent trees; a child does not necessarily have the same process, pipe and journal cost as a main agent. Rotation tests with 48 parent/child writer documents verify delivery and bounded connections, not the capacity to run 48 model processes. Raising sync caps does not create descriptors for Git, pipes, journals or MCP. Calibrating a real parent/subagent workload remains separate from these correctness tests.

## Baseline validation before rotation

Before the rotation change, restoring the original 12/24/4/8 limits gave 423 passing selected tests with the normal parallel test runner: 275 engine unit tests (including 15 sync lifecycle cases), 55 engine integration tests, 65 sync tests, 15 RPC tests and 13 MCP tests. Five tests requiring external/private fixtures remained ignored. The engine integration set covers chat2 creation races, device routing, local profiles, queues, restart/resume, publication, transcript salvage and the 1,000-job regression under RLIMIT_NOFILE=256. `cargo check --locked -p zeron-ui`, formatting checks on changed Rust files and `git diff --check` also passed. These are historical results for the measured baseline, not measurements of the new rotation policy.
