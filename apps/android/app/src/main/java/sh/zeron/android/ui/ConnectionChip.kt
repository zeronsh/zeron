package sh.zeron.android.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.slideInVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import sh.zeron.android.R
import sh.zeron.android.core.ConnectionIssue
import sh.zeron.android.core.ConnectionState
import sh.zeron.android.core.Endpoint
import sh.zeron.android.core.EndpointKind
import sh.zeron.android.core.Machine
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.ChevronMark
import sh.zeron.android.design.MenuDivider
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronType

/** Traffic-light colour of a connection dot. */
internal fun ConnectionState.Dot.color(colors: ZeronColors): Color = when (this) {
    ConnectionState.Dot.CONNECTED -> colors.success
    ConnectionState.Dot.CONNECTING -> colors.warning
    ConnectionState.Dot.FAILED -> colors.danger
    ConnectionState.Dot.NEUTRAL -> colors.tertiary
}

/** The dot; yellow breathes (alpha pulse) while connecting. */
@Composable
internal fun ConnectionDot(dot: ConnectionState.Dot, colors: ZeronColors, modifier: Modifier = Modifier) {
    val alpha = if (dot == ConnectionState.Dot.CONNECTING) {
        val pulse = rememberInfiniteTransition(label = "connecting")
        pulse.animateFloat(1f, 0.25f, infiniteRepeatable(tween(700, easing = LinearEasing), RepeatMode.Reverse), label = "alpha").value
    } else {
        1f
    }
    Box(modifier.size(8.dp).graphicsLayer { this.alpha = alpha }.clip(CircleShape).background(dot.color(colors)))
}

@Composable
internal fun ConnectionState.Dot.label(): String = stringResource(
    when (this) {
        ConnectionState.Dot.CONNECTED -> R.string.conn_state_connected
        ConnectionState.Dot.CONNECTING -> R.string.conn_state_connecting
        ConnectionState.Dot.FAILED -> R.string.conn_state_failed
        ConnectionState.Dot.NEUTRAL -> R.string.conn_state_demo
    },
)

/**
 * Home title bar chip (iOS title-menu style): dot + computer name + small
 * chevron. Long names ellipsize; the caller gives it a bounded width.
 * Tap: the computer's page (link state, why it's down, fixes); long-press:
 * the quick switcher.
 */
@OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
@Composable
internal fun ConnectionChip(model: ZeronModel, colors: ZeronColors, modifier: Modifier = Modifier) {
    val view = model.connectionView()
    val desc = stringResource(R.string.conn_chip_desc, view.title, view.dot.label())
    val haptics = androidx.compose.ui.platform.LocalHapticFeedback.current
    Row(
        modifier
            .height(30.dp)
            .clip(RoundedCornerShape(15.dp))
            .background(colors.controlFill)
            .combinedClickable(
                role = Role.Button,
                onLongClickLabel = stringResource(R.string.switch_computer),
                onLongClick = {
                    haptics.performHapticFeedback(androidx.compose.ui.hapticfeedback.HapticFeedbackType.LongPress)
                    model.openSwitcher()
                },
                onClick = { model.openConnectionChip() },
            )
            .semantics(mergeDescendants = true) { contentDescription = desc }
            .testTag("connection-chip")
            .padding(start = 8.dp, end = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        ConnectionDot(view.dot, colors)
        Spacer(Modifier.width(6.dp))
        Text(
            view.title,
            color = colors.text,
            fontFamily = ZeronType.Sans,
            fontWeight = FontWeight.Medium,
            fontSize = 13.sp,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f, fill = false),
        )
        view.route?.let { route ->
            Spacer(Modifier.width(5.dp))
            RouteTag(colors, route)
        }
        Spacer(Modifier.width(2.dp))
        ChevronMark(colors.secondary, Modifier.size(10.dp), expanded = true)
    }
}

@Composable
internal fun EndpointKind.label(): String = stringResource(
    when (this) {
        EndpointKind.LAN -> R.string.route_lan
        EndpointKind.TAILSCALE -> R.string.route_tailscale
        EndpointKind.OTHER -> R.string.route_other
    },
)

/**
 * Small "局域网" (LAN) / "Tailscale" tag: which address the link runs over.
 * [connected]: filled with the accent (white text) — the address in use now.
 */
