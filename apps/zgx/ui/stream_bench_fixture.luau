-- The stream bench's page (src/stream_bench.rs, feature `stream-bench`): the
-- chat page as the main window shows a session, its transcript and docked
-- composer, with nothing stubbed. Everything it shows is the host's chat
-- state (`chat.session`, `chat.messages`, `chat.draft`: src/chat.rs), for
-- the chat `stream-bench`, which the host fills from a recorded stream of
-- the engine's frames in place of the engine.
--
--   ZERON_STREAM_BENCH=<frames.jsonl> zeron-zgx

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local chat = require("@app/screens/chat")


-- The shell, as small as can hold the page (ui/chat_fixture.luau's): the
-- session's page, and the composer over it.
return ui.overlay {
	w_full = true,
	h_full = true,
	ui.column {
		w_full = true,
		h_full = true,
		pt = ui.raw(38, "the title bar's height"),
		chat.Session("stream-bench"),
	},
	chat.Composer(),
}
