-- Settings → Voice: a row a speech model (ui/settings_stub.luau lists
-- Parakeet Ultra here and in use, Cohere Transcribe not downloaded, slower
-- here). Removing the model in use leaves dictation without one, as before
-- any download; a model downloaded but not in use offers Use. How long the
-- model stays ready is chosen under them.

local PAGE = "#settings-page"

local function voice(stub: any?): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	if stub then app:set_state("settings_stub_voice", stub) end
	app:set_state("settings_dictation", true)
	app:command("shell.settings")
	app:frames(20)
	app:click('#settings-nav >> role:tab "Voice"')
	app:frames(30)
	return app
end

local function shows(app: any, text: string): boolean
	return app:tree(PAGE):find(text, 1, true) ~= nil
end

test("each model says its size and languages, the one in use marked", function()
	local app = voice()
	expect(shows(app, "Parakeet Ultra")).to_be(true)
	expect(shows(app, "740 MB · 25 languages")).to_be(true)
	expect(shows(app, "1.8 GB · 14 languages · Slower on this computer")).to_be(true)
	expect(shows(app, "In use")).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Download"`)).to_be(true)
	-- How long it stays ready, 5 minutes unless chosen, and what that holds.
	expect(shows(app, "Keep the model ready")).to_be(true)
	expect(shows(app, "5 minutes")).to_be(true)
	expect(shows(app, "Parakeet Ultra holds 1.1 GB of memory")).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("removing the model in use turns dictation off, nothing in use", function()
	local app = voice()
	app:click(`{PAGE} >> role:button "Remove"`)
	app:frames(10)
	expect(app:state("settings_dictation")).to_be(false)
	expect(shows(app, "In use")).to_be(false)
	expect(app:count(`{PAGE} >> role:button "Download"`)).to_be(2)
	expect(app:error()).to_be(nil)
end)

test("a processor that can't run them says so and offers no download", function()
	local app = voice({ cohere = "absent", supported = false })
	expect(shows(app, "can't run speech models")).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Download"`)).to_be(false)
	expect(app:error()).to_be(nil)
end)

test("removing the model in use hands over to another downloaded one", function()
	local app = voice({ cohere = "installed", supported = true })
	app:click(`{PAGE} >> role:button "Remove"[1]`)
	app:frames(10)
	expect(app:state("settings_dictation")).to_be(true)
	expect(app:state("settings_dictation_model")).to_be("cohere-transcribe-q5")
	expect(app:error()).to_be(nil)
end)

test("a stopped download offers to resume or remove what it left", function()
	local app = voice({ cohere = "partial", supported = true })
	expect(app:count(`{PAGE} >> role:button "Remove"`)).to_be(2)
	expect(app:has(`{PAGE} >> role:button "Download"`)).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("a second model downloads on its own row, the switch left as it is", function()
	local app = voice()
	app:click(`{PAGE} >> role:button "Download"`)
	app:set_state("dictation", { phase = "downloading", bytes = 900000000, total = 1796963840 })
	app:frames(5)
	expect(shows(app, "Downloading · 900 MB of 1.8 GB")).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Cancel"`)).to_be(true)
	expect(app:state("settings_voice_on")).to_be(true)
	-- It stops: the switch still says dictation is on (Ultra in use).
	app:set_state("dictation", { phase = "cancelled" })
	app:frames(5)
	expect(app:has(`{PAGE} >> role:button "Cancel"`)).to_be(false)
	expect(app:state("settings_voice_on")).to_be(true)
	expect(app:error()).to_be(nil)
end)
