package sh.zeron.android.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import sh.zeron.android.transcript.Transcript
import sh.zeron.android.transcript.TranscriptActions
import sh.zeron.android.transcript.TranscriptState
import sh.zeron.android.transcript.elapsed
import uniffi.zeron_core.SubagentRow
import uniffi.zeron_core.SubagentState
import uniffi.zeron_core.modelLabel

/** Subagent labels shared by the companions sheet and the subagent page. */
object SubagentFormat {
    /** `claude-haiku-5` → "Haiku 5"; Claude's bare aliases capitalized. */
    fun model(id: String): String {
        val lower = id.lowercase()
        if (lower in listOf("haiku", "sonnet", "opus")) return lower.replaceFirstChar { it.uppercase() }
        val harness = if (lower.startsWith("claude")) "claude-code" else if (lower.startsWith("gpt")) "codex" else ""
        val label = runCatching { modelLabel(harness, id) }.getOrDefault(id)
        if (label != id || !lower.startsWith("claude-")) return label
        // An id the catalog doesn't know yet: `claude-haiku-5` → "Haiku 5".
        val words = lower.removePrefix("claude-").split('-')
        val family = words.firstOrNull()?.replaceFirstChar { it.uppercase() } ?: return label
        val version = words.drop(1).takeWhile { it.length <= 2 && it.all(Char::isDigit) }.joinToString(".")
        return if (version.isEmpty()) family else "$family $version"
    }

    /** "Haiku 5 · Explore". */
    fun detail(row: SubagentRow): String = listOfNotNull(row.model?.let(::model), row.kind).joinToString(" · ")

    fun elapsedSince(startedAtMs: Long, now: Long = System.currentTimeMillis()): String = elapsed(maxOf(0L, (now - startedAtMs) / 1000))
}

