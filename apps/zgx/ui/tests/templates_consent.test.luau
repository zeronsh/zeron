-- The template consent sheet says plainly that a template is Zeron's own
-- code with all its powers, who signed it, that its signature checks out,
-- and which files it may not replace; Install and Not now answer with
-- exactly what was shown (its hash); an unknown signer says so, and the
-- sheet follows once Zeron has looked them up. On the sheet in
-- ui/template_consent_fixture.luau.

local function open(): any
	local app = mount("template_consent_fixture.luau")
	app:resize(460, 540)
	app:frames(20)
	return app
end

test("it says what a template can do, who signed it, and what's left out", function()
	local app = open()
	local tree = app:tree("#template-consent")
	expect(tree).to_contain("Install Ledger?")
	expect(tree).to_contain("Made by @ada")
	expect(tree).to_contain("Install it only if you trust @ada")
	expect(tree).to_contain("A template is Zeron's own code")
	expect(tree).to_contain("Its signature checks out")
	expect(tree).to_contain("by key AbCdEfGh")
	expect(tree).to_contain("42 files")
	expect(tree).to_contain("2 files left out")
	expect(tree).to_contain("No template may replace these")
	expect(tree).never.to_contain("Unknown author")
end)

test("Install and Not now answer with what was shown", function()
	local app = open()
	app:click("#template-consent-allow")
	app:frames(20)
	expect(app:state("calls")).to_contain("decide:ledger-1700000000:true:ledger-hash-1")
	expect(app:tree("#template-consent")).to_contain("Nothing waiting")
	app:command("mystery")
	app:frames(20)
	app:click("#template-consent-decline")
	app:frames(20)
	expect(app:state("calls")).to_contain("decide:mystery-1700000001:false:mystery-hash-1")
end)

test("an unknown signer is named so, and the sheet follows once they're found", function()
	local app = open()
	app:command("mystery")
	app:frames(20)
	local tree = app:tree("#template-consent")
	expect(tree).to_contain("Unknown author")
	expect(tree).to_contain("Checking who made it")
	expect(tree).to_contain("Install it only if you trust whoever sent it")
	expect(tree).to_contain("1 file")
	expect(tree).never.to_contain("left out")
	app:command("found")
	app:frames(20)
	tree = app:tree("#template-consent")
	expect(tree).to_contain("Made by @grace")
	expect(tree).to_contain("trust @grace")
	expect(tree).never.to_contain("Unknown author")
end)

test("while it's being fetched, nothing can be answered", function()
	local app = open()
	app:command("loading")
	app:frames(20)
	expect(app:tree("#template-consent")).to_contain("Checking the template")
	expect(app:has("#template-consent-allow")).to_be(false)
end)
