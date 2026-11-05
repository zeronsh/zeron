-- Zeron's design system, on zgx's primitives. Three layers, as zgx's
-- design guide has them: the theme (tokens.luau, themes/*.json) holds every
-- value; the components here turn tokens into styles with `ui.variants`;
-- the design lint (`ui.theme.strict`, on in every entry but the intro)
-- keeps screens from going around them.
--
-- Screens take what they need from here:
--   local ds = require("@ds")
--   ds.Text { "Welcome", style = "display" }

-- (First: the words' catalog, before any module makes a message.)
local locale = require("@self/locale")
local tokens = require("@self/tokens")
local mark = require("@self/mark")
local surface = require("@self/surface")

export type MarkName = mark.Name
export type SurfaceKind = surface.Kind
local member_card = require("@self/member_card")
local composer = require("@self/composer")
local model_picker = require("@self/model_picker")
local status = require("@self/status")
local sidebar = require("@self/sidebar")
export type SidebarSession = sidebar.Session
export type SidebarProject = sidebar.Project
export type StatusKind = status.Kind
export type Harness = model_picker.Harness
export type Model = model_picker.Model
export type ComposerDevice = composer.Device
export type ComposerUsage = composer.Usage
export type ComposerFixed = composer.Fixed
export type CardLayer = member_card.Layer
export type CardBadge = member_card.Badge
local card_wall = require("@self/card_wall")
local onboarding_frame = require("@self/onboarding_frame")
export type OnboardingStep = onboarding_frame.StepProps
export type OnboardingForward = onboarding_frame.Forward
export type OnboardingQuiet = onboarding_frame.Quiet
export type OnboardingForm = onboarding_frame.FormProps
export type CardMember = card_wall.Member
export type CardColumn = card_wall.Column
-- The primitives' modules that export several pieces.
local badge = require("@self/badge")
local people = require("@self/people")
local banner = require("@self/banner")
local dialog = require("@self/dialog")
local progress = require("@self/progress")
local layout = require("@self/layout")
local toolbar = require("@self/toolbar")
local comment = require("@self/comment")
local inline_block = require("@self/inline_block")
local onboarding = require("@self/onboarding")
local skeleton = require("@self/skeleton")

return {
	-- Loads the themes and declares the app's tokens: an entry calls it
	-- first, before it builds anything.
	setup = tokens.setup,
	-- Dates and times in the locale's words (ds/locale).
	Locale = locale,
	Text = require("@self/text"),
	Button = require("@self/button"),
	Surface = surface,
	Mark = mark,
	Icon = require("@self/icon"),
	MemberCard = member_card,
	-- The ways to share a card, as one control (under a card, or beside one).
	CardActions = require("@self/card_actions"),
	-- A card from a seed (a user id): its scene, foil and sky, as the
	-- backend and the web compute them; and the scenes a seed picks among.
	CardSeed = require("@self/card_seed"),
	-- Columns of member cards drifting beside a screen's words.
	CardWall = card_wall,
	CardScenes = require("@self/card_scenes"),
	Steps = require("@self/steps"),
	Switch = require("@self/switch"),
	Segmented = require("@self/segmented"),
	Dropdown = require("@self/dropdown"),
	Composer = composer,
	-- What shows as the pointer rests on something: every node tooltip's
	-- frame (`HoverCard.Card`, `HoverCard.Tip`).
	HoverCard = require("@self/hover_card"),
	-- How full a model's context is: the composer ring's card and its words.
	context_usage = require("@self/context_usage"),
	ModelPicker = model_picker,
	EffortSlider = require("@self/effort_slider"),
	RollText = require("@self/roll_text"),
	SidebarIcon = require("@self/sidebar_icon"),
	Sidebar = sidebar,
	-- A session's hover card (the sidebar's rest on a row; the shell places it).
	SessionCard = require("@self/session_card"),
	FadeText = require("@self/fade_text"),
	-- An email address that can be hidden, blurred (Settings → General).
	PrivateText = require("@self/private_text"),
	Tooltip = require("@self/tooltip"),
	AccountRow = require("@self/account_row"),
	Avatar = require("@self/avatar"),
	Tile = require("@self/tile"),
	TitleBar = require("@self/title_bar"),
	-- Controls and pieces every surface shares.
	IconButton = require("@self/icon_button").IconButton,
	ToggleButton = require("@self/toggle_button"),
	Menu = require("@self/menu"),
	Glass = require("@self/glass").Panel,
	glass = require("@self/glass").style,
	SectionTitle = require("@self/section_title"),
	Hint = require("@self/hint"),
	-- Primitives: fields, choices, notices, overlays, and the pieces
	-- settings, onboarding and the git pages are built from.
	Kbd = require("@self/kbd"),
	TextField = require("@self/text_field"),
	TextArea = require("@self/text_area"),
	SearchField = require("@self/search_field"),
	TokenField = require("@self/token_field"),
	Checkbox = require("@self/checkbox"),
	RadioGroup = require("@self/radio"),
	Select = require("@self/select"),
	Slider = require("@self/slider"),
	NumberStepper = require("@self/stepper"),
	Tabs = require("@self/tabs"),
	Badge = badge.Badge,
	Tag = badge.Tag,
	Count = badge.Count,
	Dot = badge.Dot,
	PullRequestBadge = badge.PullRequest,
	ReviewBadge = badge.Review,
	ChecksBadge = badge.Checks,
	FileStatus = badge.FileStatus,
	DiffStat = badge.DiffStat,
	AvatarStack = people.AvatarStack,
	Person = people.Person,
	Timestamp = people.Timestamp,
	Callout = banner.Callout,
	Banner = banner.Banner,
	toast = require("@self/toast"),
	Dialog = dialog.Dialog,
	Sheet = dialog.Sheet,
	EmptyState = require("@self/empty_state"),
	Skeleton = skeleton.Bar,
	SkeletonRow = skeleton.Row,
	ProgressBar = progress.ProgressBar,
	ProgressRing = progress.ProgressRing,
	Divider = layout.Divider,
	Link = layout.Link,
	Breadcrumbs = layout.Breadcrumbs,
	KeyValue = layout.KeyValue,
	SplitButton = require("@self/split_button"),
	Toolbar = toolbar.Toolbar,
	ToolbarGroup = toolbar.Group,
	ToolbarSpacer = toolbar.Spacer,
	CommentCard = comment.CommentCard,
	CommentThread = comment.Thread,
	-- What stands between two lines of code.
	InlineBlock = inline_block.InlineBlock,
	InlineComposer = inline_block.Composer,
	InlineThread = inline_block.Thread,
	InlineNote = inline_block.Note,
	InlineAdd = inline_block.Add,
	ReplyComposer = comment.ReplyComposer,
	Settings = require("@self/settings"),
	-- A new chat's wallpaper: a picture, or a plugin's, under an effect.
	Wallpaper = require("@self/wallpaper").Wallpaper,
	wallpaper = require("@self/wallpaper"),
	-- A project's files beside the one they show, the tree hideable.
	FilesPane = require("@self/files_pane"),
	-- The app's scrollbar, floating over a scroll view's trailing edge.
	Rail = require("@self/rail"),
	Hero = onboarding.Hero,
	StepHeader = onboarding.StepHeader,
	OptionCards = onboarding.OptionCards,
	-- Setup's window and its steps: the frame, a step with its footer, the
	-- footer's pills, a step's form.
	Onboarding = onboarding_frame,
	-- Make it yours: the scheme, the window's material and the accent.
	AppearancePicker = require("@self/appearance_picker"),
	-- The organizations someone is in, and invitations, to choose one.
	OrgList = require("@self/org_list"),
	-- The address a name makes, and whether it's free.
	AddressLine = require("@self/address_line"),
	-- GitHub for Cloud agents: Settings → Cloud's block, in setup too.
	GitHubBlock = require("@self/github_block"),
	-- An agent on a device and its accounts (Settings → Providers' cards).
	ProviderCard = require("@self/provider_card"),
	-- An agent's mark: Zeron's own, a registry agent's icon, or its initial.
	AgentMark = require("@self/agent_mark"),
	-- The accent the app is drawn in, as the material is (ds.material).
	look = require("@self/look"),
	Status = status.Status,
	StatusDot = status.StatusDot,
	StatusIcon = status.StatusIcon,
	StatusTag = status.StatusTag,
	-- Effects.
	decode = require("@self/decode"),
	effects = require("@self/effects"),
	island = require("@self/island"),
	-- Frosted or opaque, for the whole app.
	material = require("@self/material"),
}
