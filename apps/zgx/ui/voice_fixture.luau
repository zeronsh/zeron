-- Settings → Voice alone, the host played by the test (tests/dictation.rs).
local ds = require("@ds")
ds.setup()
local prefs = require("@app/screens/settings/prefs")
prefs.dictation:set(false)
return require("@app/screens/settings/voice").view()
