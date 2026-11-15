-- PR #1436 visual review: real app shell/components, isolated synthetic data.
local store = require("@app/screens/chat/store")
local offers_auto = store.offers_auto
local shell = require("./shell_fixture")
-- shell_fixture stubs host functions; keep the real capability resolver.
store.offers_auto = offers_auto
ui.theme.follow_system(false)
ui.theme.current:set("zeron-dark")
local route = require("@app/screens/shell/route")
store.catalog:set({
    state = "ready", complete = true,
    harnesses = {
        { id = "codex", label = "Codex", mark = "openai", auto = true },
        { id = "claude-code", label = "Claude Code", mark = "claude", auto = true },
        { id = "devin", label = "Devin", mark = "devin", auto = false },
    },
    models = {
        { value = "codex/gpt-6", label = "GPT-6", harness = "codex" },
        { value = "claude-code/sonnet", label = "Sonnet", harness = "claude-code" },
        { value = "devin/fusion", label = "Fusion", harness = "devin" },
    },
} :: any)
ui.shared("fixture.chats", {} :: { [string]: any }):set({
    a2 = {
        session = { id = "a2", state = "ready", title = "Auto access review", project = "atlas", device = "mac", harness = "codex", model = "codex/gpt-6", model_label = "GPT-6", access = "auto", running = false },
        messages = {
            { id = "u1", role = "user", text = "Check the workspace and show which actions Auto reviewed." },
            { id = "a1", role = "assistant", model = "GPT-6", text = "I checked the project and reviewed the requested changes.", tools = {
                { id = "t1", kind = "read", verb = "Read", target = "Cargo.toml" },
                { id = "t2", kind = "exec", verb = "Ran", target = "cargo check -p zeron-proto", review = { status = "approved", risk = "low", reason = "Reads project sources and writes build artifacts inside the workspace." } },
                { id = "t3", kind = "exec", verb = "Ran", target = "rm -rf /tmp/review-example", failed = true, review = { status = "denied", risk = "high", reason = "Deletes files outside the project without explicit authorization." } },
                { id = "t4", kind = "exec", verb = "Ran", target = "curl https://example.com/status", failed = true, review = { status = "timed-out", reason = "The reviewer did not return a decision before the deadline." } },
            } },
            { id = "a2-reply", role = "assistant", model = "GPT-6", text = "The workspace check was approved. The deletion was blocked, and the network review timed out." },
        },
    },
})
ui.state("activity_groups_chosen", {} :: { [string]: boolean }):set({ a1 = true })
ui.after(0, function()
    route.open_session("a2")
end)
-- The native host may clear chat state when its isolated engine is absent.
-- Keep the fixture data, just as the headless fixture host does.
ui.effect(function()
    if store.session:get() == nil then
        local data = ui.shared("fixture.chats", {} :: { [string]: any }):peek().a2
        if data then
            store.session:set(data.session)
            store.messages:set(data.messages)
        end
    end
end)
return shell
