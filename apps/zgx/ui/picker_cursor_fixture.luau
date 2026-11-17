-- A model picker open on Cursor's tab, beside Claude Code's and Codex's,
-- with Cursor's catalog as its agent lists it (Auto, Composer 2 and the
-- models it reaches): the picture of Cursor in the picker, for
-- src/audit_shots.rs (`AUDIT_SHOTS=picker_cursor`), and what
-- tests/model_picker_catalog.test.luau drives. `test_cursor_status` is how
-- Cursor's models stand ("ready", "empty", "error", "loading": the host's
-- `status`), its models listed only while "ready"; `test_picker_calls` the
-- picker's asks, in order ("open", "retry:<agent>", "sign_in:<agent>").
-- The pictures (its tab pressed; then on a look that failed):
--   AUDIT_SHOTS='picker_cursor@520x560|=test_picker_open:1|^role:tab "Cursor"'
--   AUDIT_SHOTS='picker_cursor@520x560|=test_cursor_status:"error"|=test_picker_open:1|^role:tab "Cursor"|~error'
-- Here, beside app.luau, as a mounted script loads modules from its own
-- directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local ModelPicker = require("@ds/model_picker")
local EffortSlider = require("@ds/effort_slider")

local THREE: { EffortSlider.Level } = {
	{ value = "low", label = "Low" },
	{ value = "medium", label = "Medium" },
	{ value = "high", label = "High" },
}

local CURSOR: { ModelPicker.Model } = {
	{ value = "cursor/auto", label = "Auto", harness = "cursor", default = true },
	{ value = "cursor/composer-2", label = "Composer 2", harness = "cursor" },
	{ value = "cursor/claude-4.6-sonnet", label = "Claude 4.6 Sonnet", harness = "cursor" },
	{ value = "cursor/claude-4.6-sonnet-thinking", label = "Claude 4.6 Sonnet Thinking", harness = "cursor" },
	{ value = "cursor/claude-4.7-opus", label = "Claude 4.7 Opus", harness = "cursor" },
	{ value = "cursor/gpt-5.5", label = "GPT-5.5", harness = "cursor" },
	{ value = "cursor/gpt-5.5-codex", label = "GPT-5.5 Codex", harness = "cursor" },
	{ value = "cursor/gemini-3.1-pro", label = "Gemini 3.1 Pro", harness = "cursor" },
	{ value = "cursor/grok-4.7", label = "Grok 4.7", harness = "cursor" },
	{ value = "cursor/kimi-k3", label = "Kimi K3", harness = "cursor" },
}
local OTHERS: { ModelPicker.Model } = {
	{ value = "claude-code/opus", label = "Opus 4.7", harness = "claude-code", efforts = THREE, effort = "high" },
	{ value = "claude-code/sonnet", label = "Sonnet 4.6", harness = "claude-code", efforts = THREE, effort = "medium" },
	{ value = "codex/gpt-5.5", label = "GPT-5.5", harness = "codex", efforts = THREE, effort = "medium" },
	{ value = "codex/gpt-5.5-mini", label = "GPT-5.5 mini", harness = "codex" },
}

local status = ui.state("test_cursor_status", "ready")
local calls = ui.state("test_picker_calls", "")
local function note(what: string)
	local now = calls:peek()
	calls:set(if now == "" then what else `{now},{what}`)
end

local harnesses = ui.computed(function(): { ModelPicker.Harness }
	local s = status:get()
	return {
		{ id = "claude-code", label = "Claude Code", mark = "claude", status = "ready" },
		{ id = "codex", label = "Codex", mark = "openai", status = "ready" },
		{
			id = "cursor",
			label = "Cursor",
			mark = "cursor",
			status = s,
			error = if s == "error" then "cursor-agent models: not reachable (timed out after 20s)" else nil,
		},
	}
end)
local models = ui.computed(function(): { ModelPicker.Model }
	local out = table.clone(OTHERS)
	if status:get() == "ready" then
		for _, m in CURSOR do
			table.insert(out, m)
		end
	end
	return out
end)

-- Bumped, it opens (the pictures' first step).
local open = ui.state("test_picker_open", 0)

return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.4",
	ModelPicker {
		-- No model chosen yet: it opens out, on the first agent's tab, and
		-- a test (or a picture's click) moves to Cursor's.
		model = ui.state("test_picker_model", nil :: string?),
		effort = ui.state("test_picker_effort", nil :: string?),
		favorites = ui.state("test_picker_favorites", {} :: { [string]: boolean }),
		open_with = open,
		harnesses = harnesses,
		models = models,
		on_open = function()
			note("open")
		end,
		on_retry = function(h: string)
			note(`retry:{h}`)
		end,
		on_sign_in = function(h: string)
			note(`sign_in:{h}`)
		end,
	},
}
