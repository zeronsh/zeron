package sh.zeron.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.zeron.android.R
import sh.zeron.android.design.MarkKind
import sh.zeron.android.design.MenuEntry
import sh.zeron.android.design.StatusMark
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronType
import sh.zeron.android.design.glassSurface
import sh.zeron.android.design.menuSection
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.SendState
import uniffi.zeron_core.SessionRow
import uniffi.zeron_core.WorkspaceSnapshot

/**
 * What the home capsule counts: the connected computer's front-page sessions
 * (pinned, sections, recent; archived ones aren't on the list) that are
 * running or failed, each newest first. A row reads "failed" exactly when its
 * list corner does (a failed send or an errored turn; that wins over
 * running), so the capsule and the rows never disagree.
 */
data class HomeStats(val running: List<SessionRow>, val failed: List<SessionRow>) {
    val idle: Boolean get() = running.isEmpty() && failed.isEmpty()

    companion object {
        val EMPTY = HomeStats(emptyList(), emptyList())

        fun of(ws: WorkspaceSnapshot?): HomeStats {
            ws ?: return EMPTY
            val rows = (ws.front.pinned + ws.front.sections.flatMap { it.sessions } + ws.front.recent)
                .distinctBy { it.id }
                .sortedByDescending { it.lastActivityMs }
            val failed = rows.filter { isFailed(it) }
            val running = rows.filter { !isFailed(it) && it.indicator == ChatIndicator.WORKING }
            return HomeStats(running, failed)
        }

        /** A failed send, or a last run that errored (seen or not), unless a new run is live. */
        fun isFailed(row: SessionRow): Boolean = row.sendState == SendState.FAILED ||
            row.indicator == ChatIndicator.ERRORED ||
            (row.lastOutcome == ChatIndicator.ERRORED && row.indicator != ChatIndicator.WORKING && row.indicator != ChatIndicator.AWAITING_INPUT)
    }
}

/** "2 running, 1 failed" / "Idle", for the capsule's TalkBack label. */
@Composable
internal fun homeStatsSummary(running: Int, failed: Int): String {
    if (running == 0 && failed == 0) return stringResource(R.string.home_stats_idle)
    val parts = buildList {
        if (running > 0) add(pluralStringResource(R.plurals.home_stats_running, running, running))
        if (failed > 0) add(pluralStringResource(R.plurals.home_stats_failed, failed, failed))
    }
    return parts.joinToString(stringResource(R.string.home_stats_separator))
}

/**
 * The home title: running (dot-matrix + count) and failed (red dot + count)
 * sessions, or a muted Idle. Stays the screen's heading for TalkBack
 * ("Sessions: 2 running, 1 failed"); tapping opens [onOpen] unless idle.
 */
@Composable
internal fun HomeStatsCapsule(running: Int, failed: Int, colors: ZeronColors, modifier: Modifier = Modifier, onOpen: () -> Unit) {
    val idle = running == 0 && failed == 0
    val label = stringResource(R.string.home_stats_a11y, stringResource(R.string.sessions), homeStatsSummary(running, failed))
    val openLabel = stringResource(R.string.home_stats_open)
    Row(
        modifier
            .testTag("home-stats")
            // One node for TalkBack: the heading, what it counts, and the
            // action (the click below would otherwise add an unlabelled one).
            .clearAndSetSemantics {
                heading()
                contentDescription = label
                if (!idle) {
                    role = Role.Button
                    onClick(openLabel) { onOpen(); true }
                }
            }
            .height(30.dp)
            .glassSurface(colors, 15.dp)
            .clip(RoundedCornerShape(15.dp))
            .then(if (idle) Modifier else Modifier.clickable(onClick = onOpen))
            // Tight: the old 28sp "会话" (Sessions) + 6dp gap put the computer chip 62dp
            // in; capsule + its 4dp gap must not take more
            // (HomeStatsCapsuleWidthTest).
            .padding(horizontal = 5.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (idle) {
            Text(stringResource(R.string.home_stats_idle), color = colors.tertiary, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 13.sp, maxLines = 1, modifier = Modifier.padding(horizontal = 4.dp))
            return@Row
        }
        if (running > 0) {
            StatusMark(MarkKind.Spinner, colors, Modifier.size(12.dp))
            Spacer(Modifier.width(2.dp))
            Text("$running", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 13.sp, maxLines = 1)
        }
        if (failed > 0) {
            if (running > 0) Spacer(Modifier.width(5.dp))
            // The list's failed dot, without StatusMark's padding box.
            Box(Modifier.size(7.dp).clip(CircleShape).background(colors.failed))
            Spacer(Modifier.width(3.dp))
            Text("$failed", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 13.sp, maxLines = 1)
        }
    }
}

/** The capsule's popover: 运行中 (Working) and 失败 (Failed) groups (empty ones hidden); a row opens its session. */
@Composable
internal fun homeStatsEntries(stats: HomeStats, colors: ZeronColors, open: (String) -> Unit): List<MenuEntry> {
    val running = stringResource(R.string.status_working)
    val failed = stringResource(R.string.status_failed)
    return buildList {
        if (stats.running.isNotEmpty()) {
            add(menuSection(running))
            stats.running.forEach { row ->
                add(MenuEntry(row.title, subtitle = rowSubtitle(row), icon = { StatusMark(MarkKind.Spinner, colors, Modifier.size(14.dp)) }) { open(row.id) })
            }
        }
        if (stats.failed.isNotEmpty()) {
            add(menuSection(failed))
            stats.failed.forEach { row ->
                add(MenuEntry(row.title, subtitle = rowSubtitle(row), icon = { StatusMark(MarkKind.Dot(colors.failed), colors, Modifier.size(14.dp)) }) { open(row.id) })
            }
        }
    }
}

/** "project · branch", like the row's second line. */
internal fun rowSubtitle(row: SessionRow): String =
    listOfNotNull(SessionGrouping.rowProjectLabel(row), row.branch?.takeIf { it.isNotBlank() }).joinToString(" · ")
