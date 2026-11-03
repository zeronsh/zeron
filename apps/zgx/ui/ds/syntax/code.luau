-- The C-like languages: each one a table of rules for the shared lexer
-- (lex.luau), its words written out here and nothing else.

local lex = require("@self/lex")

local set = lex.set

-- The usual string kinds.
local DOUBLE = { open = '"' }
local SINGLE = { open = "'" }
local function multi(open: string, interp: string?): { [string]: any }
	return { open = open, multi = true, interp = interp }
end

local SLASHES = { "//" }
local BLOCK = { "/*", "*/" }

local JS_KEYWORDS = "async await break case catch class const continue debugger default delete do else export extends finally for from function get if import in instanceof let new of return set static switch throw try typeof var void while with yield as satisfies"
local TS_KEYWORDS = JS_KEYWORDS
	.. " abstract declare enum implements interface keyof namespace private protected public readonly type infer is asserts override unique module"
local JS_CONSTANTS = "true false null undefined NaN Infinity"
local TS_TYPES = "string number boolean any unknown never void object symbol bigint"

local function js(ts: boolean, jsx: boolean): { [string]: any }
	return {
		line = SLASHES,
		block = BLOCK,
		strings = { DOUBLE, SINGLE, multi("`", "${") },
		keywords = set(if ts then TS_KEYWORDS else JS_KEYWORDS),
		constants = set(JS_CONSTANTS),
		variables = set("this super arguments globalThis"),
		types = if ts then set(TS_TYPES) else nil,
		defs = set("function"),
		decls = set("class interface type enum namespace"),
		caps_type = true,
		decorators = true,
		regex = true,
		jsx = jsx,
	}
end

local C_KEYWORDS = "auto break case const continue default do else enum extern for goto if inline register restrict return sizeof static struct switch typedef union volatile while _Alignas _Alignof _Atomic _Generic _Noreturn _Static_assert _Thread_local"
local C_TYPES = "char double float int long short signed unsigned void bool size_t ssize_t ptrdiff_t intptr_t uintptr_t int8_t int16_t int32_t int64_t uint8_t uint16_t uint32_t uint64_t FILE"
local CPP_KEYWORDS = C_KEYWORDS
	.. " alignas alignof and asm catch class co_await co_return co_yield concept consteval constexpr constinit decltype delete explicit export friend mutable namespace new noexcept not operator or override private protected public requires static_assert static_cast dynamic_cast reinterpret_cast const_cast template this thread_local throw try typeid typename using virtual final xor"

