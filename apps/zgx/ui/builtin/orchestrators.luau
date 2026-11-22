-- Orchestrators' app-side module as the app has it with the plugin off:
-- nothing. The built-in plugin (plugins-builtin/orchestrators) puts its own
-- in this one's place while it's on (`ui.override`, a bundled built-in's
-- trust: docs/plugins.md, "Zeron's own plugins"). Reach it through
-- `require("@account/plugins").module("orchestrators")`, never directly.
return false
