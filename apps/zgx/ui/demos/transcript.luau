-- The gallery's Transcript page: a conversation of three hundred turns,
-- virtualized, and a reply to watch streaming in. "Send" adds a prompt and
-- writes its reply: it thinks, its tools arrive one by one, then its text
-- grows a few words at a time as incremental markdown, at the speed picked.
-- "Stop" ends it where it is, as interrupting an agent does.
--
-- The composer under it is the real one. Select text anywhere in the
-- conversation: Mention quotes it in the draft, Attach adds it to the strip.

local ds = require("@ds")
local T = require("@ds/transcript")
local Quote = require("@ds/quote")
local samples = require("@demos/transcript_samples")

type Message = T.Message

local TURNS = 300
-- Characters a second, each speed: slow enough to watch the markdown form,
-- a model's usual pace, and a fast one's.
local SPEEDS: { [string]: number } = { slow = 45, steady = 220, fast = 900 }
-- How often the reply's text is committed: often enough that it reads as a
-- flow, not lumps (Zeron's 120 ms read as lumps), rarely enough that each
-- commit carries a few words.
local TICK = 33

-- The conversation ends on a reply that shows pictures (`![alt](path)`):
-- two that load, a moment late, and one that can't be read.
local function with_pictures(list: { Message }): { Message }
	table.insert(list, { id = "pics-u", role = "user", text = "Show me the screens you captured, and the failed one." })
	table.insert(list, {
		id = "pics-a",
		role = "assistant",
		model = "Opus 5.5",
		text = "Here's the shell after the change:\n\n![The shell](assets/composer/shell.png)\n\nAnd the transcript beside it:\n\n![The transcript](assets/composer/transcript.png)\n\nThe third capture didn't save:\n\n![The settings page](captures/settings.png)",
	})
	return list
end
-- Then a reply whose work opens to what each tool did (ds/tool_detail):
-- an edit (its diff, and the whole of it when asked), a command's long
-- output (the whole of it when asked, a moment late), a command that
-- failed, and a read.
local LOG: { string } = {}
for i = 1, 40 do
	table.insert(LOG, `test transcript::row_{i} ... ok`)
end
local function with_tools(list: { Message }): { Message }
	table.insert(list, { id = "tools-u", role = "user", text = "Fix the fold toggle and run the tests." })
	table.insert(list, {
		id = "tools-a",
		role = "assistant",
		model = "Opus 5.5",
		text = "Done: the toggle now keys by id. The suite passes, and the lint run that failed was a stale cache.",
		worked = 48,
		tools = {
			{ id = "tools-t1", kind = "read", verb = "Read", target = "transcript.rs", output = "fn toggle_user_fold(&mut self, row_ix: usize) {\n    let id = self.rows[row_ix].id.clone();\n    self.user_folds.toggle(id);\n}" },
			{
				id = "tools-t2",
				kind = "edit",
				verb = "Edited",
				target = "transcript.rs",
				added = 2,
				removed = 1,
				diff_ref = "gallery/tools-t2.diff",
				diff = {
					kind = "diff",
					path = "crates/ui/src/transcript.rs",
					lines = {
						{ kind = "hunk", text = "@@ -6996,4 +6996,5 @@" },
						{ kind = " ", old = 6996, new = 6996, text = "        .on_click(cx.listener(move |this, _, _, cx| {" },
						{ kind = "-", old = 6997, text = "            this.toggle_user_fold(row_ix, collapsed_h, cx);" },
						{ kind = "+", new = 6997, text = "            let Some(ix) = this.row_index(&row_id) else { return };" },
						{ kind = "+", new = 6998, text = "            this.toggle_user_fold(row_id.clone(), ix, collapsed_h, cx);" },
						{ kind = " ", old = 6998, new = 6999, text = "        }))" },
					},
				},
			},
			{ id = "tools-t3", kind = "exec", verb = "Ran", target = "cargo test -p zeron-ui transcript", output = table.concat(LOG, "\n"), output_ref = "gallery/tools-t3", output_bytes = 18432 },
			{ id = "tools-t4", kind = "exec", verb = "Ran", target = "cargo clippy -p zeron-ui", failed = true, output = "error: could not compile `zeron-ui` (lib) due to 1 previous error\nerror[E0425]: cannot find value `row_ix` in this scope\n  --> crates/ui/src/transcript.rs:7001:31" },
		},
	})
	return list
end
local messages = ui.state("transcript_messages_v5", with_tools(with_pictures(samples.conversation(TURNS))) :: { Message })

-- The whole of a tool's output or diff, as the engine's sidecar gives it,
-- a moment late.
local ToolDetail = require("@ds/tool_detail")
ToolDetail.fetch = function(blob_ref: string): any
	return {
		after = function(_self: any, ok: (any) -> (), _err: any)
			ui.after(600, function()
				if blob_ref == "gallery/tools-t3" then
					local all = table.clone(LOG)
					for i = 41, 120 do
						table.insert(all, `test transcript::row_{i} ... ok`)
					end
					table.insert(all, "test result: ok. 120 passed; 0 failed")
					ok({ kind = "output", lines = all, more = 0 })
				else
					ok({
						kind = "diff",
						path = "crates/ui/src/transcript.rs",
						lines = {
							{ kind = "hunk", text = "@@ -6990,10 +6990,11 @@" },
							{ kind = " ", old = 6990, new = 6990, text = "    fn render_user_body(&mut self, row_id: SharedString) -> AnyElement {" },
							{ kind = " ", old = 6991, new = 6991, text = "        let collapsed_h = self.collapsed_height();" },
							{ kind = " ", old = 6996, new = 6996, text = "        .on_click(cx.listener(move |this, _, _, cx| {" },
							{ kind = "-", old = 6997, text = "            this.toggle_user_fold(row_ix, collapsed_h, cx);" },
							{ kind = "+", new = 6997, text = "            let Some(ix) = this.row_index(&row_id) else { return };" },
							{ kind = "+", new = 6998, text = "            this.toggle_user_fold(row_id.clone(), ix, collapsed_h, cx);" },
							{ kind = " ", old = 6998, new = 6999, text = "        }))" },
							{ kind = " ", old = 6999, new = 7000, text = "    }" },
						},
					})
				end
			end)
		end,
	}
end

-- The reply's pictures: as the app reads them, from a chat folder here (the
-- gallery's own files, a moment late), the missing one refused.
local ReplyImages = require("@app/screens/chat/reply_images")
local pictures = ReplyImages.for_session(function(): any
	return { id = "gallery", cwd = "/gallery" }
end, function(_target: any, path: string): any
	local rel = string.match(path, "^/gallery/(.*)$")
	return {
		after = function(_self: any, ok: (any) -> (), err: (any) -> ())
			ui.after(700, function()
				if rel and string.sub(rel, 1, 7) == "assets/" then
					ok({ src = rel })
				else
					err("No such file in the chat's folder")
				end
			end)
		end,
	}
end)
local offset = ui.state("transcript_offset_v2", math.huge)
local speed = ui.state("transcript_speed", "steady")
local writing = ui.state("transcript_writing", false)
local sent = ui.state("transcript_sent", 0)
local draft = ui.state("transcript_draft", { id = "", text = "" })

-- The composer: what's being written, what the message carries, each quote
-- mentioned (its passage, by its line in the draft), and a bump that gives it
-- the keys.
local typed = ui.state("transcript_typed", "")
local cargo = ui.state("transcript_cargo", {} :: { any })
local quotes = ui.state("transcript_quotes", {} :: { [string]: string })
local to_composer = ui.state("transcript_to_composer", 0)
local chips = 0
local access = ui.state("transcript_access", "full" :: string?)
local plan = ui.state("transcript_plan", false)
local model = ui.state("transcript_model", "opus" :: string?)
local effort = ui.state("transcript_effort", "high" :: string?)
local HARNESSES: { ds.Harness } = {
	{ id = "claude", label = "Claude", mark = "claude" },
}
local EFFORT = {
	{ value = "medium", label = "Medium" },
	{ value = "high", label = "High" },
}
local MODELS: { ds.Model } = {
	{ value = "opus", label = "Opus 5.5", harness = "claude", efforts = EFFORT, effort = "high" },
	{ value = "fable", label = "Fable 5.1", harness = "claude", efforts = EFFORT, effort = "high" },
}

-- The reply being written: its place in the list, its whole text, how much
-- shows, and the timers driving it.
local live = { index = 0, text = "", shown = 0, stops = {} :: { () -> () } }

local function halt()
	for _, stop in live.stops do
		stop()
	end
	live.stops = {}
end

-- Ends what is being written: the live reply, and any a reload left
-- writing with no timers behind it (they look back from the end).
local function finish()
	halt()
	local all = messages:peek()
	local now = ui.now() / 1000
	ui.batch(function()
		for i = #all, math.max(1, #all - 8), -1 do
			local m = all[i]
			if m.streaming then
				-- The text it got to, from the draft, into the list once.
				if draft:peek().id == m.id then
					messages:set_key({ i, "text" }, draft:peek().text)
				end
				messages:set_key({ i, "streaming" }, false)
				messages:set_key({ i, "worked" }, math.max(1, math.floor(now - (m.started or now))))
			end
		end
		writing:set(false)
	end)
end
-- A reload drops the timers: nothing is being written after one.
finish()

-- The text, a few words a tick: as many characters as the speed allows
-- since the last, then on to the end of the word it lands in.
local function stream()
	local budget = 0
	local last = ui.now()
	table.insert(
		live.stops,
		ui.every(TICK, function()
			-- By the clock, not the tick count: a late tick carries more.
			local now = ui.now()
			budget += SPEEDS[speed:peek()] * (now - last) / 1000
			last = now
			local want = live.shown + math.floor(budget)
			if want <= live.shown then
				return true
			end
			budget -= want - live.shown
			local stop = string.find(live.text, "[%s]", want) or #live.text
			live.shown = math.min(stop, #live.text)
			draft:set_key("text", string.sub(live.text, 1, live.shown))
			if live.shown >= #live.text then
				finish()
				return false
			end
			return true
		end)
	)
end

-- What was typed and carried, sent: `text` (the sample's own, from the Send
-- button) and `carried`, what rode with it.
local function send(text: string?, carried: { any }?)
	if writing:peek() then
		return
	end
	halt()
	sent:add(1)
	local prompt, reply, which = samples.live(sent:peek())
	local tag = `live{sent:peek()}`
	-- Sending lifts the prompt to the top of the view (from far up, it goes
	-- to the end), the reply to be written under it.
	draft:set { id = `{tag}a`, text = "" }
	T.send(function()
		local t = os.date("*t")
		messages:push { id = `{tag}u`, role = "user", text = text or prompt, attachments = carried or samples.ATTACH[which] }
		messages:push {
			id = `{tag}a`,
			role = "assistant",
			text = "",
			harness = "claude",
			model = "Opus 5.5",
			at = samples.clock(t.hour * 60 + t.min),
			tools = {},
			streaming = true,
			started = ui.now() / 1000,
		}
		writing:set(true)
	end)
	live.index = messages:len()
	live.text = reply
	live.shown = 0
	-- It thinks, then its tools arrive one by one, then it writes.
	local tools = samples.tools(which, tag)
	local n = 0
	table.insert(
		live.stops,
		ui.after(700, function()
			if #tools == 0 then
				stream()
				return
			end
			table.insert(
				live.stops,
				ui.every(420, function()
					n += 1
					messages:set_key({ live.index, "tools", n }, tools[n])
					if n >= #tools then
						table.insert(live.stops, ui.after(500, stream))
						return false
					end
					return true
				end)
			)
		end)
	)
end

return {
	id = "transcript",
	title = "Transcript",
	fill = true,
	about = "A long conversation, virtualized: what a person sent in bubbles that fold when long, what the agent did as a tree of tools, and replies in markdown, streaming in as they are written.",
	demo = function(): Node
		return ui.column {
			w_full = true,
			h_full = true,
			gap = "$space.4",
			ui.row {
				gap = "$space.3",
				items = "center",
				ui.show(
					writing,
					ds.Button { "Stop", tone = "secondary", size = "sm", on_click = finish },
					ds.Button { "Send a message", tone = "primary", size = "sm", on_click = function()
						send()
					end }
				),
				ds.Segmented {
					value = speed,
					label = "Speed",
					options = {
						{ value = "slow", label = "Slow" },
						{ value = "steady", label = "Steady" },
						{ value = "fast", label = "Fast" },
					},
				},
				ds.Text {
					function()
						return `{messages:len()} messages`
					end,
					style = "caption",
					tone = "tertiary",
				},
			},
			ui.column {
				w_full = true,
				-- The demo's frame: the window's height, less what's above it.
				grow = 1,
				min_h = 0,
				rounded = "$radius.xl",
				border = 1,
				border_color = "$border.subtle",
				overflow = "hidden",
				T.Transcript {
					messages = messages,
					offset = offset,
					draft = draft,
					pictures = pictures,
					on_mention = function(q)
						local line = Quote.line(q)
						local kept = table.clone(quotes:peek())
						kept[line] = q.text
						quotes:set(kept)
						typed:set(Quote.add(typed:peek(), line))
						to_composer:add(1)
					end,
					on_attach = function(q)
						chips += 1
						cargo:push(Quote.chip(q, `quote{chips}`))
						to_composer:add(1)
					end,
				},
			},
			ui.row {
				w_full = true,
				justify = "center",
				shrink = 0,
				ui.column {
					w_full = true,
					max_w = ui.theme.get("zeron.prose.measure") :: number,
					min_w = 0,
					ds.Composer {
						value = typed,
						placeholder = "Ask anything",
						running = writing,
						access = access,
						plan = plan,
						model = model,
						effort = effort,
						harnesses = HARNESSES,
						models = MODELS,
						menus_below = false,
						focus = to_composer,
						cargo = cargo,
						on_remove_cargo = function(id: string)
							local left = {}
							for _, item in cargo:peek() do
								if item.id ~= id then
									table.insert(left, item)
								end
							end
							cargo:set(left)
						end,
						on_stop = finish,
						on_send = function(text: string)
							local carried = {}
							for _, item in cargo:peek() do
								table.insert(carried, { kind = "file", name = item.name, meta = item.meta, icon = "cursor" })
							end
							local full = Quote.expand(text, Quote.kept(text, quotes:peek()))
							ui.batch(function()
								typed:set("")
								cargo:set({})
								quotes:set({})
							end)
							send(full, carried)
						end,
					},
				},
			},
		}
	end,
}
