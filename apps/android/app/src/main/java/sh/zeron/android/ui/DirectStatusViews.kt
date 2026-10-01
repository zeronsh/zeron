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
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.clickable
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.LocalZeronColors
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronType
import uniffi.zeron_core.DirectPhase
import uniffi.zeron_core.DirectStatus

/** One-line state of the active machine's link, for list subtitles. */
internal fun directSummary(status: DirectStatus?): String = when {
    status == null -> "connecting…"
    status.phase == DirectPhase.LIVE -> "connected" + (status.engineVersion?.let { " · Zeron $it" } ?: "")
    status.phase == DirectPhase.SYNCING -> "connected · loading sessions…"
    status.phase == DirectPhase.FAILED -> status.lastError ?: "not connected"
    else -> "connecting…"
}

private fun retryLabel(status: DirectStatus): String? {
    val at = status.retryAtMs ?: return null
    val secs = ((at - System.currentTimeMillis()) / 1000).coerceAtLeast(0)
    return if (secs > 0) "Retrying in ${secs}s" else "Retrying…"
}

/**
 * The direct link's state above the sessions list: never a silent blank
 * page. Hidden once the workspace is live and every row parsed.
 */
@Composable
internal fun DirectBanner(model: ZeronModel, colors: ZeronColors, modifier: Modifier = Modifier) {
    if (model.client?.isDirect() != true) return
    val status = model.directStatus
    val machine = model.activeTitle()
    val skipped = status?.streams?.sumOf { it.skippedRows.toInt() } ?: 0
    val notice = status?.notice?.takeIf { status.phase == DirectPhase.LIVE && !model.noticeDismissed(it) }
    if (notice != null && skipped == 0) {
        // Newer engine: informational only, never blocks anything.
        Row(
            modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp).clip(RoundedCornerShape(14.dp)).background(colors.elevated).padding(horizontal = 14.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(notice, color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp, modifier = Modifier.weight(1f))
            Spacer(Modifier.width(8.dp))
            Pill(colors, "OK") { model.dismissNotice(notice) }
        }
        return
    }
    val (title, detail, danger) = when {
        status == null || status.phase == DirectPhase.CONNECTING ->
            Triple("Connecting to $machine…", status?.log?.lastOrNull()?.message, false)
        status.phase == DirectPhase.SYNCING ->
            Triple("Connected to $machine · loading sessions…", status.lastError ?: status.log.lastOrNull()?.message, status.lastError != null)
        status.phase == DirectPhase.FAILED ->
            Triple("Can't load $machine", listOfNotNull(status.lastError, retryLabel(status)).joinToString("\n"), true)
        skipped > 0 -> Triple("Some sessions couldn't be read", "$skipped row(s) from Zeron ${status.engineVersion ?: ""} were skipped. Tap Details.", false)
        else -> return
    }
    Column(
        modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp).clip(RoundedCornerShape(14.dp)).background(colors.elevated).padding(14.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(8.dp).clip(CircleShape).background(if (danger) colors.danger else colors.tertiary))
            Spacer(Modifier.width(8.dp))
            Text(title, color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 15.sp)
        }
        if (!detail.isNullOrBlank()) {
            Spacer(Modifier.height(4.dp))
            Text(detail, color = if (danger) colors.danger else colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp)
        }
        Spacer(Modifier.height(10.dp))
        Row {
            Pill(colors, "Retry now") { model.retryDirect() }
            Spacer(Modifier.width(8.dp))
            Pill(colors, "Details") { model.showLinkDetails = true }
            Spacer(Modifier.width(8.dp))
            Pill(colors, "Machines") { model.showMachines = true }
        }
    }
}

/** Text for the sessions page when there are no rows to show. */
internal fun emptySessionsText(model: ZeronModel): String {
    if (model.client?.isDirect() != true) return "No sessions yet"
    val status = model.directStatus
    return when (status?.phase) {
        DirectPhase.LIVE -> "No sessions on ${model.activeTitle()} yet"
        DirectPhase.FAILED -> "Not connected"
        else -> "Loading sessions from ${model.activeTitle()}…"
    }
}

/** Connection details: phase, engine, per-stream counters and the link log. */
@Composable
internal fun LinkDetailsScreen(model: ZeronModel) {
    val colors = LocalZeronColors.current
    val clipboard = LocalClipboardManager.current
    val status = model.directStatus
    val fmt = remember { java.text.SimpleDateFormat("HH:mm:ss", java.util.Locale.getDefault()) }
    Column(Modifier.fillMaxSize().background(colors.background).statusBarsPadding().navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("Close", color = colors.text, fontFamily = ZeronType.Sans, fontSize = 16.sp, modifier = Modifier.clip(RoundedCornerShape(12.dp)).clickable { model.showLinkDetails = false }.padding(8.dp))
            Text("Connection Details", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp, modifier = Modifier.weight(1f), textAlign = androidx.compose.ui.text.style.TextAlign.Center)
            Text("Copy", color = colors.accent, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 16.sp, modifier = Modifier.clip(RoundedCornerShape(12.dp)).clickable {
                clipboard.setText(AnnotatedString(model.linkReport()))
                model.showToast("Copied")
            }.padding(8.dp))
        }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp)) {
            if (status == null) {
                GroupLabel(colors, "Not connected to a machine")
                return@Column
            }
            GroupLabel(colors, model.activeTitle())
            val state = when (status.phase) {
                DirectPhase.LIVE -> "Live · synced"
                DirectPhase.SYNCING -> "Tunnel open · waiting for the workspace"
                DirectPhase.CONNECTING -> "Connecting"
                DirectPhase.FAILED -> listOfNotNull("Not connected", retryLabel(status)).joinToString(" · ")
            }
            SettingRow(colors, "State", state)
            SettingRow(colors, "Zeron engine", listOfNotNull(status.engineVersion, status.engineDeviceId?.let { "device ${it.take(8)}" }).joinToString(" · ").ifEmpty { "not reached yet" })
            status.lastError?.let { SettingRow(colors, "Last error", it) }
            status.notice?.let { SettingRow(colors, "Note", it) }
            GroupLabel(colors, "Streams")
            for (st in status.streams) {
                val line = buildString {
                    append("${st.frames} frames · ${st.rows} rows")
                    if (st.skippedRows > 0u) append(" · ${st.skippedRows} skipped")
                    if (st.repairedRows > 0u) append(" · ${st.repairedRows} read with unknown values ignored")
                    st.lastFrameMs?.let { append(" · last ${fmt.format(java.util.Date(it))}") }
                    st.error?.let { append("\n$it") }
                }
                SettingRow(colors, st.name, line)
            }
            val ws = model.workspace
            SettingRow(colors, "On this phone", "${ws?.projects?.size ?: 0} projects · ${ws?.front?.recent?.size ?: 0} recent · ${ws?.archived?.size ?: 0} archived")
            GroupLabel(colors, "Log")
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(colors.elevated).padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                for (line in status.log.asReversed()) {
                    Text("${fmt.format(java.util.Date(line.atMs))}  ${line.message}", color = colors.text, fontFamily = ZeronType.Mono, fontSize = 11.sp)
                }
            }
            Spacer(Modifier.height(12.dp))
            Row {
                Pill(colors, "Retry now", primary = true) { model.retryDirect() }
            }
            Spacer(Modifier.height(40.dp))
        }
    }
}