@Composable
internal fun RouteTag(colors: ZeronColors, route: EndpointKind, modifier: Modifier = Modifier, connected: Boolean = false) {
    Text(
        route.label(),
        color = if (connected) Color.White else colors.secondary,
        fontFamily = ZeronType.Sans,
        fontWeight = FontWeight.Medium,
        fontSize = 10.sp,
        maxLines = 1,
        modifier = modifier.clip(RoundedCornerShape(6.dp))
            .background(if (connected) colors.accent else colors.hairline.copy(alpha = 0.5f))
            .padding(horizontal = 5.dp, vertical = 1.dp).testTag(if (connected) "route-tag-connected" else "route-tag"),
    )
}

/** Solid bottom sheet sliding up over a scrim (same look as the usage sheet). */
@Composable
internal fun BottomSheetFrame(colors: ZeronColors, onDismiss: () -> Unit, tag: String? = null, content: @Composable ColumnScope.() -> Unit) {
    BackHandler(onBack = onDismiss)
    val none = remember { MutableInteractionSource() }
    var shown by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) { shown = true }
    Box(
        Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.45f)).clickable(interactionSource = none, indication = null, onClick = onDismiss),
        contentAlignment = Alignment.BottomCenter,
    ) {
        AnimatedVisibility(shown, enter = slideInVertically { it / 2 } + fadeIn()) {
            Column(
                Modifier
                    .widthIn(max = 560.dp)
                    .fillMaxWidth()
                    .windowInsetsPadding(WindowInsets.navigationBars)
                    .padding(12.dp)
                    .clip(RoundedCornerShape(28.dp))
                    .background(colors.sheet)
                    .clickable(interactionSource = none, indication = null) {}
                    .then(if (tag != null) Modifier.testTag(tag) else Modifier)
                    .heightIn(max = 620.dp)
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 20.dp, vertical = 18.dp),
                content = content,
            )
        }
    }
}

/** Whichever connection sheet the model asks for, over the home screen. */
@Composable
internal fun ConnectionSheetHost(model: ZeronModel, colors: ZeronColors) {
    when (model.connectionSheet) {
        ZeronModel.ConnectionSheet.SWITCHER -> ConnectionSwitcherSheet(model, colors)
        ZeronModel.ConnectionSheet.FAILURE -> ConnectionFailureSheet(model, colors)
        null -> {}
    }
}

