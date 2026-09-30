package sh.zeron.android.ui

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import sh.zeron.android.core.FavoriteModel
import sh.zeron.android.design.HarnessMark
import sh.zeron.android.design.LocalDarkTheme
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.ModelInfo

/** One model a device offers, with the harness (provider) it runs under. */
data class ModelChoice(val harness: String, val harnessLabel: String, val model: ModelInfo) {
    val key get() = FavoriteModel(harness, model.id)
}

/** What the picker's list shows: starred models across providers, or one provider's catalog. */
sealed interface ModelRail {
    data object Favorites : ModelRail
    data class Provider(val harness: String) : ModelRail
}

/** A provider on the rail: its harness id and name. */
data class PickerProvider(val harness: String, val label: String)

/**
 * The picker's pure rules (mirroring the desktop's pickers.rs where it has
 * one): which providers the rail shows, where it opens, which rows a view
 * lists and how many before "More models".
 */
object ModelPickerRules {
    /** Rows listed before the "More models" expander. */
    const val COLLAPSED_ROWS = 5

    /**
     * The rail's providers in catalog order — the catalog only holds harnesses
     * offered by the target device, so missing ones are simply absent. A
     * locked chat (an open session can't change harness) shows only its own.
     */
    fun providers(catalog: List<ModelChoice>, current: String?, locked: Boolean, labelFor: (String) -> String): List<PickerProvider> {
        val all = catalog.distinctBy { it.harness }.map { PickerProvider(it.harness, it.harnessLabel) }
        if (!locked) return all
        val own = current ?: return all
        return listOf(all.firstOrNull { it.harness == own } ?: PickerProvider(own, labelFor(own)))
    }

    /**
     * Where the rail opens: Favorites when the current model is starred (so
     * the selection is in view), else the current model's provider. Locked
     * chats stay on their own harness, as on the desktop.
     */
    fun defaultRail(favorites: List<FavoriteModel>, current: FavoriteModel?, locked: Boolean, providers: List<PickerProvider>): ModelRail {
        if (!locked && current != null && current in favorites) return ModelRail.Favorites
        val harness = current?.harness?.takeIf { h -> providers.any { it.harness == h } } ?: providers.firstOrNull()?.harness
        return if (harness != null) ModelRail.Provider(harness) else ModelRail.Favorites
    }

    /**
     * The view's rows. Favorites: every starred model the rail's providers
     * still offer, in starring order. A provider: its catalog in order — rows
     * never jump under a finger when starred — with the current model first
     * if the catalog doesn't list it (the desktop's "selected only" row).
     */
    fun rows(rail: ModelRail, catalog: List<ModelChoice>, favorites: List<FavoriteModel>, providers: List<PickerProvider>, current: ModelChoice?): List<ModelChoice> {
        val offered = providers.map { it.harness }.toSet()
        return when (rail) {
            ModelRail.Favorites -> favorites.mapNotNull { f ->
                if (f.harness !in offered) null else catalog.firstOrNull { it.harness == f.harness && it.model.id == f.model }
            }
            is ModelRail.Provider -> {
                val list = catalog.filter { it.harness == rail.harness }
                if (current != null && current.harness == rail.harness && list.none { it.model.id == current.model.id }) listOf(current) + list else list
            }
        }
    }

    /** The rows to show and how many are tucked behind "More models". */
    fun visible(rows: List<ModelChoice>, expanded: Boolean): Pair<List<ModelChoice>, Int> =
        if (expanded || rows.size <= COLLAPSED_ROWS) rows to 0 else rows.take(COLLAPSED_ROWS) to rows.size - COLLAPSED_ROWS

    /** Open expanded when the current model would otherwise hide behind "More models". */
    fun startsExpanded(rows: List<ModelChoice>, current: FavoriteModel?): Boolean =
        current != null && rows.indexOfFirst { it.key == current } >= COLLAPSED_ROWS
}

/** The desktop's warning amber (favorite stars, filling context). */
@Composable
fun warningColor(): Color = if (LocalDarkTheme.current) Color(0xFFFBBF24) else Color(0xFFB45309)

