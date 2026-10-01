package sh.zeron.android.ui

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.SizeTransform
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.DragInteraction
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.derivedStateOf
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import kotlinx.coroutines.launch
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.zeron.android.core.FavoriteModel
import sh.zeron.android.core.AccountUsage
import sh.zeron.android.design.HarnessMark
import sh.zeron.android.design.LocalDarkTheme
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import sh.zeron.android.feedback.Cue
import sh.zeron.android.feedback.Haptic
import sh.zeron.android.feedback.LocalFeedback
import sh.zeron.android.feedback.tapAction
import uniffi.zeron_core.ModelInfo
import uniffi.zeron_core.ModelOption
import uniffi.zeron_core.reasoningLabel

/** The desktop's warning amber (filling context). */
@Composable
fun warningColor(): Color = if (LocalDarkTheme.current) Color(0xFFFBBF24) else Color(0xFFB45309)

private val CardWidth = 344.dp
private val RowHeight = 52.dp

/** What the card is showing: its controls, the model list, or one option's choices. */
private sealed interface PickerView {
    data object Settings : PickerView
    data object Models : PickerView
    data class Choices(val optionId: String) : PickerView
}

/**
 * The compact model picker (the desktop's compact picker, adapted to touch):
 * a small card over the chip with the effort name big, the model beneath it
 * (tap for the model list), a fast-mode button, an effort slider and rows for
 * the model's other options. Everything the picker changes is reported as it
 * happens; nothing waits for a Done.
 *
 * [effort] and [options] are the explicit picks (null / absent = the model's
 * defaults). A [locked] picker belongs to an open session, which keeps its
 * harness. [statuses] are catalogs still loading or failed ([onRetry] is
 * called with the failed one's harness).
 */
