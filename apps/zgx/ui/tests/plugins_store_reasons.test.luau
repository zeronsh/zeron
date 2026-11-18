-- Every reason word the store's calls can answer (docs/plugins.md "Reason
-- words") is said in Zeron's words, never the engine's English as it came,
-- whether it came as "<word>: <words>" or as an answer's `error` alone.


local REASONS = {
	"not_a_package", "unsigned", "tampered", "bad_signature", "too_big", "bad_file",
	"incompatible", "other_signer", "not_newer", "different_plugin", "up_to_date", "no_source",
	"store_unreachable", "not_in_store", "store_mismatch", "store_error", "rate_limited",
	"signed_out", "review", "changed", "not_published", "step_up_required",
	"bad_package", "id_reserved", "not_semver", "missing_readme", "missing_listing", "bad_listing", "too_large",
	"binary_file", "no_notes", "needs_username", "id_taken", "version_exists", "suspended",
	"key_not_allowed", "key_revoked", "key_expired", "invalid_signature", "unauthenticated",
	"stale_request", "replayed_request", "body_mismatch",
	"too_many_keys", "key_taken", "invalid_name", "invalid_expiry", "invalid_proof", "bad_csrf",
}

local function said(): { [string]: any }
	local app = mount("plugins_store_fixture.luau")
	app:frames(3)
	local asked = table.clone(REASONS)
	table.insert(asked, "something_new")
	app:set_state("reason_asked", asked)
	app:frames(3)
	return app:state("reason_said")
end

test("every reason word the store answers is said in Zeron's words", function()
	local out = said()
	for _, word in REASONS do
		local w = out[word]
		local ok = w ~= nil and w.came ~= `{word}: the engine's English` and w.came ~= "" and w.alone == w.came
		expect(`{word}: {tostring(ok)}`).to_be(`{word}: true`)
	end
end)

test("an unknown word is shown as it came", function()
	expect(said().something_new.came).to_be("something_new: the engine's English")
end)