/**
 * The model picker: a wide popover over the chip. The current provider's
 * models scroll vertically (five, then "More models"); below them a rail of
 * provider marks — Favorites first — switches the list. Stars persist
 * device-locally in starring order.
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
    locked: Boolean = false,
    loading: Boolean = false,
    labelFor: (String) -> String = { it },
) {
    AnchoredPopover(expanded, onDismiss) {
        ModelPickerContent(catalog, current, favorites, onToggleFavorite, { onPick(it); onDismiss() }, locked, loading, labelFor)
    }
}

@Composable
private fun ColumnScope.ModelPickerContent(
    catalog: List<ModelChoice>,
    current: ModelChoice?,
    favorites: List<FavoriteModel>,
    onToggleFavorite: (FavoriteModel) -> Unit,
    onPick: (ModelChoice) -> Unit,
    locked: Boolean,
    loading: Boolean,
    labelFor: (String) -> String,
) {
    val providers = ModelPickerRules.providers(catalog, current?.harness, locked, labelFor)
    // Chosen once per opening; later stars don't yank the view away.
    var rail by remember { mutableStateOf(ModelPickerRules.defaultRail(favorites, current?.key, locked, providers)) }
    val rows = ModelPickerRules.rows(rail, catalog, favorites, providers, current)
    var expanded by remember(rail) { mutableStateOf(ModelPickerRules.startsExpanded(rows, current?.key)) }
    val (shown, hidden) = ModelPickerRules.visible(rows, expanded)
    val favoritesView = rail == ModelRail.Favorites
    val list = rememberLazyListState()
    LaunchedEffect(rail, rows.size) {
        val at = shown.indexOfFirst { it.key == current?.key }
        if (at > 0) list.scrollToItem(at)
    }

    val title = when (val r = rail) {
        ModelRail.Favorites -> "Favorites"
        is ModelRail.Provider -> providers.firstOrNull { it.harness == r.harness }?.label ?: labelFor(r.harness)
    }
    Row(Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, top = 16.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(title, style = MaterialTheme.typography.titleSmallEmphasized, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
        if (rows.isNotEmpty()) {
            Text(
                if (rows.size == 1) "1 model" else "${rows.size} models",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
    LazyColumn(
        state = list,
        contentPadding = PaddingValues(horizontal = 8.dp, vertical = 4.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
        modifier = Modifier.weight(1f, fill = false),
    ) {
        when {
            rows.isEmpty() && favoritesView -> item("empty") { FavoritesEmpty() }
            rows.isEmpty() && loading -> item("loading") {
                Box(Modifier.fillMaxWidth().padding(vertical = 24.dp), contentAlignment = Alignment.Center) { LoadingIndicator(Modifier.size(32.dp)) }
            }
            rows.isEmpty() -> item("none") { EmptyNote("No models reported by this device yet.") }
        }
        items(shown, key = { "${it.harness}/${it.model.id}" }) { choice ->
            ModelRow(
                choice,
                selected = choice.key == current?.key,
                starred = choice.key in favorites,
                showProvider = favoritesView,
                onClick = { onPick(choice) },
                onStar = { onToggleFavorite(choice.key) },
            )
        }
        if (hidden > 0 || (expanded && rows.size > ModelPickerRules.COLLAPSED_ROWS)) {
            item("more") { MoreRow(hidden, onClick = { expanded = !expanded }) }
        }
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.6f))
    ProviderRail(providers, rail, favoritesCount = favorites.count { f -> providers.any { it.harness == f.harness } }) { rail = it }
}

@Composable
private fun ModelRow(choice: ModelChoice, selected: Boolean, starred: Boolean, showProvider: Boolean, onClick: () -> Unit, onStar: () -> Unit) {
    val haptics = LocalHapticFeedback.current
    val container by animateColorAsState(
        if (selected) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent,
        MaterialTheme.motionScheme.fastEffectsSpec(),
        label = "row",
    )
    val content = if (selected) MaterialTheme.colorScheme.onSecondaryContainer else MaterialTheme.colorScheme.onSurface
    val muted = if (selected) MaterialTheme.colorScheme.onSecondaryContainer.copy(alpha = 0.75f) else MaterialTheme.colorScheme.onSurfaceVariant
    val subline = listOfNotNull(
        if (showProvider) choice.harnessLabel else null,
        choice.model.description?.trim()?.takeIf { it.isNotEmpty() && !it.equals(choice.harnessLabel, ignoreCase = true) },
    ).joinToString(" · ").ifEmpty { null }
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 52.dp)
            .clip(RoundedCornerShape(20.dp))
            .background(container)
            .semantics { this.selected = selected }
            .clickable(role = Role.RadioButton, onClick = onClick)
            .padding(start = 14.dp, end = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (showProvider) {
            HarnessMark(choice.harness, 18.dp, tint = content)
            Spacer(Modifier.width(12.dp))
        }
        Column(Modifier.weight(1f).padding(vertical = 8.dp)) {
            Text(choice.model.label, style = MaterialTheme.typography.bodyLargeEmphasized, color = content, maxLines = 1, overflow = TextOverflow.Ellipsis)
            subline?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = muted, maxLines = 1, overflow = TextOverflow.Ellipsis) }
        }
        if (selected) {
            Spacer(Modifier.width(8.dp))
            ZIcon(ZIcons.Check, "Selected", Modifier.size(20.dp), tint = content)
        }
        IconButton(onClick = {
            haptics.performHapticFeedback(HapticFeedbackType.ToggleOn)
            onStar()
        }) {
            ZIcon(
                if (starred) ZIcons.StarFilled else ZIcons.Star,
                if (starred) "Remove ${choice.model.label} from favorites" else "Add ${choice.model.label} to favorites",
                Modifier.size(20.dp),
                tint = if (starred) warningColor() else muted,
            )
        }
    }
}

@Composable
private fun MoreRow(hidden: Int, onClick: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(20.dp))
            .clickable(onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            if (hidden > 0) "More models ($hidden)" else "Fewer models",
            style = MaterialTheme.typography.labelLarge,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.weight(1f),
        )
        ZIcon(if (hidden > 0) ZIcons.ChevronDown else ZIcons.ChevronUp, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.primary)
    }
}

@Composable
private fun FavoritesEmpty() {
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 20.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Box(
            Modifier.size(44.dp).clip(RoundedCornerShape(16.dp)).background(MaterialTheme.colorScheme.surfaceContainerHighest),
            contentAlignment = Alignment.Center,
        ) { ZIcon(ZIcons.Star, null, Modifier.size(22.dp), tint = warningColor()) }
        Text("No favorites yet", style = MaterialTheme.typography.titleSmallEmphasized)
        Text(
            "Pick a provider below and tap the star next to a model to keep it here.",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
        )
    }
}

@Composable
private fun EmptyNote(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        textAlign = TextAlign.Center,
        modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 24.dp),
    )
}

/** Favorites, a hairline, then one brand mark per provider; the viewed one is a filled, squarer tile. */
@Composable
private fun ProviderRail(providers: List<PickerProvider>, rail: ModelRail, favoritesCount: Int, onSelect: (ModelRail) -> Unit) {
    val state = rememberLazyListState()
    LaunchedEffect(Unit) {
        val at = (rail as? ModelRail.Provider)?.let { r -> providers.indexOfFirst { it.harness == r.harness } } ?: -1
        // The viewed provider's tile (index at + 2, after favorites and the
        // hairline) scrolls into view with two neighbours before it.
        if (at >= 3) state.scrollToItem(at)
    }
    LazyRow(
        state = state,
        contentPadding = PaddingValues(horizontal = 10.dp, vertical = 10.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier.fillMaxWidth(),
    ) {
        item("favorites") {
            RailTile(
                selected = rail == ModelRail.Favorites,
                description = if (favoritesCount > 0) "Favorites, $favoritesCount starred" else "Favorites",
                onClick = { onSelect(ModelRail.Favorites) },
            ) { tint ->
                ZIcon(if (rail == ModelRail.Favorites) ZIcons.StarFilled else ZIcons.Star, null, Modifier.size(20.dp), tint = if (rail == ModelRail.Favorites) tint else warningColor())
            }
        }
        item("divider") {
            Box(Modifier.padding(horizontal = 2.dp).size(width = 1.dp, height = 24.dp).background(MaterialTheme.colorScheme.outlineVariant))
        }
        items(providers, key = { it.harness }) { p ->
            RailTile(selected = rail == ModelRail.Provider(p.harness), description = p.label, onClick = { onSelect(ModelRail.Provider(p.harness)) }) { tint ->
                HarnessMark(p.harness, 20.dp, tint = tint)
            }
        }
    }
}

@Composable
private fun RailTile(selected: Boolean, description: String, onClick: () -> Unit, content: @Composable (Color) -> Unit) {
    val corner by animateDpAsState(if (selected) 14.dp else 22.dp, MaterialTheme.motionScheme.fastSpatialSpec(), label = "corner")
    val container by animateColorAsState(
        if (selected) MaterialTheme.colorScheme.secondaryContainer else chipContainer(),
        MaterialTheme.motionScheme.fastEffectsSpec(),
        label = "tile",
    )
    val tint = if (selected) MaterialTheme.colorScheme.onSecondaryContainer else MaterialTheme.colorScheme.onSurfaceVariant
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Surface(
            onClick = onClick,
            shape = RoundedCornerShape(corner),
            color = container,
            modifier = Modifier.size(44.dp).semantics {
                contentDescription = description
                this.selected = selected
            },
        ) {
            Box(contentAlignment = Alignment.Center) { content(tint) }
        }
        Spacer(Modifier.height(4.dp))
        Box(
            Modifier
                .size(width = 16.dp, height = 3.dp)
                .clip(RoundedCornerShape(2.dp))
                .background(if (selected) MaterialTheme.colorScheme.primary else Color.Transparent),
        )
    }
}
