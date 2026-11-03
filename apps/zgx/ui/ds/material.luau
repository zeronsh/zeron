-- How Zeron's surfaces stand: frosted (the window's glass; menus over the
-- platform's frosted material) or opaque (solid colours throughout). One
-- setting for the whole app, kept across launches; components read it.

local M = {}

M.frosted = ui.persist("frosted", true)

return M
