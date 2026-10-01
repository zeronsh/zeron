package sh.zeron.android.ui

import sh.zeron.android.feedback.OpenCloseFeedback
import android.content.Intent
import android.graphics.BitmapFactory
import android.net.Uri
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.browser.customtabs.CustomTabsIntent
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.Geist
import sh.zeron.android.design.ZIcons
import sh.zeron.android.tools.Links
import sh.zeron.android.transcript.Transcript
import sh.zeron.android.transcript.TranscriptActions
import sh.zeron.android.transcript.TranscriptState
import uniffi.zeron_core.SessionHandle
import uniffi.zeron_core.SubagentState
import uniffi.zeron_core.SubagentView

/** Links in a transcript that name a subagent (spawn cards): `zeron-subagent:{doc}`. */
fun subagentDocOf(url: String): String? =
    url.removePrefix(SUBAGENT_LINK).takeIf { url.startsWith(SUBAGENT_LINK) && it.contains("--sub--") }

const val SUBAGENT_LINK = "zeron-subagent:"

/**
 * One subagent of [chatId], read-only: its own transcript (live while it
 * runs, the settled log after), under a card with what the spawn chip says —
 * type, model, age, and its report to the parent. There is no composer: a
 * subagent is steered through its parent chat.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SubagentScreen(model: AppModel, chatId: String, docId: String, onBack: () -> Unit, onNavigate: (String) -> Unit = {}) {
    val client by model.client.collectAsState()
    val core = client ?: return
    // The parent's transcript holds the spawn chip: keep it open for the card.
    val parent: SessionHandle? = remember(chatId) { runCatching { core.openSession(chatId) }.getOrNull() }
    val handle: SessionHandle? = remember(chatId, docId) { runCatching { core.openSubagent(chatId, docId) }.getOrNull() }
    val transcript = remember(docId) { TranscriptState() }
    DisposableEffect(docId) {
        if (handle != null) {
            transcript.engine.attach(core, docId)
            handle.setViewAttached(true)
        }
        onDispose {
            handle?.setViewAttached(false)
            transcript.close()
        }
    }
    var groups by remember(chatId) { mutableStateOf(groupsOf(core, chatId)) }
    LaunchedEffect(chatId) {
        model.sessionEvents.collect { if (it == chatId || it == docId) groups = groupsOf(core, chatId) }
    }
    val workspace by model.workspace.collectAsState()
    val row = remember(workspace, chatId) { model.row(chatId) }
    val view = remember(groups, docId) { Subagents.find(groups, docId) }
    val now = rememberNow(view?.state == SubagentState.RUNNING)

    val context = LocalContext.current
    var report by remember { mutableStateOf(false) }
    val actions = remember(chatId, docId) {
        TranscriptActions(
            openUrl = { url ->
                val uri = Uri.parse(url)
                val nested = subagentDocOf(url)
                if (nested != null) onNavigate(Routes.subagent(chatId, nested))
                else when (val target = Links.classify(url, model.workspaceRef(chatId))) {
                    is Links.Target.Web -> onNavigate(Routes.browser(chatId, target.url))
                    is Links.Target.File -> onNavigate(Routes.file(chatId, target.path))
                    is Links.Target.Outside -> sh.zeron.android.tools.toast(context, "${target.path} is outside this session's folder")
                    Links.Target.Other -> runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, uri)) }
                        .onFailure { runCatching { CustomTabsIntent.Builder().build().launchUrl(context, uri) } }
                }
            },
            openFile = { path ->
                when (val target = Links.classify(path, model.workspaceRef(chatId))) {
                    is Links.Target.File -> onNavigate(Routes.file(chatId, target.path))
                    else -> Unit
                }
            },
            loadImage = { ref ->
                runCatching {
                    val device = parent?.composer()?.host?.deviceId ?: model.row(chatId)?.deviceId ?: error("no host")
                    val bytes = core.readAttachment(device, ref)
                    withContext(Dispatchers.Default) { BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap() }
                }.getOrNull()
            },
            showText = { _, _, _ -> },
        )
    }

    val scrolled by remember { derivedStateOf { transcript.offset > 1f } }
    val headerColor by animateColorAsState(
        if (scrolled) MaterialTheme.colorScheme.surfaceContainerHigh else MaterialTheme.colorScheme.background,
        MaterialTheme.motionScheme.defaultEffectsSpec(),
        label = "header",
    )
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Surface(color = headerColor) {
            Row(
                Modifier.fillMaxWidth().statusBarsPadding().padding(horizontal = 12.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                TonalCircleButton(ZIcons.Back, "Back", onClick = onBack, container = MaterialTheme.colorScheme.surfaceContainerHighest)
                Column(Modifier.weight(1f).padding(horizontal = 12.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Text(view?.title ?: "Subagent", maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.titleMediumEmphasized)
                    Text(
                        "Subagent of ${row?.title ?: "this chat"}",
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                // Keeps the title centred against the back button.
                Spacer(Modifier.size(48.dp))
            }
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            if (handle != null) {
                Transcript(transcript, actions, Modifier.fillMaxSize())
                val empty by remember { derivedStateOf { (transcript.frame?.rowCount() ?: 0u) == 0u } }
                if (empty) Placeholder("Loading the subagent's transcript…")
            } else {
                Placeholder("This subagent's transcript can't be opened here. Its spawn card is below.")
            }
        }
        SpawnCard(view, row?.harnessLabel, now, onReport = { report = true })
    }

    if (report && view?.summary != null) {
        ModalBottomSheet(onDismissRequest = { report = false }) {
            OpenCloseFeedback()
            Text("Report to the parent", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(horizontal = 24.dp))
            SelectionContainer {
                Text(
                    view.summary.orEmpty(),
                    fontFamily = Geist,
                    style = MaterialTheme.typography.bodyLarge,
                    modifier = Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(24.dp),
                )
            }
        }
    }
}

@Composable
private fun Placeholder(text: String) {
    Box(Modifier.fillMaxSize().padding(32.dp), contentAlignment = Alignment.Center) {
        Text(text, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/** What the parent's spawn chip records about this subagent. */
