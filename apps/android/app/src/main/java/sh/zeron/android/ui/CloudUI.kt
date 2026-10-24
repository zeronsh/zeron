package sh.zeron.android.ui

import androidx.annotation.DrawableRes
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.LinearWavyProgressIndicator
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import sh.zeron.android.core.CloudStore
import sh.zeron.android.design.HarnessMark
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.UsageWindowInfo
import kotlin.math.roundToInt

/** Whose agent logins a page shows: Cloud's vault, or a computer's CLIs. */
sealed interface AccountsTarget {
    val harness: String
    val label: String

    data class Cloud(val agent: CloudStore.Agent) : AccountsTarget {
        override val harness get() = agent.harness
        override val label get() = agent.name
    }

    data class Device(val deviceId: String, val name: String, override val harness: String, override val label: String) : AccountsTarget

    /** As a route argument (`cloud:<harness>` · `device:<id>:<harness>`). */
    fun encode(): String = when (this) {
        is Cloud -> "cloud:${agent.harness}"
        is Device -> "device:$deviceId:$harness"
    }

    companion object {
        fun decode(value: String, deviceName: (String) -> String): AccountsTarget? {
            val parts = value.split(':')
            return when (parts.firstOrNull()) {
                "cloud" -> CloudStore.agents.firstOrNull { it.harness == parts.getOrNull(1) }?.let(::Cloud)
                "device" -> {
                    val id = parts.getOrNull(1) ?: return null
                    val harness = parts.getOrNull(2) ?: return null
                    Device(id, deviceName(id), harness, uniffi.zeron_core.agentLoginRules(harness).name.ifEmpty { harness })
                }
                else -> null
            }
        }
    }
}

/**
 * A Settings-style page (iOS Cloud pages): a round back button, the big
 * expressive title over a muted line, then grouped cards.
 */
@Composable
fun CloudPage(
    title: String,
    subtitle: String?,
    onBack: (() -> Unit)?,
    modifier: Modifier = Modifier,
    actions: @Composable androidx.compose.foundation.layout.RowScope.() -> Unit = {},
    content: LazyListScope.() -> Unit,
) {
    val list = rememberLazyListState()
    Box(modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        LazyColumn(
            state = list,
            contentPadding = PaddingValues(
                top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 8.dp,
                bottom = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding() + 32.dp,
            ),
            modifier = Modifier.fillMaxSize(),
        ) {
            if (onBack != null) {
                item("back") {
                    Box(Modifier.padding(start = 12.dp, top = 4.dp)) {
                        TonalCircleButton(ZIcons.Back, "Back", onClick = onBack, container = MaterialTheme.colorScheme.surfaceContainerHighest)
                    }
                }
            }
            item("header") { ScreenHeader(title, subtitle, actions = actions) }
            content()
        }
        StatusBarScrim(scrolled = list.firstVisibleItemIndex > 0 || list.firstVisibleItemScrollOffset > 0)
    }
}

/** A titled group of cards on a [CloudPage]. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
fun LazyListScope.cloudSection(title: String, footer: String? = null, key: String = title, content: @Composable ColumnScope.() -> Unit) {
    item(key) {
        Column {
            Text(
                title,
                style = MaterialTheme.typography.titleSmallEmphasized,
                color = MaterialTheme.colorScheme.primary,
                modifier = Modifier.padding(start = 28.dp, top = 24.dp, bottom = 8.dp),
            )
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap), content = content)
            footer?.let {
                Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(start = 28.dp, end = 28.dp, top = 8.dp))
            }
        }
    }
}

/** One grouped row: a tile, a title over a muted line, and a trailing accessory. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun CloudRow(
    index: Int,
    count: Int,
    title: String,
    detail: String? = null,
    leading: (@Composable () -> Unit)? = null,
    trailing: (@Composable () -> Unit)? = null,
    enabled: Boolean = true,
    titleColor: Color = Color.Unspecified,
    onClick: (() -> Unit)? = null,
) {
    SegmentedListItem(
        onClick = { onClick?.invoke() },
        enabled = enabled && onClick != null,
        shapes = segmentedShapes(index, count),
        colors = ListItemDefaults.segmentedColors(containerColor = cardColor(), disabledContainerColor = cardColor()),
        leadingContent = leading,
        supportingContent = detail?.let { { Text(it, maxLines = 3, overflow = TextOverflow.Ellipsis) } },
        trailingContent = trailing ?: if (onClick != null) {
            { Chevron() }
        } else {
            null
        },
    ) { Text(title, color = if (titleColor == Color.Unspecified) MaterialTheme.colorScheme.onSurface else titleColor, maxLines = 2, overflow = TextOverflow.Ellipsis) }
}

@Composable
fun Chevron() = ZIcon(ZIcons.ChevronRight, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.outline)

/** An agent's brand mark on a tile (Claude Code, Codex, …). */
@Composable
fun AgentTile(harness: String?, size: Dp = 40.dp) {
    Box(Modifier.size(size).clip(RoundedCornerShape(size * 0.35f)).background(chipContainer()), contentAlignment = Alignment.Center) {
        HarnessMark(harness, size * 0.55f, tint = MaterialTheme.colorScheme.onSurface)
    }
}