local specs: { [string]: { [string]: any } } = {
	rust = {
		line = SLASHES,
		block = BLOCK,
		nest = true,
		strings = { multi('"') },
		keywords = set(
			"as async await break const continue crate dyn else enum extern fn for if impl in let loop match mod move mut pub ref return static struct super trait type union unsafe use where while yield macro_rules"
		),
		constants = set("true false None Some Ok Err"),
		variables = set("self Self"),
		types = set(
			"i8 i16 i32 i64 i128 isize u8 u16 u32 u64 u128 usize f32 f64 bool char str String Vec Option Result Box Rc Arc"
		),
		defs = set("fn"),
		decls = set("struct enum trait type union mod"),
		caps_type = true,
		chars = true,
		lifetimes = true,
		macros = true,
		attributes = true,
		rust_raw = true,
		format = true,
	},
	typescript = js(true, false),
	tsx = js(true, true),
	javascript = js(false, true),
	jsx = js(false, true),
	luau = {
		line = { "--" },
		long = true,
		strings = { DOUBLE, SINGLE, { open = "`", interp = "{" } },
		keywords = set(
			"and break continue do else elseif end for function if in local not or repeat return then until while export type typeof"
		),
		constants = set("true false nil"),
		variables = set("self"),
		types = set("number string boolean any unknown never thread buffer vector"),
		defs = set("function"),
		decls = set("type"),
		decorators = true,
	},
	lua = {
		line = { "--" },
		long = true,
		strings = { DOUBLE, SINGLE },
		keywords = set("and break do else elseif end for function goto if in local not or repeat return then until while"),
		constants = set("true false nil"),
		variables = set("self _G _ENV"),
		defs = set("function"),
	},
	python = {
		line = { "#" },
		strings = { multi('"""'), multi("'''"), DOUBLE, SINGLE },
		prefixes = set("r u b f rb br fr rf"),
		keywords = set(
			"and as assert async await break class continue def del elif else except finally for from global if import in is lambda nonlocal not or pass raise return try while with yield match case type"
		),
		constants = set("True False None NotImplemented Ellipsis __name__ __file__"),
		variables = set("self cls"),
		types = set("int float str bool bytes list dict set tuple object type complex frozenset bytearray"),
		defs = set("def"),
		decls = set("class"),
		caps_type = true,
		decorators = true,
	},
	go = {
		line = SLASHES,
		block = BLOCK,
		strings = { DOUBLE, { open = "`", escape = false, multi = true } },
		keywords = set(
			"break case chan const continue default defer else fallthrough for func go goto if import interface map package range return select struct switch type var"
		),
		constants = set("true false nil iota"),
		types = set(
			"bool byte complex64 complex128 error float32 float64 int int8 int16 int32 int64 rune string uint uint8 uint16 uint32 uint64 uintptr any comparable"
		),
		defs = set("func"),
		decls = set("type"),
		caps_type = true,
		chars = true,
	},
	swift = {
		line = SLASHES,
		block = BLOCK,
		nest = true,
		strings = { multi('"""', "\\("), { open = '"', interp = "\\(" } },
		keywords = set(
			"actor associatedtype async await break case catch class continue default defer deinit do else enum extension fallthrough fileprivate final for func guard if import in indirect init inout internal is lazy let mutating nonmutating open operator override private protocol public repeat rethrows return some any static struct subscript super switch throw throws try typealias var weak where while unowned convenience required dynamic optional get set willSet didSet consume borrowing consuming"
		),
		constants = set("true false nil"),
		variables = set("self Self super"),
		types = set("Int Int8 Int16 Int32 Int64 UInt UInt8 UInt16 UInt32 UInt64 Double Float Bool String Character Void Any AnyObject"),
		defs = set("func"),
		decls = set("class struct enum protocol extension actor typealias"),
		caps_type = true,
		decorators = true,
		preprocessor = true,
	},
	kotlin = {
		line = SLASHES,
		block = BLOCK,
		nest = true,
		strings = { { open = '"""', escape = false, multi = true, interp = "$" }, { open = '"', interp = "$" } },
		keywords = set(
			"abstract actual annotation as break by catch class companion const constructor continue crossinline data do else enum expect external final finally for fun get if import in infix init inline inner interface internal is lateinit noinline object open operator out override package private protected public reified return sealed set super suspend tailrec throw try typealias val value var vararg when where while"
		),
		constants = set("true false null"),
		variables = set("this it"),
		types = set("Int Long Short Byte Double Float Boolean Char String Unit Any Nothing Array List Map Set"),
		defs = set("fun"),
		decls = set("class interface object typealias"),
		caps_type = true,
		chars = true,
		decorators = true,
	},
	java = {
		line = SLASHES,
		block = BLOCK,
		strings = { multi('"""'), DOUBLE },
		keywords = set(
			"abstract assert break case catch class const continue default do else enum extends final finally for goto if implements import instanceof interface native new package private protected public return static strictfp super switch synchronized throw throws transient try var volatile while record sealed permits yield"
		),
		constants = set("true false null"),
		variables = set("this super"),
		types = set("boolean byte char double float int long short void String Object"),
		decls = set("class interface enum record"),
		caps_type = true,
		chars = true,
		decorators = true,
	},
	csharp = {
		line = SLASHES,
		block = BLOCK,
		strings = { multi('"""'), DOUBLE },
		prefixes = set("@ $"),
		keywords = set(
			"abstract as async await base break case catch checked class const continue default delegate do else enum event explicit extern finally fixed for foreach get goto if implicit in init interface internal is lock namespace new operator out override params partial private protected public readonly record ref required return sealed set sizeof stackalloc static struct switch throw try typeof unchecked unsafe using var virtual volatile when where while yield"
		),
		constants = set("true false null"),
		variables = set("this base value"),
		types = set("bool byte char decimal double dynamic float int long nint nuint object sbyte short string uint ulong ushort void"),
		decls = set("class interface enum struct record namespace"),
		caps_type = true,
		chars = true,
		preprocessor = true,
	},
	c = {
		line = SLASHES,
		block = BLOCK,
		strings = { DOUBLE },
		keywords = set(C_KEYWORDS),
		constants = set("true false NULL"),
		types = set(C_TYPES),
		decls = set("struct union enum"),
		chars = true,
		preprocessor = true,
	},
	cpp = {
		line = SLASHES,
		block = BLOCK,
		strings = { DOUBLE },
		keywords = set(CPP_KEYWORDS),
		constants = set("true false NULL nullptr"),
		variables = set("this"),
		types = set(C_TYPES .. " wchar_t char8_t char16_t char32_t std string vector map unique_ptr shared_ptr"),
		decls = set("struct union enum class namespace"),
		chars = true,
		preprocessor = true,
		cpp_raw = true,
	},
	ruby = {
		line = { "#" },
		strings = { { open = '"', multi = true, interp = "#{" }, { open = "'", multi = true } },
		keywords = set(
			"alias and begin break case class def defined? do else elsif end ensure for if in module next not or redo rescue retry return then undef unless until when while yield require require_relative attr_reader attr_writer attr_accessor private protected public"
		),
		constants = set("true false nil"),
		variables = set("self super"),
		defs = set("def"),
		decls = set("class module"),
		caps_type = true,
		ident = "^[%a_][%w_]*[%?!]?",
	},
	json = {
		line = SLASHES,
		block = BLOCK,
		strings = { DOUBLE },
		constants = set("true false null"),
		key = ":",
	},
	toml = {
		line = { "#" },
		strings = { multi('"""'), { open = "'''", escape = false, multi = true }, DOUBLE, { open = "'", escape = false } },
		constants = set("true false inf nan"),
		ident = "^[%a_][%w_%-]*",
		key = "=",
		headers = true,
		number = "^[%d][%w%-%:%.%+_]*",
	},
	sql = {
		line = { "--" },
		block = BLOCK,
		strings = { { open = "'", escape = false, multi = true }, { open = "`", escape = false } },
		fold = true,
		keywords = set(
			"select from where and or not in is null like ilike between exists as on join inner left right full outer cross natural using group by order asc desc having limit offset union all distinct insert into values update set delete create alter drop table index view trigger function procedure database schema if replace primary key foreign references unique check default constraint cascade begin commit rollback transaction case when then else end with recursive returning conflict do nothing explain analyze vacuum grant revoke over partition window filter lateral"
		),
		constants = set("true false null current_date current_time current_timestamp"),
		types = set(
			"int integer bigint smallint tinyint serial bigserial real double precision float decimal numeric boolean bool char varchar text blob bytea date time timestamp timestamptz interval json jsonb uuid"
		),
		ident = "^[%a_][%w_$]*",
	},
	wgsl = {
		line = SLASHES,
		block = BLOCK,
		nest = true,
		strings = {},
		keywords = set(
			"alias break case const const_assert continue continuing default diagnostic discard else enable fn for if let loop override requires return struct switch var while private function workgroup uniform storage read write read_write"
		),
		constants = set("true false"),
		types = set(
			"bool f16 f32 i32 u32 vec2 vec3 vec4 vec2f vec3f vec4f vec2i vec3i vec4i vec2u vec3u vec4u mat2x2 mat2x3 mat2x4 mat3x2 mat3x3 mat3x4 mat4x2 mat4x3 mat4x4 mat2x2f mat3x3f mat4x4f array atomic ptr sampler sampler_comparison texture_1d texture_2d texture_2d_array texture_3d texture_cube texture_cube_array texture_multisampled_2d texture_storage_2d texture_depth_2d texture_external"
		),
		defs = set("fn"),
		decls = set("struct alias"),
		caps_type = true,
		decorators = true,
	},
}

local M = {}
for id, spec in specs do
	M[id] = lex.code(spec)
end
return M
