package sh.zeron.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.ZeronTheme
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.SessionRow

/** Every count the badge has a different look for, plus a hidden zero and the overflow. */
val BadgePreviewCounts: List<UInt> = listOf(1u, 2u, 3u, 4u, 5u, 6u, 7u, 8u, 9u, 10u, 11u, 12u, 20u, 21u, 67u, 99u, 100u, UInt.MAX_VALUE, 0u)

private fun captionOf(count: UInt) = when (count) {
    0u -> "0"
    UInt.MAX_VALUE -> "max"
    else -> "$count"
}

private fun previewRow(id: String, title: String, running: UInt, indicator: ChatIndicator = ChatIndicator.IDLE) = SessionRow(
    id = id, revision = 0uL, title = title, hasTitle = true, preview = null, project = null,
    deviceId = "host", deviceName = "Preview", deviceOnline = true, harness = "claude-code", harnessLabel = "Claude Code",
    model = null, modelLabel = null, reasoning = null, branch = "main", cwd = null,
    indicator = indicator, hostIndicator = indicator, workingSinceMs = null, lastActivityMs = 0, timeLabel = "now",
    createdAtMs = 0, unseen = false, archived = false, pinned = false, sectionId = null, pullRequest = null,
    sendState = null, parentChatId = null, roomGen = 2u, runningSubagents = running, pendingCallbacks = 0u,
)

/**
 * Debug builds (`--es route badges` / `badges:rows`): the badge on the real harness tile for every count, then the
 * same tiles in real session rows and the chat header's Subagents button. This is what the contact sheets in
 * docs/media/android are screenshotted from, and what their alignment is measured on.
 */
@Composable
fun BadgePreviewScreen(model: AppModel, mode: String, onBack: () -> Unit) {
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).statusBarsPadding().verticalScroll(rememberScrollState())) {
        Text("Running subagents", style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(start = 16.dp, top = 12.dp))
        when (mode) {
            "rows" -> RowsPreview(model)
            else -> GridPreview()
        }
    }
}

@Composable
private fun GridPreview() {
    Column(Modifier.padding(start = 20.dp, end = 12.dp, top = 12.dp, bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(20.dp)) {
        BadgePreviewCounts.chunked(5).forEach { counts ->
            Row(Modifier.fillMaxWidth()) {
                counts.forEach { count ->
                    Column(Modifier.weight(1f), horizontalAlignment = Alignment.Start, verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Box(Modifier.padding(top = 10.dp)) { HarnessActivityTile("claude-code", Color(0xFFAB6092), count) }
                        Text(captionOf(count), style = MaterialTheme.typography.labelMedium)
                    }
                }
                repeat(5 - counts.size) { Spacer(Modifier.weight(1f)) }
            }
        }
    }
}

@Composable
private fun RowsPreview(model: AppModel) {
    val counts = listOf(0u, 1u, 2u, 3u, 99u, 100u, UInt.MAX_VALUE)
    Column(Modifier.padding(top = 12.dp, bottom = 24.dp)) {
        counts.forEachIndexed { i, count ->
            Box(Modifier.padding(horizontal = 16.dp).padding(bottom = 2.dp)) {
                SessionItem(
                    previewRow("preview-$i", "Subagents: ${captionOf(count)}", count, if (i % 3 == 1) ChatIndicator.WORKING else ChatIndicator.IDLE),
                    i, counts.size, model, onOpen = {}, archive = {},
                )
            }
        }
        Spacer(Modifier.height(20.dp))
        Row(Modifier.padding(horizontal = 20.dp), horizontalArrangement = Arrangement.spacedBy(16.dp)) {
            listOf(0, 1, 3, 12, 99, 150).forEach { SubagentsButton(it, onClick = {}) }
        }
    }
}

@Preview(showBackground = true)
@Composable
fun ActivityBadgePreview() {
    ZeronTheme { Box(Modifier.background(MaterialTheme.colorScheme.background)) { GridPreview() } }
}
