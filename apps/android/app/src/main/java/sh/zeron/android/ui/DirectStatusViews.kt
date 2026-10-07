package sh.zeron.android.ui

import sh.zeron.android.design.BackButton
import sh.zeron.android.design.consumeBlankTaps
import sh.zeron.android.R
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
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
@Composable
internal fun directSummary(status: DirectStatus?): String = when {
    status == null -> stringResource(R.string.direct_connecting)
    status.phase == DirectPhase.LIVE -> status.engineVersion?.let { stringResource(R.string.direct_connected_version, it) } ?: stringResource(R.string.direct_connected)
    status.phase == DirectPhase.SYNCING -> stringResource(R.string.direct_syncing)
    status.phase == DirectPhase.FAILED -> status.lastError ?: stringResource(R.string.direct_not_connected)
    else -> stringResource(R.string.direct_connecting)
}

@Composable
private fun retryLabel(status: DirectStatus): String? {
    val at = status.retryAtMs ?: return null
    val secs = ((at - System.currentTimeMillis()) / 1000).coerceAtLeast(0)
    return if (secs > 0) stringResource(R.string.retrying_in, secs.toInt()) else stringResource(R.string.retrying)
}

/**
 * The direct link's state above the sessions list: never a silent blank
 * page. Hidden once the workspace is live and every row parsed.
 */
@Composable
internal fun DirectBanner(model: ZeronModel, colors: ZeronColors, modifier: Modifier = Modifier) {
    if (model.client?.isDirect() != true) return
    val status = model.directStatus
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
            Pill(colors, stringResource(R.string.ok)) { model.dismissNotice(notice) }
        }
        return
    }
    // Connecting / syncing / failed live in the title-bar chip and the
    // failure sheet now; only the engine's skipped-rows warning stays here.
    if (status?.phase != DirectPhase.LIVE || skipped == 0) return
    val title = stringResource(R.string.banner_skipped_title)
    val detail = pluralStringResource(R.plurals.banner_skipped_detail, skipped, skipped, status.engineVersion ?: "")
    val danger = false
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
            Pill(colors, stringResource(R.string.retry_now)) { model.retryDirect() }
            Spacer(Modifier.width(8.dp))
            Pill(colors, stringResource(R.string.details)) { model.showLinkDetails = true }
            Spacer(Modifier.width(8.dp))
            Pill(colors, stringResource(R.string.computers)) { model.showMachines = true }
        }
    }
}

/** Text for the sessions page when there are no rows to show. */
@Composable
internal fun emptySessionsText(model: ZeronModel): String {
    if (model.client?.isDirect() != true) return stringResource(R.string.no_sessions_yet)
    val status = model.directStatus
    return when (status?.phase) {
        DirectPhase.LIVE -> stringResource(R.string.no_sessions_on, model.activeTitle())
        DirectPhase.FAILED -> stringResource(R.string.not_connected)
        else -> stringResource(R.string.loading_sessions_from, model.activeTitle())
    }
}

/** Connection details: phase, engine, per-stream counters and the link log. */
@Composable
internal fun LinkDetailsScreen(model: ZeronModel) {
    val colors = LocalZeronColors.current
    val clipboard = LocalClipboardManager.current
    val context = LocalContext.current
    val status = model.directStatus
    val fmt = remember { java.text.SimpleDateFormat("HH:mm:ss", java.util.Locale.getDefault()) }
    Column(Modifier.fillMaxSize().background(colors.background).consumeBlankTaps().statusBarsPadding().navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            BackButton(colors, onClick = { model.showLinkDetails = false })
            Text(stringResource(R.string.connection_details), color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp, modifier = Modifier.weight(1f), textAlign = androidx.compose.ui.text.style.TextAlign.Center)
            Text(stringResource(R.string.copy), color = colors.accent, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 16.sp, modifier = Modifier.clip(RoundedCornerShape(12.dp)).clickable {
                clipboard.setText(AnnotatedString(model.linkReport()))
                model.showToast(context.getString(R.string.copied))
            }.padding(8.dp))
        }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp)) {
            if (status == null) {
                GroupLabel(colors, stringResource(R.string.not_connected_machine))
                return@Column
            }
            GroupLabel(colors, model.activeTitle())
            // Down: why, in terms of this phone's network, and the fix.
            val view = model.connectionView()
            if (view.dot == sh.zeron.android.core.ConnectionState.Dot.FAILED && view.diagnosis != null) {
                Column(Modifier.fillMaxWidth().padding(bottom = 8.dp).clip(RoundedCornerShape(14.dp)).background(colors.elevated).padding(14.dp).testTag("details-diagnosis")) {
                    FailureReason(colors, view)
                    if (view.diagnosis.opensTailscale || view.diagnosis.installsTailscale) {
                        Spacer(Modifier.height(12.dp))
                        TailscaleAction(model, colors, view.diagnosis)
                    }
                }
            }
            val state = when (status.phase) {
                DirectPhase.LIVE -> stringResource(R.string.link_live)
                DirectPhase.SYNCING -> stringResource(R.string.link_syncing)
                DirectPhase.CONNECTING -> stringResource(R.string.link_connecting)
                DirectPhase.FAILED -> listOfNotNull(stringResource(R.string.not_connected), retryLabel(status)).joinToString(" · ")
            }
            SettingRow(colors, stringResource(R.string.link_state), state)
            SettingRow(colors, stringResource(R.string.link_engine), listOfNotNull(status.engineVersion, status.engineDeviceId?.let { stringResource(R.string.link_device, it.take(8)) }).joinToString(" · ").ifEmpty { stringResource(R.string.link_not_reached) })
            status.lastError?.let { SettingRow(colors, stringResource(R.string.link_last_error), it) }
            status.notice?.let { SettingRow(colors, stringResource(R.string.link_note), it) }
            status.clockOffsetMs?.let { SettingRow(colors, stringResource(R.string.link_clock), clockOffsetText(it)) }
            RoutesSection(model, colors, status)
            GroupLabel(colors, stringResource(R.string.link_streams))
            val res = context.resources
            for (st in status.streams) {
                val line = buildString {
                    append(res.getQuantityString(R.plurals.stream_frames, st.frames.toInt(), st.frames.toInt()))
                    append(" · ")
                    append(res.getQuantityString(R.plurals.stream_rows, st.rows.toInt(), st.rows.toInt()))
                    if (st.skippedRows > 0u) append(" · " + res.getQuantityString(R.plurals.stream_skipped, st.skippedRows.toInt(), st.skippedRows.toInt()))
                    if (st.repairedRows > 0u) append(" · " + res.getQuantityString(R.plurals.stream_repaired, st.repairedRows.toInt(), st.repairedRows.toInt()))
                    st.lastFrameMs?.let { append(" · " + res.getString(R.string.stream_last, fmt.format(java.util.Date(it)))) }
                    st.error?.let { append("\n$it") }
                }
                SettingRow(colors, st.name, line)
            }
            val ws = model.workspace
            val nProjects = ws?.projects?.size ?: 0
            val nRecent = ws?.front?.recent?.size ?: 0
            val nArchived = ws?.archived?.size ?: 0
            SettingRow(
                colors,
                stringResource(R.string.link_on_phone),
                listOf(
                    pluralStringResource(R.plurals.count_projects, nProjects, nProjects),
                    pluralStringResource(R.plurals.count_recent, nRecent, nRecent),
                    pluralStringResource(R.plurals.count_archived, nArchived, nArchived),
                ).joinToString(" · "),
            )
            GroupLabel(colors, stringResource(R.string.link_log))
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(colors.elevated).padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                for (line in status.log.asReversed()) {
                    Text("${fmt.format(java.util.Date(line.atMs))}  ${line.message}", color = colors.text, fontFamily = ZeronType.Mono, fontSize = 11.sp)
                }
            }
            Spacer(Modifier.height(12.dp))
            Row {
                Pill(colors, stringResource(R.string.retry_now), primary = true) { model.retryDirect() }
            }
            Spacer(Modifier.height(40.dp))
        }
    }
}

