-- The theme's look for the composer's markdown (ds/markdown_edit `look`),
-- for tests/composer_markdown.test.luau: read once the theme is set up, as
-- the composer reads it, and kept in `composer_markdown_look`.

ui.theme.strict("warn")
local ds = require("@ds")
local markdown = require("@ds/markdown_edit")
ds.setup()

ui.state("composer_markdown_look", markdown.look())

return ui.div {}
