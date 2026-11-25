-- A template's own tokens: where Zeron lays a template's ds/tokens.luau,
-- beside Zeron's own rather than over them. ds/tokens declares Zeron's
-- first and then runs these, so a token the template names is the
-- template's and every other one is this Zeron's (those added after the
-- template was made too). With no template, this: nothing more.

return {
	setup = function() end,
}