/** The filled capsule: the one way forward. */
@Composable
fun PrimaryCapsule(text: String, onClick: () -> Unit, modifier: Modifier = Modifier, @DrawableRes icon: Int? = null, enabled: Boolean = true, busy: Boolean = false) {
    Button(
        onClick = onClick,
        enabled = enabled && !busy,
        shape = CircleShape,
        contentPadding = PaddingValues(horizontal = 24.dp),
        modifier = modifier.fillMaxWidth().heightIn(min = 56.dp),
    ) { CapsuleContent(text, icon, busy) }
}

/** A quieter tonal capsule beside or under the primary one. */
@Composable
fun SecondaryCapsule(text: String, onClick: () -> Unit, modifier: Modifier = Modifier, @DrawableRes icon: Int? = null, enabled: Boolean = true, busy: Boolean = false) {
    FilledTonalButton(
        onClick = onClick,
        enabled = enabled && !busy,
        shape = CircleShape,
        contentPadding = PaddingValues(horizontal = 24.dp),
        modifier = modifier.fillMaxWidth().heightIn(min = 56.dp),
    ) { CapsuleContent(text, icon, busy) }
}

/** A text-only button ("Skip for now"). */
@Composable
fun PlainCapsule(text: String, onClick: () -> Unit, modifier: Modifier = Modifier) {
    TextButton(onClick = onClick, modifier = modifier.fillMaxWidth().heightIn(min = 48.dp)) {
        Text(text, style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun CapsuleContent(text: String, @DrawableRes icon: Int?, busy: Boolean) {
    if (busy) {
        LoadingIndicator(Modifier.size(28.dp), color = ButtonDefaults.buttonColors().contentColor)
        return
    }
    icon?.let {
        ZIcon(it, null, Modifier.size(20.dp))
        Spacer(Modifier.width(8.dp))
    }
    Text(text, style = MaterialTheme.typography.titleMedium)
}

/** One plan window's use ("Session · 42% · resets 4:10 PM"). */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun UsageBar(window: UsageWindowInfo, modifier: Modifier = Modifier) {
    val used = window.usedFraction.coerceIn(0f, 1f)
    val tone = when {
        used >= 0.9f -> MaterialTheme.colorScheme.error
        used >= 0.7f -> Color(0xFFD97706)
        else -> MaterialTheme.colorScheme.primary
    }
    Column(modifier.fillMaxWidth()) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(window.label, style = MaterialTheme.typography.labelLarge, maxLines = 1, softWrap = false)
            Spacer(Modifier.width(8.dp))
            Text(
                listOfNotNull("${(used * 100).roundToInt()}% used", window.resetsAtMs?.let { "resets ${resetLabel(it)}" }).joinToString(" · "),
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.End,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
        }
        Spacer(Modifier.height(6.dp))
        LinearWavyProgressIndicator(
            progress = { used },
            color = tone,
            trackColor = tone.copy(alpha = 0.18f),
            modifier = Modifier.fillMaxWidth(),
        )
    }
}

/** "4:10 PM" today, "Tue" this week, else "Mar 3". */
fun resetLabel(ms: Long, now: Long = System.currentTimeMillis()): String {
    val delta = ms - now
    val pattern = when {
        delta < 20 * 3_600_000L -> "h:mm a"
        delta < 6 * 86_400_000L -> "EEE"
        else -> "MMM d"
    }
    return java.text.SimpleDateFormat(pattern, java.util.Locale.getDefault()).format(java.util.Date(ms))
}

/** Cloud as a whole is in trouble: a tonal card on top of its pages. */
@Composable
fun ProblemCard(problem: CloudStore.Problem, onBilling: () -> Unit, modifier: Modifier = Modifier) {
    Surface(
        shape = RoundedCornerShape(24.dp),
        color = MaterialTheme.colorScheme.errorContainer,
        contentColor = MaterialTheme.colorScheme.onErrorContainer,
        modifier = modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
    ) {
        Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
            ZIcon(ZIcons.Warning, null, Modifier.size(24.dp))
            Spacer(Modifier.width(14.dp))
            Column(Modifier.weight(1f)) {
                Text(problem.title, style = MaterialTheme.typography.titleMedium)
                Text(problem.detail, style = MaterialTheme.typography.bodyMedium)
            }
            if (problem.billing) {
                TextButton(onClick = onBilling) { Text("Add credits", color = MaterialTheme.colorScheme.onErrorContainer) }
            }
        }
    }
}

/** A centred title over a muted paragraph: how onboarding steps and empty states open. */
@Composable
fun PageIntro(title: String, body: String, modifier: Modifier = Modifier, centered: Boolean = false) {
    Column(modifier, horizontalAlignment = if (centered) Alignment.CenterHorizontally else Alignment.Start) {
        Text(title, style = MaterialTheme.typography.headlineMediumEmphasized, textAlign = if (centered) TextAlign.Center else TextAlign.Start)
        Spacer(Modifier.height(10.dp))
        Text(body, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = if (centered) TextAlign.Center else TextAlign.Start)
    }
}

/** A small "Done"/"Connected" check label for a row's trailing edge. */
@Composable
fun DoneLabel(text: String) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        ZIcon(ZIcons.Check, null, Modifier.size(16.dp), tint = successColor())
        Spacer(Modifier.width(4.dp))
        Text(text, style = MaterialTheme.typography.labelLarge, color = successColor())
    }
}

/** Fill the page with a centred spinner (a first read in flight). */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
fun LazyListScope.loadingItem(key: String = "loading") {
    item(key) {
        Box(Modifier.fillMaxWidth().padding(48.dp), contentAlignment = Alignment.Center) { LoadingIndicator(Modifier.size(48.dp)) }
    }
}