/**
 * Phone clock vs the computer's, from session heartbeats: within a couple of
 * seconds reads "in sync", otherwise which side is ahead and by how much.
 */
@Composable
internal fun clockOffsetText(offsetMs: Long): String {
    val secs = kotlin.math.abs(offsetMs) / 1000
    return when {
        secs < 3 -> stringResource(R.string.link_clock_in_sync)
        offsetMs > 0 -> stringResource(R.string.link_clock_phone_ahead, secs.toInt())
        else -> stringResource(R.string.link_clock_phone_behind, secs.toInt())
    }
}

/**
 * Connection details > 线路 (Routes): the network the phone is on, then every
 * address of the computer in the order it is tried, with the one in use
 * and what each did last.
 */
@Composable
private fun RoutesSection(model: ZeronModel, colors: ZeronColors, status: DirectStatus) {
    if (status.endpoints.isEmpty()) return
    GroupLabel(colors, stringResource(R.string.route_group))
    SettingRow(colors, stringResource(R.string.route_network), networkText(model.network))
    for (e in status.endpoints) {
        val kind = sh.zeron.android.core.EndpointKind.fromWire(e.kind)
        val title = "${kind.label()} · ${sh.zeron.android.core.Endpoint(e.host, e.port.toInt()).display()}"
        val reason = endpointReason(e, several = status.endpoints.size > 1)
        val detail = e.lastError?.takeIf { !e.active }
        SettingRow(colors, title, listOfNotNull(reason, detail).joinToString("\n"))
    }
    Text(
        stringResource(if (model.autoRoute) R.string.route_auto_hint else R.string.route_manual_hint),
        color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 12.sp,
        modifier = Modifier.padding(start = 4.dp, top = 4.dp, bottom = 4.dp),
    )
}

/** "Wi-Fi 192.168.1.0/24 · Tailscale 已开启" ("… · Tailscale on"). */
@Composable
internal fun networkText(net: sh.zeron.android.core.NetworkSnapshot): String {
    val subnet = net.localSubnets.firstOrNull()?.toString().orEmpty()
    val where = when (net.transport) {
        sh.zeron.android.core.NetworkSnapshot.Transport.WIFI -> stringResource(R.string.net_wifi, subnet).trim()
        sh.zeron.android.core.NetworkSnapshot.Transport.ETHERNET -> stringResource(R.string.net_ethernet, subnet).trim()
        sh.zeron.android.core.NetworkSnapshot.Transport.CELLULAR -> stringResource(R.string.net_cellular)
        sh.zeron.android.core.NetworkSnapshot.Transport.NONE -> stringResource(R.string.net_none)
        sh.zeron.android.core.NetworkSnapshot.Transport.OTHER -> stringResource(R.string.net_other)
    }
    val vpn = when (net.vpn) {
        sh.zeron.android.core.NetworkSnapshot.Vpn.TAILSCALE -> stringResource(R.string.net_vpn_tailscale)
        sh.zeron.android.core.NetworkSnapshot.Vpn.OTHER -> stringResource(R.string.net_vpn_other)
        sh.zeron.android.core.NetworkSnapshot.Vpn.NONE -> stringResource(R.string.net_vpn_none)
        sh.zeron.android.core.NetworkSnapshot.Vpn.UNKNOWN -> null
    }
    return listOfNotNull(where, vpn).joinToString(" · ")
}