@Composable
private fun SheetTitle(colors: ZeronColors, text: String) {
    Text(text, color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
}

/** Quick switcher: saved computers, Zeron Cloud and Demo, then the full screens. */
@Composable
internal fun ConnectionSwitcherSheet(model: ZeronModel, colors: ZeronColors) {
    val close = { model.connectionSheet = null }
    val view = model.connectionView()
    BottomSheetFrame(colors, onDismiss = close, tag = "connection-switcher") {
        SheetTitle(colors, stringResource(R.string.switch_computer))
        Spacer(Modifier.height(10.dp))
        model.machines.forEach { machine ->
            val active = view.id == machine.id
            SwitcherRow(
                colors,
                title = machine.title(),
                subtitle = if (active) listOfNotNull(view.dot.label(), view.route?.label()).joinToString(" · ") else addressesLine(machine),
                dot = if (active) view.dot else when (model.machineOnline[machine.id]) {
                    true -> ConnectionState.Dot.CONNECTED
                    false -> ConnectionState.Dot.FAILED
                    null -> null
                },
                active = active,
            ) {
                if (machine.hostKey == null && !active) {
                    close()
                    model.editMachine = machine
                } else {
                    model.switchConnection(machine.id)
                }
            }
            // Auto-select route off: the computer opens up into its addresses.
            val addresses = machine.addresses()
            if (!model.autoRoute && addresses.size > 1) {
                model.routePicks
                val picked = model.pinnedAddress(machine)
                Column(Modifier.padding(start = 34.dp, bottom = 4.dp).testTag("switcher-routes-${machine.id}")) {
                    addresses.forEach { a ->
                        RouteChoiceRow(colors, a, selected = a.key == picked.key) {
                            close()
                            model.pickRoute(machine, a)
                        }
                    }
                }
            }
        }
        val cloud = view.id == "cloud"
        SwitcherRow(colors, "Zeron Cloud", if (cloud) view.dot.label() else stringResource(R.string.cloud_sub), if (cloud) view.dot else null, cloud) { model.switchConnection("cloud") }
        val demo = view.id == "demo"
        SwitcherRow(colors, stringResource(R.string.demo), stringResource(R.string.demo_sub), if (demo) ConnectionState.Dot.NEUTRAL else null, demo) { model.switchConnection("demo") }
        Spacer(Modifier.height(6.dp))
        MenuDivider(colors)
        Spacer(Modifier.height(6.dp))
        SheetLink(colors, stringResource(R.string.add_computer_ellipsis)) { close(); model.editMachine = Machine() }
        SheetLink(colors, stringResource(R.string.accounts_computers_ellipsis)) { close(); model.showMachines = true }
    }
}

/** "tx_vi@192.168.1.102 · 100.124.7.39": the sign-in and every address. */
internal fun addressesLine(machine: Machine): String {
    val list = machine.addresses()
    val first = list.firstOrNull()?.display() ?: machine.host
    return (listOf("${machine.user}@$first") + list.drop(1).map { it.display() }).joinToString(" · ")
}

@Composable
private fun SwitcherRow(colors: ZeronColors, title: String, subtitle: String, dot: ConnectionState.Dot?, active: Boolean, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(if (active) colors.rowActive else Color.Transparent)
            .clickable(onClick = onClick).padding(horizontal = 12.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(10.dp), contentAlignment = Alignment.Center) {
            if (dot != null) ConnectionDot(dot, colors) else Box(Modifier.size(8.dp).clip(CircleShape).background(colors.hairline))
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(title, color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 16.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(subtitle, color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (active) Text("✓", color = colors.accent, fontSize = 16.sp, modifier = Modifier.padding(start = 8.dp))
    }
}

/** One address to pick by hand: kind tag, host, a tick on the one in use. */
@Composable
internal fun RouteChoiceRow(colors: ZeronColors, address: Endpoint, selected: Boolean, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).clickable(onClick = onClick)
            .padding(horizontal = 10.dp, vertical = 7.dp).testTag("route-choice-${address.key}"),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RouteTag(colors, address.kind)
        Spacer(Modifier.width(8.dp))
        Text(address.display(), color = if (selected) colors.text else colors.secondary, fontFamily = ZeronType.Mono, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        if (selected) Text(stringResource(R.string.route_selected), color = colors.accent, fontFamily = ZeronType.Sans, fontSize = 12.sp, modifier = Modifier.padding(start = 8.dp))
    }
}

@Composable
private fun SheetLink(colors: ZeronColors, label: String, onClick: () -> Unit) {
    Text(
        label,
        color = colors.accent,
        fontFamily = ZeronType.Sans,
        fontSize = 15.sp,
        modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).clickable(onClick = onClick).padding(horizontal = 12.dp, vertical = 11.dp),
    )
}

internal fun ConnectionIssue.Kind.titleRes(): Int = when (this) {
    ConnectionIssue.Kind.TIMEOUT -> R.string.conn_kind_timeout
    ConnectionIssue.Kind.REFUSED -> R.string.conn_kind_refused
    ConnectionIssue.Kind.UNREACHABLE -> R.string.conn_kind_unreachable
    ConnectionIssue.Kind.DNS -> R.string.conn_kind_dns
    ConnectionIssue.Kind.AUTH_KEY -> R.string.conn_kind_auth_key
    ConnectionIssue.Kind.AUTH_PASSWORD -> R.string.conn_kind_auth_password
    ConnectionIssue.Kind.AUTH -> R.string.conn_kind_auth
    ConnectionIssue.Kind.HOST_KEY_UNKNOWN -> R.string.conn_kind_host_unknown
    ConnectionIssue.Kind.HOST_KEY_CHANGED -> R.string.conn_kind_host_changed
    ConnectionIssue.Kind.KEY -> R.string.conn_kind_key
    ConnectionIssue.Kind.ENGINE -> R.string.conn_kind_engine
    ConnectionIssue.Kind.LOST -> R.string.conn_kind_lost
    ConnectionIssue.Kind.SYNC -> R.string.conn_kind_sync
    ConnectionIssue.Kind.OFFLINE -> R.string.conn_kind_offline
    ConnectionIssue.Kind.UNKNOWN -> R.string.conn_kind_unknown
}

internal fun ConnectionIssue.Kind.hintRes(): Int = when (this) {
    ConnectionIssue.Kind.TIMEOUT -> R.string.conn_hint_timeout
    ConnectionIssue.Kind.REFUSED -> R.string.conn_hint_refused
    ConnectionIssue.Kind.UNREACHABLE -> R.string.conn_hint_unreachable
    ConnectionIssue.Kind.DNS -> R.string.conn_hint_dns
    ConnectionIssue.Kind.AUTH_KEY -> R.string.conn_hint_auth_key
    ConnectionIssue.Kind.AUTH_PASSWORD -> R.string.conn_hint_auth_password
    ConnectionIssue.Kind.AUTH -> R.string.conn_hint_auth
    ConnectionIssue.Kind.HOST_KEY_UNKNOWN -> R.string.conn_hint_host_unknown
    ConnectionIssue.Kind.HOST_KEY_CHANGED -> R.string.conn_hint_host_changed
    ConnectionIssue.Kind.KEY -> R.string.conn_hint_key
    ConnectionIssue.Kind.ENGINE -> R.string.conn_hint_engine
    ConnectionIssue.Kind.LOST -> R.string.conn_hint_lost
    ConnectionIssue.Kind.SYNC -> R.string.conn_hint_sync
    ConnectionIssue.Kind.OFFLINE -> R.string.conn_hint_offline
    ConnectionIssue.Kind.UNKNOWN -> R.string.conn_hint_unknown
}

/**
 * Why the link is down, in words; the raw engine text folds away under
 * "Show details". Actions: Retry (primary), Switch Computer, Close; plus
 * Edit Computer when the fix is on the phone side (keys, host key, password).
 */
@Composable
internal fun ConnectionFailureSheet(model: ZeronModel, colors: ZeronColors) {
    val close = { model.connectionSheet = null }
    val view = model.connectionView()
    val diagnosis = view.diagnosis
    val kind = diagnosis?.issue ?: if (view.workspace == ConnectionState.Workspace.CLOUD) ConnectionIssue.Kind.OFFLINE else ConnectionIssue.classify(view.error)
    var details by rememberSaveable { mutableStateOf(false) }
    BottomSheetFrame(colors, onDismiss = close, tag = "connection-failure") {
        Row(verticalAlignment = Alignment.CenterVertically) {
            ConnectionDot(ConnectionState.Dot.FAILED, colors)
            Spacer(Modifier.width(10.dp))
            Box(Modifier.weight(1f)) { SheetTitle(colors, stringResource(R.string.conn_fail_title, view.title)) }
        }
        Spacer(Modifier.height(14.dp))
        FailureReason(colors, view)
        val retryAt = view.retryAtMs
        if (diagnosis?.early == true) {
            // Found before the dial gave up: no countdown, it redials when the network changes.
            if (diagnosis.showsReconnectLine) {
                Spacer(Modifier.height(8.dp))
                Text(stringResource(R.string.conn_reconnects_by_itself), color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = 13.sp)
            }
        } else if (retryAt != null || kind.needsUser) {
            Spacer(Modifier.height(8.dp))
            var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
            if (retryAt != null) LaunchedEffect(retryAt) { while (true) { now = System.currentTimeMillis(); delay(1000) } }
            val line = when {
                retryAt == null -> stringResource(R.string.conn_waiting_for_you)
                retryAt > now -> stringResource(R.string.conn_auto_retry_in, ((retryAt - now + 999) / 1000).toInt())
                else -> stringResource(R.string.conn_auto_retry_now)
            }
            Text(line, color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = 13.sp)
        }
        val endpoints = model.directStatus?.endpoints.orEmpty()
        if (endpoints.size > 1) {
            Spacer(Modifier.height(12.dp))
            Text(stringResource(R.string.conn_fail_per_address), color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = 12.sp)
            Spacer(Modifier.height(4.dp))
            Column(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(colors.controlFill).padding(horizontal = 12.dp, vertical = 8.dp)
                    .testTag("failure-endpoints"),
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                endpoints.forEach { e -> FailureEndpointRow(colors, e, several = true) }
            }
        }
        val failedMachine = model.machines.firstOrNull { it.id == view.id }
        if (!model.autoRoute && failedMachine != null && failedMachine.addresses().size > 1) {
            model.routePicks
            val picked = model.pinnedAddress(failedMachine)
            Spacer(Modifier.height(12.dp))
            Text(stringResource(R.string.conn_switch_route), color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = 12.sp)
            Spacer(Modifier.height(4.dp))
            Column(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(colors.controlFill).padding(4.dp).testTag("failure-routes"),
            ) {
                failedMachine.addresses().forEach { a ->
                    RouteChoiceRow(colors, a, selected = a.key == picked.key) {
                        close()
                        model.pickRoute(failedMachine, a)
                    }
                }
            }
        }
        val raw = view.error
        if (!raw.isNullOrBlank()) {
            Spacer(Modifier.height(10.dp))
            Row(
                Modifier.clip(RoundedCornerShape(10.dp)).clickable { details = !details }.padding(vertical = 6.dp, horizontal = 2.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(stringResource(if (details) R.string.conn_hide_details else R.string.conn_show_details), color = colors.accent, fontFamily = ZeronType.Sans, fontSize = 14.sp)
                Spacer(Modifier.width(4.dp))
                ChevronMark(colors.accent, Modifier.size(11.dp).graphicsLayer { rotationZ = if (details) 180f else 0f }, expanded = true)
            }
            if (details) {
                Text(
                    raw,
                    color = colors.secondary,
                    fontFamily = ZeronType.Mono,
                    fontSize = 12.sp,
                    modifier = Modifier.fillMaxWidth().padding(top = 4.dp).clip(RoundedCornerShape(12.dp)).background(colors.controlFill).padding(12.dp),
                )
            }
        }
        Spacer(Modifier.height(18.dp))
        val fix = diagnosis?.opensTailscale == true || diagnosis?.installsTailscale == true
        if (fix) {
            TailscaleAction(model, colors, diagnosis)
            Spacer(Modifier.height(10.dp))
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            Pill(colors, stringResource(R.string.conn_retry), primary = !fix) { model.retryConnection() }
            Pill(colors, stringResource(R.string.switch_computer)) { model.connectionSheet = ZeronModel.ConnectionSheet.SWITCHER }
            Spacer(Modifier.weight(1f))
            Pill(colors, stringResource(R.string.close)) { close() }
        }
        val machine = model.machines.firstOrNull { it.id == view.id }
        if (machine != null && kind.needsUser) {
            Spacer(Modifier.height(4.dp))
            SheetLink(colors, stringResource(R.string.edit_computer)) { close(); model.editMachine = machine }
        }
    }
}

/** "局域网 192.168.1.102 — 连接超时" ("LAN 192.168.1.102 — Connection timed out") in the failure sheet. */
@Composable
private fun FailureEndpointRow(colors: ZeronColors, e: uniffi.zeron_core.DirectEndpointStat, several: Boolean) {
    Row(verticalAlignment = Alignment.Top) {
        RouteTag(colors, EndpointKind.fromWire(e.kind), Modifier.padding(top = 2.dp))
        Spacer(Modifier.width(8.dp))
        Column(Modifier.weight(1f)) {
            Text(sh.zeron.android.core.Endpoint(e.host, e.port.toInt()).display(), color = colors.text, fontFamily = ZeronType.Mono, fontSize = 13.sp)
            Text(endpointReason(e, several), color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp)
        }
    }
}

/** One address's last outcome in words (failure sheet, connection details). */
@Composable
internal fun endpointReason(e: uniffi.zeron_core.DirectEndpointStat, several: Boolean): String {
    val error = e.lastError
    return when {
        e.active -> e.latencyMs?.let { stringResource(R.string.route_in_use_ms, it.toInt()) } ?: stringResource(R.string.route_in_use)
        error != null -> {
            val kind = ConnectionIssue.classify(error)
            if (kind == ConnectionIssue.Kind.HOST_KEY_CHANGED && several) stringResource(R.string.route_stranger)
            else stringResource(kind.titleRes())
        }
        e.lastOkMs != null -> stringResource(R.string.route_ok_at, java.text.SimpleDateFormat("HH:mm", java.util.Locale.getDefault()).format(java.util.Date(e.lastOkMs!!)))
        e.lastAttemptMs != null -> stringResource(R.string.route_trying)
        else -> stringResource(R.string.route_untried)
    }
}