/**
 * What a subagent is doing (or did): its own transcript, read-only, live
 * while it runs (iOS `SubagentViewController`). A bar at the bottom says how
 * it's going — working and for how long, done, or failed — and that it runs
 * on its own. Subagents it spawns open the same way, one level in.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SubagentScreen(
    model: AppModel,
    parentId: String,
    docId: String,
    title: String,
    running: Boolean,
    onBack: () -> Unit,
    onOpenSubagent: (parentId: String, docId: String, title: String, running: Boolean) -> Unit,
) {
    val client by model.client.collectAsState()
    val core = client ?: return
    // The root chat runs it, and its spawn list says how it's doing.
    val rootChatId = remember(docId) { docId.substringBefore("--sub--").ifEmpty { parentId } }
    val transcript = remember(docId) { TranscriptState() }
    var unavailable by remember { mutableStateOf(false) }
    val density = LocalDensity.current.density
    DisposableEffect(docId) {
        if (!transcript.engine.attachSubagent(core, parentId, docId, !running)) unavailable = true
        model.row(rootChatId)?.deviceId?.let { transcript.setVisualSource(core, it, maxOf(2f, density)) }
        onDispose { transcript.close() }
    }
    val seenRows by remember { derivedStateOf { (transcript.frame?.rowCount() ?: 0u) > 0u } }
    // Nothing arrives (host offline, doc gone): say so instead of spinning.
    LaunchedEffect(docId) {
        delay(10_000)
        if (!seenRows) unavailable = true
    }
    var revision by remember { mutableIntStateOf(0) }
    LaunchedEffect(rootChatId) { model.sessionEvents.collect { if (it == rootChatId) revision++ } }
    val spawn = remember(revision) { model.sessionSubagents(rootChatId).firstOrNull { it.docId == docId } }
    val detail = spawn?.let(SubagentFormat::detail).orEmpty()
    val state = when {
        !seenRows && unavailable -> null
        spawn == null -> if (running) SubagentState.RUNNING else SubagentState.DONE
        else -> spawn.state
    }
    val clipboard = LocalClipboardManager.current

    val scrolled by remember { derivedStateOf { transcript.offset > 1f } }
    val headerColor by animateColorAsState(
        if (scrolled) MaterialTheme.colorScheme.surfaceContainerHigh else MaterialTheme.colorScheme.background,
        MaterialTheme.motionScheme.defaultEffectsSpec(),
        label = "header",
    )
    val actions = remember(docId) {
        TranscriptActions(
            openUrl = {},
            loadImage = { null },
            showText = { _, _, _ -> },
            openSubagent = { doc, t, r -> onOpenSubagent(docId, doc, t, r) },
        )
    }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Surface(color = headerColor) {
            Row(Modifier.fillMaxWidth().statusBarsPadding().padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                TonalCircleButton(ZIcons.Back, "Back", onClick = onBack, container = MaterialTheme.colorScheme.surfaceContainerHighest)
                Column(Modifier.weight(1f).padding(horizontal = 12.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.titleMediumEmphasized)
                    Text(listOf("Subagent", detail).filter { it.isNotEmpty() }.joinToString(" · "), maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                TonalCircleButton(ZIcons.Copy, "Copy transcript", onClick = {
                    transcript.frame?.let { clipboard.setText(AnnotatedString(it.plainText())) }
                    model.toast("Transcript copied")
                }, container = MaterialTheme.colorScheme.surfaceContainerHighest)
            }
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            val shown by animateFloatAsState(if (seenRows) 1f else 0f, MaterialTheme.motionScheme.defaultEffectsSpec(), label = "reveal")
            Transcript(transcript, actions, Modifier.fillMaxSize().alpha(shown))
            androidx.compose.animation.AnimatedVisibility(!seenRows && !unavailable, enter = fadeIn(), exit = fadeOut(), modifier = Modifier.align(Alignment.Center)) {
                LoadingIndicator(Modifier.size(48.dp))
            }
        }
        SubagentStatusBar(state, detail, spawn?.startedAtMs, Modifier.navigationBarsPadding().padding(horizontal = 12.dp, vertical = 8.dp))
    }
}

/** The subagent page's bottom bar: status glyph, what it's doing, how long. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun SubagentStatusBar(state: SubagentState?, detail: String, startedAtMs: Long?, modifier: Modifier = Modifier) {
    var now by remember { mutableStateOf(System.currentTimeMillis()) }
    if (state == SubagentState.RUNNING) {
        LaunchedEffect(Unit) {
            while (true) {
                now = System.currentTimeMillis()
                delay(1000)
            }
        }
    }
    val label = when (state) {
        SubagentState.RUNNING -> startedAtMs?.let { "Working · ${SubagentFormat.elapsedSince(it, now)}" } ?: "Working"
        SubagentState.DONE -> "Finished"
        SubagentState.FAILED -> "Failed"
        null -> "Waiting for its computer…"
    }
    val trailing = detail.ifEmpty { if (state == SubagentState.RUNNING) "Runs on its own" else "" }
    Surface(
        shape = RoundedCornerShape(28.dp),
        color = MaterialTheme.colorScheme.surfaceContainerHigh,
        modifier = modifier.fillMaxWidth().widthIn(max = 560.dp).semantics { contentDescription = listOf(label, trailing).filter { it.isNotEmpty() }.joinToString(", ") },
    ) {
        Row(Modifier.heightIn(min = 56.dp).padding(horizontal = 18.dp), verticalAlignment = Alignment.CenterVertically) {
            when (state) {
                SubagentState.RUNNING -> LoadingIndicator(Modifier.size(24.dp))
                SubagentState.DONE -> ZIcon(ZIcons.Check, null, Modifier.size(18.dp), tint = successColor())
                SubagentState.FAILED -> Box(Modifier.size(8.dp).clip(CircleShape).background(MaterialTheme.colorScheme.error))
                null -> Box(Modifier.size(8.dp).clip(CircleShape).background(MaterialTheme.colorScheme.outline))
            }
            Spacer(Modifier.width(12.dp))
            Text(label, style = MaterialTheme.typography.labelLargeEmphasized, maxLines = 1, modifier = Modifier.weight(1f))
            if (trailing.isNotEmpty()) Text(trailing, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}
