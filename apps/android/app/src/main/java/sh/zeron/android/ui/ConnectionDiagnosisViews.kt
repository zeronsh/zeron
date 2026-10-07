package sh.zeron.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextAlign
import sh.zeron.android.core.ConnectionIssue
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.zeron.android.R
import sh.zeron.android.core.ConnectionDiagnosis
import sh.zeron.android.core.ConnectionDiagnosis.Kind
import sh.zeron.android.core.ConnectionState
import sh.zeron.android.core.Machine
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronType

/** The diagnosis headline, or null to use the classified error's ([Kind.ISSUE]). */
internal fun ConnectionDiagnosis.Result.titleRes(): Int? = when (kind) {
    Kind.NO_NETWORK -> R.string.conn_diag_no_network
    Kind.TAILSCALE_OFF -> R.string.conn_diag_tailscale_off
    Kind.AWAY_TAILSCALE_OFF -> R.string.conn_diag_away_tailscale_off
    Kind.TAILSCALE_MISSING -> R.string.conn_diag_tailscale_missing
    Kind.NOT_SAME_WIFI -> R.string.conn_diag_not_same_wifi
    Kind.PC_UNREACHABLE -> R.string.conn_diag_pc_unreachable
    Kind.LAN_UNREACHABLE -> R.string.conn_diag_lan_unreachable
    Kind.ISSUE -> null
}

/** What to do about it, in plain words. */
internal fun ConnectionDiagnosis.Result.hintRes(): Int? = when (kind) {
    Kind.NO_NETWORK -> R.string.conn_diag_hint_no_network
    Kind.TAILSCALE_OFF -> when {
        otherVpn -> R.string.conn_diag_hint_other_vpn
        lanFailed -> R.string.conn_diag_hint_tailscale_off_lan
        else -> R.string.conn_diag_hint_tailscale_off
    }
    Kind.AWAY_TAILSCALE_OFF -> if (otherVpn) R.string.conn_diag_hint_other_vpn else R.string.conn_diag_hint_away_tailscale_off
    Kind.TAILSCALE_MISSING -> R.string.conn_diag_hint_tailscale_missing
    Kind.NOT_SAME_WIFI -> if (pickedLan) R.string.conn_diag_hint_picked_lan else R.string.conn_diag_hint_not_same_wifi
    Kind.PC_UNREACHABLE -> R.string.conn_diag_hint_pc_unreachable
    Kind.LAN_UNREACHABLE -> R.string.conn_diag_hint_lan_unreachable
    Kind.ISSUE -> null
}

/**
 * Show "reconnects by itself" under an early reason, unless its hint says
 * so already (the Tailscale-off and offline hints do).
 */
internal val ConnectionDiagnosis.Result.showsReconnectLine: Boolean
    get() = early && !(kind == Kind.NO_NETWORK || (kind == Kind.TAILSCALE_OFF || kind == Kind.AWAY_TAILSCALE_OFF) && !otherVpn)

/** "打开 Tailscale" / "安装 Tailscale" ("Open Tailscale" / "Install Tailscale") as a full-width primary button, when it's the fix. */
@Composable
internal fun TailscaleAction(model: ZeronModel, colors: ZeronColors, diagnosis: ConnectionDiagnosis.Result?, modifier: Modifier = Modifier) {
    val d = diagnosis ?: return
    val label = when {
        d.opensTailscale -> stringResource(R.string.conn_open_tailscale)
        d.installsTailscale -> stringResource(R.string.conn_install_tailscale)
        else -> return
    }
    Text(
        label,
        color = Color.White,
        fontFamily = ZeronType.Sans,
        fontWeight = FontWeight.SemiBold,
        fontSize = 15.sp,
        textAlign = TextAlign.Center,
        modifier = modifier.fillMaxWidth().clip(RoundedCornerShape(22.dp)).background(colors.accent)
            .clickable(role = Role.Button) { if (d.opensTailscale) model.openTailscale() else model.installTailscale() }
            .testTag("tailscale-action")
            .padding(vertical = 12.dp),
    )
}

/** Headline + hint for a failed link: the diagnosis when it has one, else the classified error. */
@Composable
internal fun FailureReason(colors: ZeronColors, view: ConnectionState.View) {
    val d = view.diagnosis
    val issue = d?.issue ?: if (view.workspace == ConnectionState.Workspace.CLOUD) ConnectionIssue.Kind.OFFLINE else ConnectionIssue.classify(view.error)
    val title = d?.titleRes() ?: issue.titleRes()
    val hint = d?.hintRes() ?: issue.hintRes()
    Text(stringResource(title), color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 15.sp, modifier = Modifier.testTag("failure-title"))
    Spacer(Modifier.height(4.dp))
    Text(stringResource(hint), color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 14.sp, lineHeight = 20.sp)
}

/**
 * Top of a computer's page (Settings > 账户与电脑 (Accounts & Computers) > computer, also what the
 * home chip opens): the link's state and route; when it's down, why and
 * the fix (打开 Tailscale / Open Tailscale…), then Retry / Connection Details / Switch.
 * Another computer than the active one gets Connect / Switch.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun ConnectionStatusCard(model: ZeronModel, colors: ZeronColors, machine: Machine) {
    val view = model.connectionView()
    val active = view.id == machine.id
    GroupLabel(colors, stringResource(R.string.conn_status_group))
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(colors.elevated).padding(14.dp).testTag("computer-status"),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            if (active) ConnectionDot(view.dot, colors)
            else androidx.compose.foundation.layout.Box(Modifier.size(8.dp).clip(CircleShape).background(colors.hairline))
            Spacer(Modifier.width(10.dp))
            val line = when {
                !active -> stringResource(R.string.conn_status_inactive)
                view.route != null -> stringResource(R.string.conn_status_route, view.dot.label(), view.route.label())
                else -> view.dot.label()
            }
            Text(line, color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 15.sp)
        }
        if (active && view.dot == ConnectionState.Dot.FAILED) {
            Spacer(Modifier.height(10.dp))
            FailureReason(colors, view)
            if (view.diagnosis?.showsReconnectLine == true) {
                Spacer(Modifier.height(6.dp))
                Text(stringResource(R.string.conn_reconnects_by_itself), color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = 13.sp)
            }
            if (view.diagnosis?.opensTailscale == true || view.diagnosis?.installsTailscale == true) {
                Spacer(Modifier.height(12.dp))
                TailscaleAction(model, colors, view.diagnosis)
            }
        }
        Spacer(Modifier.height(12.dp))
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (active) {
                if (view.dot == ConnectionState.Dot.FAILED) Pill(colors, stringResource(R.string.conn_retry), primary = view.diagnosis?.opensTailscale != true) { model.retryConnection() }
                Pill(colors, stringResource(R.string.connection_details)) { model.showLinkDetails = true }
            } else if (machine.hostKey != null) {
                Pill(colors, stringResource(R.string.conn_use_this_computer), primary = true) {
                    model.editMachine = null
                    model.switchConnection(machine.id)
                }
            }
            Pill(colors, stringResource(R.string.switch_computer)) { model.openSwitcher() }
        }
    }
}
