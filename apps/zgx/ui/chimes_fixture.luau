-- The session chimes (screens/settings/chimes) with no engine: a test sets
-- the `sessions` shared state as the engine would publish it, and reads what
-- played from `settings_stub_calls` ("sound.play:done", …).
require("./settings_stub")
require("@app/screens/settings/chimes")

return ui.column {}
