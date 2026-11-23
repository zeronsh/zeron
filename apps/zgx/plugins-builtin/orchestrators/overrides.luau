--!strict
-- The app modules Orchestrators puts its own in place of: each one its
-- manifest names (`app_modules`), which a bundled built-in is trusted with.
-- Kept apart from init.luau, needing nothing but `ui`, so the Luau UI tests
-- load these alone into the app they run (scripts/ci/test-zgx.sh).

ui.override { module = "@app/builtin/orchestrators", with = "./index" }

-- (A module returns one value: nothing else here.)
return nil