@Composable
private fun SpawnCard(view: SubagentView?, harness: String?, now: Long, onReport: () -> Unit) {
    Surface(
        color = composerContainer(),
        shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(Modifier.navigationBarsPadding().padding(horizontal = 20.dp, vertical = 16.dp).animateContentSize()) {
            if (view == null) {
                Text("Waiting for the parent chat's spawn card…", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                return@Column
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                SubagentGlyph(view.state)
                Spacer(Modifier.width(10.dp))
                Text(
                    Subagents.stateLabel(view.state),
                    style = MaterialTheme.typography.titleSmallEmphasized,
                    color = when (view.state) {
                        SubagentState.FAILED -> MaterialTheme.colorScheme.error
                        SubagentState.COMPLETED -> successColor()
                        else -> activityColor()
                    },
                )
                Spacer(Modifier.width(8.dp))
                Text(
                    listOfNotNull(view.agentType, view.model, harness).joinToString(" · "),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
            }
            Spacer(Modifier.height(4.dp))
            val timing = when (view.state) {
                SubagentState.RUNNING -> "Started ${Subagents.agoLabel(now - view.startedAtMs)} · running ${Subagents.durationLabel(now - view.startedAtMs)}"
                SubagentState.PENDING -> "Started ${Subagents.agoLabel(now - view.startedAtMs)}"
                else -> "Started ${Subagents.agoLabel(now - view.startedAtMs)} · last update ${Subagents.agoLabel(now - view.updatedAtMs)}"
            }
            Text(timing, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            view.summary?.let { summary ->
                Spacer(Modifier.height(8.dp))
                Text(
                    summary,
                    style = MaterialTheme.typography.bodyMedium,
                    maxLines = 3,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.clickable(onClickLabel = "Show the full report", onClick = onReport),
                )
            }
            Spacer(Modifier.height(6.dp))
            Text(
                "Read-only — steer it from the parent chat.",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.outline,
            )
        }
    }
}
