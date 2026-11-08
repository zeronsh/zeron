-- Headless route-motion tests drive this clock alongside TestApp's frames.
-- ui.now normally reads wall time, unlike the harness's frame/timer clocks.
-- Keep the override in this fixture's VM; the app and visual fixtures use
-- the real clock. Peek so advancing time alone starts no reactive work.
local now = ui.state("fixture_composer_now", 0)
local clock = require("@ds/composer_clock")
clock.now = function(): number
	return now:peek()
end

return require("./shell_fixture")