@Composable
fun ModelPickerPopover(
    expanded: Boolean,
    onDismiss: () -> Unit,
    catalog: List<ModelChoice>,
    current: ModelChoice?,
    favorites: List<FavoriteModel>,
    onToggleFavorite: (FavoriteModel) -> Unit,
    onPick: (ModelChoice) -> Unit,
    effort: String?,
    onEffort: (String?) -> Unit,
    options: Map<String, String>,
    onOptions: (Map<String, String>) -> Unit,
    locked: Boolean = false,
    statuses: List<CatalogStatus> = emptyList(),
    onRetry: (String) -> Unit = {},
) {
    val feedback = LocalFeedback.current
    val rail = remember { RailHost() }
    // Open / Close feedback comes from AnchoredPopover itself (ExpandedFeedback).
    AnchoredPopover(
        expanded, onDismiss, width = CardWidth, maxHeight = 560.dp,
        // The provider rail straddles the card's start edge, outside what the card clips.
        overhang = RailOverhang,
        overlay = { RailOverlay(rail) },
    ) {
        var view by remember { mutableStateOf<PickerView>(if (current == null) PickerView.Models else PickerView.Settings) }
        val spatial = MaterialTheme.motionScheme.defaultSpatialSpec<IntOffset>()
        val sizeSpec = MaterialTheme.motionScheme.defaultSpatialSpec<androidx.compose.ui.unit.IntSize>()
        val effects = MaterialTheme.motionScheme.fastEffectsSpec<Float>()
        AnimatedContent(
            targetState = view,
            transitionSpec = {
                // Sub-pages slide in from the right (back reverses it); the card's height eases between them.
                val sign = if (targetState is PickerView.Settings) -1 else 1
                (slideInHorizontally(spatial) { sign * it / 5 } + fadeIn(effects)) togetherWith
                    (slideOutHorizontally(spatial) { -sign * it / 5 } + fadeOut(effects)) using
                    SizeTransform(clip = true) { _, _ -> sizeSpec }
            },
            label = "picker view",
        ) { shown ->
            Column {
                when (shown) {
                    PickerView.Settings -> {
                        val model = current?.model
                        val fast = ModelOptions.isFast(model, options)
                        Box {
                            // Fast mode's lightning is the bottom layer: above only the card's surface.
                            FastLightning(active = fast, modifier = Modifier.matchParentSize())
                            Column {
                                SettingsCard(
                                    current = current,
                                    effort = effort,
                                    options = options,
                                    onEffort = onEffort,
                                    onOptions = onOptions,
                                    onReset = {
                                        feedback.haptic(Haptic.Confirm)
                                        onEffort(null)
                                        onOptions(emptyMap())
                                    },
                                    onModels = { feedback.haptic(Haptic.Tick); view = PickerView.Models },
                                    onChoices = { feedback.haptic(Haptic.Tick); view = PickerView.Choices(it) },
                                )
                            }
                        }
                    }
                    PickerView.Models -> ModelList(
                        rail = rail,
                        catalog = catalog,
                        current = current,
                        favorites = favorites,
                        locked = locked,
                        statuses = statuses,
                        onBack = { feedback.haptic(Haptic.Tick); if (current != null) view = PickerView.Settings else onDismiss() },
                        onToggleFavorite = onToggleFavorite,
                        onRetry = onRetry,
                        onPick = {
                            feedback.both(Haptic.Select, Cue.Select)
                            onPick(it)
                            view = PickerView.Settings
                        },
                    )
                    is PickerView.Choices -> {
                        val option = current?.model?.options?.firstOrNull { it.id == shown.optionId }
                        if (option == null) {
                            LaunchedEffect(Unit) { view = PickerView.Settings }
                        } else {
                            OptionChoices(
                                option = option,
                                picks = options,
                                onBack = { feedback.haptic(Haptic.Tick); view = PickerView.Settings },
                                onPick = { choice ->
                                    feedback.both(Haptic.Select, Cue.Select)
                                    onOptions(ModelOptions.with(options, option, choice))
                                    view = PickerView.Settings
                                },
                            )
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Settings card
// ---------------------------------------------------------------------------

@Composable
private fun ColumnScope.SettingsCard(
    current: ModelChoice?,
    effort: String?,
    options: Map<String, String>,
    onEffort: (String?) -> Unit,
    onOptions: (Map<String, String>) -> Unit,
    onReset: () -> Unit,
    onModels: () -> Unit,
    onChoices: (String) -> Unit,
) {
    val feedback = LocalFeedback.current
    val model = current?.model
    val levels = model?.reasoningLevels.orEmpty()
    val shownEffort = ModelOptions.effort(model, effort)
    val fastMode = ModelOptions.fastMode(model)
    val fast = ModelOptions.isFast(model, options)
    val rows = ModelOptions.rows(model)

    Row(
        Modifier.fillMaxWidth().padding(start = 20.dp, end = 14.dp, top = 14.dp, bottom = 4.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Column(Modifier.weight(1f)) {
            val title = if (shownEffort != null) reasoningLabel(shownEffort) else model?.label.orEmpty()
            // The reset button sits right after the title and takes no height, so it never moves anything.
            Row(verticalAlignment = Alignment.CenterVertically) {
                AnimatedContent(
                    targetState = title,
                    transitionSpec = { fadeIn(tween(120)) togetherWith fadeOut(tween(90)) },
                    label = "title",
                    modifier = Modifier.weight(1f, fill = false),
                ) { text ->
                    Text(text, style = MaterialTheme.typography.headlineSmallEmphasized.copy(lineHeight = 30.sp), maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                ResetButton(
                    visible = ModelOptions.differsFromDefaults(model, effort, options),
                    modifier = Modifier.padding(start = 10.dp),
                    onClick = onReset,
                )
            }
            Row(
                Modifier
                    .offset(y = (-4).dp)
                    .clip(RoundedCornerShape(12.dp))
                    .heightIn(min = 36.dp)
                    .clickable(role = Role.Button, onClickLabel = "Choose a model", onClick = onModels)
                    .padding(end = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    if (shownEffort != null) model?.label.orEmpty() else current?.harnessLabel.orEmpty(),
                    style = MaterialTheme.typography.titleSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false),
                )
                ZIcon(ZIcons.ChevronRight, null, Modifier.padding(start = 2.dp).size(16.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        if (fastMode != null) {
            Spacer(Modifier.width(12.dp))
            FastButton(fast) { on ->
                // A bolt cracks on; the charge drains off.
                if (on) {
                    feedback.haptic(Haptic.Lightning)
                    feedback.cue(Cue.FastOn)
                } else {
                    feedback.cue(Cue.FastOff)
                    feedback.haptic(Haptic.Tick)
                }
                ModelOptions.fastToggle(model, options)?.let { (id, choice) ->
                    model?.options?.firstOrNull { it.id == id }?.let { onOptions(ModelOptions.with(options, it, choice)) }
                }
            }
        }
    }
    if (levels.size > 1 && shownEffort != null) {
        EffortSlider(
            levels = levels.map { EffortLevel(it, reasoningLabel(it)) },
            selected = levels.indexOf(shownEffort).coerceAtLeast(0),
            onSelected = { onEffort(levels[it]) },
            fast = fast,
            modifier = Modifier.padding(horizontal = 20.dp, vertical = 4.dp),
        )
    }
    if (rows.isNotEmpty()) Spacer(Modifier.size(4.dp))
    for (option in rows) {
        OptionRow(option.label, ModelOptions.valueLabel(option, options)) { onChoices(option.id) }
    }
    Spacer(Modifier.size(10.dp))
}

/**
 * "Reset to defaults": an icon right of the effort name. It fades and scales in
 * only while something differs from the defaults. Its slot is measured with zero
 * height and centred on the title line, so its coming and going moves nothing.
 */
@Composable
private fun ResetButton(visible: Boolean, modifier: Modifier = Modifier, onClick: () -> Unit) {
    AnimatedVisibility(
        visible,
        modifier = modifier.layout { measurable, constraints ->
            val placeable = measurable.measure(constraints)
            layout(placeable.width, 0) { placeable.place(0, -placeable.height / 2) }
        },
        enter = fadeIn(MaterialTheme.motionScheme.fastEffectsSpec()) + scaleIn(MaterialTheme.motionScheme.fastSpatialSpec(), initialScale = 0.5f),
        exit = fadeOut(MaterialTheme.motionScheme.fastEffectsSpec()) + scaleOut(MaterialTheme.motionScheme.fastSpatialSpec(), targetScale = 0.5f),
    ) {
        Box(
            Modifier
                .size(38.dp)
                .clip(CircleShape)
                .background(MaterialTheme.colorScheme.surfaceContainerHighest.copy(alpha = 0.85f))
                .clickable(role = Role.Button, onClickLabel = "Reset to defaults", onClick = tapAction(onClick))
                .semantics { contentDescription = "Reset to defaults" },
            contentAlignment = Alignment.Center,
        ) {
            ZIcon(sh.zeron.android.R.drawable.ic_reset_defaults, null, Modifier.size(20.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

/** The square fast-mode toggle: a bolt that lights up and, on, gives off a slow sheen. */
@Composable
private fun FastButton(on: Boolean, modifier: Modifier = Modifier, onToggle: (Boolean) -> Unit) {
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val corner by animateDpAsState(if (pressed) 26.dp else 18.dp, MaterialTheme.motionScheme.fastSpatialSpec(), label = "fast corner")
    val container by animateColorAsState(
        if (on) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceContainerHighest,
        MaterialTheme.motionScheme.fastEffectsSpec(),
        label = "fast container",
    )
    val tint by animateColorAsState(
        if (on) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
        MaterialTheme.motionScheme.fastEffectsSpec(),
        label = "fast tint",
    )
    val power by animateFloatAsState(if (on) 1f else 0f, tween(300), label = "fast power")
    val reduceMotion = rememberReduceMotion()
    val phase = if (on && !reduceMotion) rememberPhase(2600).value else 0.5f
    val shape = RoundedCornerShape(corner)
    Box(
        modifier
            .size(52.dp)
            .clip(shape)
            .background(container)
            .border(1.dp, if (on) MaterialTheme.colorScheme.primary.copy(alpha = 0.45f) else MaterialTheme.colorScheme.outlineVariant, shape)
            .drawWithContent {
                drawContent()
                if (power > 0.01f) {
                    val band = size.width * 0.9f
                    val x = -band + (size.width + 2 * band) * phase
                    drawRect(
                        Brush.linearGradient(
                            listOf(Color.Transparent, Color.White.copy(alpha = 0.22f * power), Color.Transparent),
                            start = Offset(x - band / 2, 0f),
                            end = Offset(x + band / 2, size.height),
                        ),
                    )
                }
            }
            .toggleable(on, interaction, null, role = Role.Switch, onValueChange = onToggle)
            .semantics {
                contentDescription = "Fast mode"
                stateDescription = if (on) "On" else "Off"
            },
        contentAlignment = Alignment.Center,
    ) {
        ZIcon(ZIcons.FastTier, null, Modifier.size(24.dp), tint = tint)
    }
}

@Composable
private fun OptionRow(label: String, value: String, onClick: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 52.dp)
            .clickable(role = Role.Button, onClickLabel = "Change $label", onClick = onClick)
            .padding(start = 20.dp, end = 14.dp)
            .semantics(mergeDescendants = true) { stateDescription = value },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, style = MaterialTheme.typography.titleSmallEmphasized, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(value, style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
        ZIcon(ZIcons.ChevronRight, null, Modifier.padding(start = 6.dp).size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/** An option's choices: back, then a radio-style list with the current one checked. */
@Composable
private fun ColumnScope.OptionChoices(option: ModelOption, picks: Map<String, String>, onBack: () -> Unit, onPick: (String) -> Unit) {
    PageHeader(option.label, onBack)
    val current = picks[option.id] ?: option.defaultChoice
    Column(Modifier.padding(horizontal = 8.dp).padding(bottom = 10.dp)) {
        for (choice in option.choices) {
            val selected = choice.id == current
            val container by animateColorAsState(
                if (selected) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent,
                MaterialTheme.motionScheme.fastEffectsSpec(),
                label = "choice",
            )
            Row(
                Modifier
                    .fillMaxWidth()
                    .heightIn(min = 52.dp)
                    .clip(RoundedCornerShape(20.dp))
                    .background(container)
                    .semantics { this.selected = selected }
                    .clickable(role = Role.RadioButton) { onPick(choice.id) }
                    .padding(horizontal = 14.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(choice.label, style = MaterialTheme.typography.bodyLargeEmphasized, modifier = Modifier.weight(1f))
                if (selected) ZIcon(ZIcons.Check, "Selected", Modifier.size(20.dp), tint = MaterialTheme.colorScheme.primary)
            }
        }
    }
}

@Composable
private fun PageHeader(title: String, onBack: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(start = 6.dp, end = 20.dp, top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = onBack) {
            ZIcon(ZIcons.ChevronLeft, "Back", Modifier.size(22.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Text(title, style = MaterialTheme.typography.titleMediumEmphasized, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

// ---------------------------------------------------------------------------
// Model list
// ---------------------------------------------------------------------------

@Composable
private fun ColumnScope.ModelList(
    rail: RailHost,
    catalog: List<ModelChoice>,
    current: ModelChoice?,
    favorites: List<FavoriteModel>,
    locked: Boolean,
    statuses: List<CatalogStatus>,
    onBack: () -> Unit,
    onToggleFavorite: (FavoriteModel) -> Unit,
    onRetry: (String) -> Unit,
    onPick: (ModelChoice) -> Unit,
) {
    val feedback = LocalFeedback.current
    val density = LocalDensity.current
    val scope = rememberCoroutineScope()
    var query by remember { mutableStateOf("") }
    val entries = ModelPickerRules.entries(catalog, favorites, query, current, locked)
    val ambiguous = ModelPickerRules.ambiguousLabels(entries)
    // With two or more sections (Favorites, providers) and no search running the list gains section
    // headers and the provider rail; otherwise it is the plain list.
    val railShown = RailRules.visible(entries, query)
    val layout = remember(entries, railShown) { RailRules.layout(entries, railShown) }
    val list = rememberLazyListState(
        initialFirstVisibleItemIndex = remember {
            ModelPickerRules.initialScrollIndex(layout.items.indexOfFirst { it is RailRules.Item.Row && it.entry.choice.key == current?.key })
        },
    )
    // A starred row moves; follow it only if it leaves the screen.
    var follow by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(layout, follow) {
        val key = follow ?: return@LaunchedEffect
        // Let the list lay the reordered rows out before reading where they sit.
        androidx.compose.runtime.withFrameNanos { }
        androidx.compose.runtime.withFrameNanos { }
        val at = layout.itemOfKey(key)
        val visible = list.layoutInfo.visibleItemsInfo
        ModelPickerRules.followScroll(at, visible.firstOrNull()?.index ?: 0, visible.lastOrNull()?.index ?: 0)?.let { list.animateScrollToItem(it) }
        follow = null
    }

    // The rail: the section at the top of the list is lit; a tap or a finger dragged down it jumps to a section.
    // A jump pins the lit section until the finger touches the list itself.
    var pinned by remember(layout.sections.map { it.id }) { mutableStateOf<Int?>(null) }
    val atTop = remember(layout) { derivedStateOf { RailRules.current(layout, list.firstVisibleItemIndex, list.canScrollForward) } }
    val lit = remember(layout) { derivedStateOf { pinned ?: atTop.value } }
    LaunchedEffect(list) {
        list.interactionSource.interactions.collect { if (it is DragInteraction.Start) pinned = null }
    }
    // Scrolling the list across a section boundary ticks once.
    LaunchedEffect(layout, railShown) {
        var last = atTop.value
        snapshotFlow { atTop.value }.collect { now ->
            if (now != last) {
                last = now
                if (railShown && pinned == null) feedback.haptic(Haptic.RailTick)
            }
        }
    }
    val select = rememberUpdatedState { index: Int, tap: Boolean ->
        val section = layout.sections.getOrNull(index)
        if (section != null) {
            pinned = index
            // Each provider has its own sound; the touch itself is a firm select, moving onto the next a tick.
            feedback.haptic(if (tap) Haptic.Select else Haptic.RailTick)
            feedback.cue(Cue.forProvider(section.harness))
            scope.launch { if (tap) list.animateScrollToItem(section.item) else list.scrollToItem(section.item) }
        }
    }
    val railUi = remember(layout) {
        val ui: @Composable () -> Unit = {
            ProviderRail(
                sections = layout.sections,
                selected = lit,
                onSelect = { i, tap -> select.value(i, tap) },
                maxHeight = railMaxHeight(rail.height, density),
            )
        }
        ui
    }
    DisposableEffect(railShown, railUi) {
        rail.content = if (railShown) railUi else null
        onDispose { if (rail.content === railUi) rail.content = null }
    }
    val startPad by animateDpAsState(if (railShown) RailOverhang + 6.dp else 8.dp, MaterialTheme.motionScheme.defaultSpatialSpec(), label = "list start")

    // A new search starts from its best match.
    LaunchedEffect(query) { list.scrollToItem(0) }
    PageHeader("Models", onBack)
    SearchField(query, onChange = { query = it })
    HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.6f))
    val loading = statuses.any { it.error == null }
    Box(
        Modifier.weight(1f, fill = false)
            .onGloballyPositioned { rail.listPlaced(it) }
            .animateContentSize(MaterialTheme.motionScheme.defaultSpatialSpec())
            .heightIn(max = RowHeight * ModelPickerRules.VISIBLE_ROWS + 12.dp),
    ) {
        LazyColumn(
            state = list,
            contentPadding = PaddingValues(start = startPad, end = 8.dp, top = 6.dp, bottom = 6.dp),
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            if (entries.isEmpty() && loading && query.isBlank()) {
                items(4, key = { "skeleton$it" }) { SkeletonRow() }
            } else if (entries.isEmpty() && statuses.isEmpty()) {
                item("empty") {
                    EmptyNote(
                        if (query.isBlank()) "No models reported by this device yet." else "No models match",
                        if (query.isBlank()) null else "Try another name or provider.",
                    )
                }
            }
            items(layout.items, key = { it.key }) { item ->
                when (item) {
                    is RailRules.Item.Header -> SectionHeader(item.section.label, Modifier.animateItem())
                    is RailRules.Item.Row -> {
                        val entry = item.entry
                        ModelRow(
                            entry,
                            selected = entry.choice.key == current?.key,
                            detail = entry.choice.model.description?.trim()?.takeIf { it.isNotEmpty() && entry.choice.model.label in ambiguous },
                            onClick = { if (!entry.selectedOnly) onPick(entry.choice) },
                            onStar = {
                                feedback.both(Haptic.Pop, if (entry.starred) Cue.Unstar else Cue.Star)
                                follow = entry.key
                                onToggleFavorite(entry.choice.key)
                            },
                            modifier = Modifier.animateItem(),
                        )
                    }
                }
            }
            items(statuses.filter { entries.isNotEmpty() || it.error != null || query.isNotBlank() }, key = { "status/${it.harness}" }) { status ->
                StatusRow(status, onRetry = { feedback.both(Haptic.Tick, Cue.Refresh); onRetry(status.harness) }, modifier = Modifier.animateItem())
            }
        }
    }
}

/** A provider's (or Favorites') heading in the sectioned list. */
@Composable
private fun SectionHeader(label: String, modifier: Modifier = Modifier) {
    Text(
        label,
        style = MaterialTheme.typography.labelMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
        modifier = modifier.fillMaxWidth().padding(start = 12.dp, top = 8.dp, bottom = 2.dp),
    )
}

@Composable
private fun SearchField(query: String, onChange: (String) -> Unit) {
    val keyboard = LocalSoftwareKeyboardController.current
    val focus = LocalFocusManager.current
    Row(
        Modifier.fillMaxWidth().heightIn(min = 52.dp).padding(start = 20.dp, end = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        ZIcon(ZIcons.Search, null, Modifier.size(20.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Box(Modifier.weight(1f).padding(horizontal = 12.dp), contentAlignment = Alignment.CenterStart) {
            if (query.isEmpty()) {
                Text("Search models…", style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
            }
            BasicTextField(
                value = query,
                onValueChange = onChange,
                singleLine = true,
                textStyle = MaterialTheme.typography.bodyLarge.copy(color = MaterialTheme.colorScheme.onSurface),
                cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                keyboardActions = KeyboardActions(onSearch = { keyboard?.hide(); focus.clearFocus() }),
                modifier = Modifier.fillMaxWidth().semantics { contentDescription = "Search models" },
            )
        }
        AnimatedVisibility(query.isNotEmpty(), enter = fadeIn(), exit = fadeOut()) {
            IconButton(onClick = { onChange("") }) {
                ZIcon(ZIcons.Close, "Clear search", Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
}

@Composable
private fun ModelRow(
    entry: ModelPickerRules.Entry,
    selected: Boolean,
    detail: String?,
    onClick: () -> Unit,
    onStar: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val choice = entry.choice
    val container by animateColorAsState(
        if (selected) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent,
        MaterialTheme.motionScheme.fastEffectsSpec(),
        label = "row",
    )
    val content = if (selected) MaterialTheme.colorScheme.onSecondaryContainer else MaterialTheme.colorScheme.onSurface
    val muted = if (selected) MaterialTheme.colorScheme.onSecondaryContainer.copy(alpha = 0.75f) else MaterialTheme.colorScheme.onSurfaceVariant
    Row(
        modifier
            .fillMaxWidth()
            .heightIn(min = RowHeight)
            .clip(RoundedCornerShape(20.dp))
            .background(container)
            .semantics { this.selected = selected; contentDescription = "${choice.model.label}, ${choice.harnessLabel}" }
            .clickable(role = Role.RadioButton, onClick = onClick)
            .padding(start = 14.dp, end = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        HarnessMark(choice.harness, 18.dp, tint = content)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f).padding(vertical = 8.dp)) {
            Text(choice.model.label, style = MaterialTheme.typography.bodyLargeEmphasized, color = content, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (detail != null) Text(detail, style = MaterialTheme.typography.bodySmall, color = muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (selected) {
            Spacer(Modifier.width(8.dp))
            ZIcon(ZIcons.Check, "Selected", Modifier.size(20.dp), tint = MaterialTheme.colorScheme.primary)
        }
        IconButton(onClick = onStar) {
            ZIcon(
                if (entry.starred) ZIcons.StarFilled else ZIcons.Star,
                if (entry.starred) "Remove ${choice.model.label} from favorites" else "Add ${choice.model.label} to favorites",
                Modifier.size(20.dp),
                tint = if (entry.starred) MaterialTheme.colorScheme.primary else muted,
            )
        }
    }
}

@Composable
private fun StatusRow(status: CatalogStatus, onRetry: () -> Unit, modifier: Modifier = Modifier) {
    val failed = status.error != null
    Row(
        modifier.fillMaxWidth().heightIn(min = RowHeight).padding(start = 14.dp, end = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (status.harness.isNotEmpty()) {
            HarnessMark(status.harness, 18.dp, tint = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.width(12.dp))
        }
        Column(Modifier.weight(1f).padding(vertical = 8.dp)) {
            val name = status.label.ifEmpty { "models" }
            Text(
                if (failed) "$name unavailable" else "Loading $name…",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurface,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            status.error?.let {
                Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
        }
        if (failed) {
            TextButton(onClick = onRetry, modifier = Modifier.semantics { contentDescription = "Retry ${status.label} models" }) { Text("Retry") }
        } else {
            LoadingIndicator(Modifier.padding(end = 10.dp).size(28.dp))
        }
    }
}

@Composable
private fun SkeletonRow() {
    val reduceMotion = rememberReduceMotion()
    val phase = if (reduceMotion) 0.25f else rememberPhase(1400).value
    val base = MaterialTheme.colorScheme.onSurface
    val wave = 0.5f + 0.5f * kotlin.math.sin(phase * 2f * Math.PI.toFloat())
    val alpha = 0.06f + 0.05f * wave
    Row(Modifier.fillMaxWidth().heightIn(min = RowHeight).padding(horizontal = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(18.dp).clip(CircleShape).background(base.copy(alpha = alpha)))
        Spacer(Modifier.width(12.dp))
        Box(Modifier.height(14.dp).width(150.dp).clip(RoundedCornerShape(7.dp)).background(base.copy(alpha = alpha)))
    }
}

@Composable
private fun EmptyNote(title: String, hint: String?) {
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Text(title, style = MaterialTheme.typography.titleSmallEmphasized, textAlign = TextAlign.Center)
        hint?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center) }
    }
}

// ---------------------------------------------------------------------------
// The chip
// ---------------------------------------------------------------------------

/**
 * The composer's model chip: provider mark, model name and the dim effort
 * ("GPT-5.4  High"), a small bolt when fast mode is on. Tapping opens the
 * compact picker over it.
 */
@Composable
fun ModelPickerChip(
    catalog: List<ModelChoice>,
    current: ModelChoice?,
    harness: String,
    fallbackLabel: String,
    favorites: List<FavoriteModel>,
    onToggleFavorite: (FavoriteModel) -> Unit,
    onPick: (ModelChoice) -> Unit,
    effort: String?,
    onEffort: (String?) -> Unit,
    options: Map<String, String>,
    onOptions: (Map<String, String>) -> Unit,
    modifier: Modifier = Modifier,
    locked: Boolean = false,
    statuses: List<CatalogStatus> = emptyList(),
    onRetry: (String) -> Unit = {},
    onOpen: () -> Unit = {},
    usage: AccountUsageState? = null,
) {
    var open by remember { mutableStateOf(false) }
    var usageOpen by remember(usage, harness) { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val feedback = LocalFeedback.current
    val usedFraction = AccountUsage.fraction(usage?.snapshot, harness)
    val selectModel = tapAction { usageOpen = false; open = true; onOpen() }
    val model: ModelInfo? = current?.model
    val parts = ChipText.parts(
        model?.label ?: fallbackLabel,
        ModelOptions.effort(model, effort),
        ModelOptions.isFast(model, options),
        ::reasoningLabel,
    )
    Box(modifier) {
        Surface(
            shape = RoundedCornerShape(50),
            color = chipContainer(),
            contentColor = MaterialTheme.colorScheme.onSurface,
            modifier = Modifier.combinedClickable(
                role = Role.Button,
                onClickLabel = "Choose model",
                onLongClickLabel = if (usage != null) "Show usage" else null,
                hapticFeedbackEnabled = false,
                onLongClick = usage?.let { state -> {
                    feedback.haptic(Haptic.LongPress)
                    open = false
                    usageOpen = true
                    scope.launch { state.refresh(force = true) }
                } },
                onClick = selectModel,
            ).semantics {
                contentDescription = ChipText.describe(parts)
                stateDescription = listOfNotNull(
                    if (open || usageOpen) "Expanded" else "Collapsed",
                    usedFraction?.let(AccountUsage::percent),
                ).joinToString(", ")
            },
        ) {
            Row(
                Modifier.accountUsageFill(usedFraction).heightIn(min = 34.dp).padding(start = 10.dp, end = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                HarnessMark(harness, 16.dp)
                Text(parts.model, style = MaterialTheme.typography.labelLarge, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 168.dp))
                parts.effort?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.labelLarge.copy(fontWeight = FontWeight.Normal),
                        color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.8f),
                        maxLines = 1,
                    )
                }
                if (parts.fast) ZIcon(ZIcons.FastTier, null, Modifier.size(14.dp), tint = MaterialTheme.colorScheme.primary)
            }
        }
        ModelPickerPopover(
            expanded = open,
            onDismiss = { open = false },
            catalog = catalog,
            current = current,
            favorites = favorites,
            onToggleFavorite = onToggleFavorite,
            onPick = onPick,
            effort = effort,
            onEffort = onEffort,
            options = options,
            onOptions = onOptions,
            locked = locked,
            statuses = statuses,
            onRetry = onRetry,
        )
        usage?.let { AccountUsagePopover(usageOpen, { usageOpen = false }, it, harness) }
    }
}
