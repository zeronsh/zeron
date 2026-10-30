package sh.zeron.android.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenuGroup
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.DropdownMenuPopup
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MenuDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import sh.zeron.android.core.CloudStore
import sh.zeron.android.design.HarnessMark
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.ContextUsage
import uniffi.zeron_core.CoreClient
import uniffi.zeron_core.DeviceAccountInfo
import uniffi.zeron_core.agentLoginRules
import kotlin.coroutines.cancellation.CancellationException
import kotlin.math.roundToInt

/** Forced usage probes hit the provider: not more often than this per computer (the desktop's rule). */
private const val FORCE_MIN_INTERVAL_MS = 30_000L
/** Re-probe while a chat is open: usage moves as turns run. */
private const val POLL_INTERVAL_MS = 5 * 60_000L

/**
 * The last account list read from each computer, shared by the composer's
 * usage pill and the accounts page so a switch in one shows in the other
 * (the desktop's `AccountsSnapshotCache`). Main thread only.
 */
object DeviceAccountsCache {
    private val lists = mutableStateMapOf<String, List<DeviceAccountInfo>>()
    private val forcedAt = HashMap<String, Long>()
    private var owner: CoreClient? = null

    // Reads the map first, even for another owner: a screen that found nothing still hears the first put.
    fun get(core: CoreClient, deviceId: String): List<DeviceAccountInfo>? = lists[deviceId]?.takeIf { owner === core }

    fun put(core: CoreClient, deviceId: String, list: List<DeviceAccountInfo>) {
        if (owner !== core) {
            // Another account (or sign-out): nothing of the last one carries over.
            lists.clear()
            forcedAt.clear()
            owner = core
        }
        lists[deviceId] = list
    }

    /** Sign-out: forget every list, throttle and the client (which is shut down). */
    fun reset() {
        lists.clear()
        forcedAt.clear()
        owner = null
    }

    /** Whether a forced probe of `deviceId` may go out now (and note that it does). */
    fun takeForce(deviceId: String, explicit: Boolean, now: Long = System.currentTimeMillis()): Boolean {
        if (!explicit && now - (forcedAt[deviceId] ?: 0L) < FORCE_MIN_INTERVAL_MS) return false
        forcedAt[deviceId] = now
        return true
    }

    /**
     * The device's list, or — `force` — with each login's usage probed now.
     * A probe asked for again too soon reads the computer's cache instead.
     */
    suspend fun load(core: CoreClient, deviceId: String, force: Boolean, explicit: Boolean = false): List<DeviceAccountInfo> {
        val probe = force && takeForce(deviceId, explicit)
        val list = if (probe) core.deviceAgentUsage(deviceId) else core.deviceAgentAccounts(deviceId)
        put(core, deviceId, list)
        return list
    }
}

/** The binding limit: a login's most-used window (0…1), if it reports any. */
val DeviceAccountInfo.peakUsage: Float?
    get() = usage.maxOfOrNull { it.usedFraction.coerceIn(0f, 1f) }

/** How full a usage window is: worth a warning past 70 %, near its limit past 90 %. */
enum class UsageLevel { Normal, High, Critical }

fun usageLevel(fraction: Float): UsageLevel = when {
    fraction >= 0.9f -> UsageLevel.Critical
    fraction >= 0.7f -> UsageLevel.High
    else -> UsageLevel.Normal
}

/** How full a window reads: the accent, amber when high, red when critical. */
@Composable
fun usageTone(fraction: Float): Color = when (usageLevel(fraction)) {
    UsageLevel.Critical -> MaterialTheme.colorScheme.error
    UsageLevel.High -> Color(0xFFD97706)
    UsageLevel.Normal -> MaterialTheme.colorScheme.primary
}

private fun percent(fraction: Float) = "${(fraction.coerceIn(0f, 1f) * 100).roundToInt()}%"

/** What the usage pill reads: the busiest window's share, else the plan's name. */
fun pillLabel(login: DeviceAccountInfo): String = login.peakUsage?.let(::percent) ?: (login.planLabel ?: "Plan")

/**
 * The login an agent runs on, among its `logins`. An agent with a login per
 * model provider (OpenCode) has one in use per provider: the one reporting
 * usage leads.
 */
fun loginInUse(logins: List<DeviceAccountInfo>): DeviceAccountInfo? =
    logins.filter { it.active }.let { live -> live.firstOrNull { it.usage.isNotEmpty() } ?: live.firstOrNull() }

/**
 * The quick switcher's rows: the logins `inUse` can be swapped for (same
 * model provider), or none when the agent can't switch or there's no choice.
 */
fun switchChoices(logins: List<DeviceAccountInfo>, inUse: DeviceAccountInfo, switches: Boolean): List<DeviceAccountInfo> {
    if (!switches) return emptyList()
    val group = logins.filter { it.provider == inUse.provider }
    return if (group.size > 1) group else emptyList()
}

/** `list` with `account` in use across its group (its agent, or that agent's model provider). */
fun withActive(list: List<DeviceAccountInfo>, account: DeviceAccountInfo): List<DeviceAccountInfo> =
    list.map { if (it.harness == account.harness && it.provider == account.provider) it.copy(active = it.id == account.id) else it }

/**
 * The CLI logins one chat's agent runs on, on its computer: loaded cached
 * then probed, re-probed every few minutes, and switched optimistically.
 * Logins belong to the computer, so every chat of that agent there shares
 * the one in use.
 */
@Stable
class AccountUsageState(private val core: CoreClient, val deviceId: String, val harness: String, private val scope: CoroutineScope) {
    // Lazy: a native call, and the loading and switching below don't need it (JVM tests run without the core).
    val rules by lazy { agentLoginRules(harness) }

    /** Account id with a switch in flight. */
    var busy by mutableStateOf<String?>(null)
        private set
    private var generation = 0

    /** This agent's logins on the computer (null until the first list lands). */
    val logins: List<DeviceAccountInfo>?
        get() = DeviceAccountsCache.get(core, deviceId)?.filter { it.harness == harness }

    /** The login the agent runs on ([loginInUse]). */
    val active: DeviceAccountInfo?
        get() = loginInUse(logins.orEmpty())

    suspend fun load(force: Boolean) {
        val gen = ++generation
        try {
            val list = if (force && DeviceAccountsCache.takeForce(deviceId, explicit = false)) {
                core.deviceAgentUsage(deviceId)
            } else {
                core.deviceAgentAccounts(deviceId)
            }
            // A switch since this list was asked for wins over it.
            if (gen == generation) DeviceAccountsCache.put(core, deviceId, list)
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            // The pill keeps what it last showed (or stays hidden).
        }
    }

    /** A quiet, throttled probe (opening the switcher). */
    fun refresh() {
        scope.launch { load(force = true) }
    }

    /** Make `account` the agent's login on the computer; undone with `onError` if the computer refuses. */
    fun activate(account: DeviceAccountInfo, onError: (String) -> Unit) {
        if (account.active || !account.switchable || busy != null) return
        val before = DeviceAccountsCache.get(core, deviceId) ?: return
        // Optimistic: shown in use before the computer answers.
        DeviceAccountsCache.put(core, deviceId, withActive(before, account))
        busy = account.id
        generation++
        scope.launch {
            try {
                DeviceAccountsCache.put(core, deviceId, core.deviceActivateAccount(deviceId, account.harness, account.id))
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                DeviceAccountsCache.put(core, deviceId, before)
                onError("Couldn't switch: ${CloudStore.message(e)}")
            } finally {
                busy = null
            }
        }
    }
}

/**
 * The chat's agent logins on `deviceId`, or null for an agent without plan
 * usage (or `enabled` false: a Cloud chat). Loads at once and re-probes
 * while on screen.
 */
@Composable
fun rememberAccountUsage(core: CoreClient, deviceId: String, harness: String, enabled: Boolean): AccountUsageState? {
    val scope = rememberCoroutineScope()
    val state = remember(core, deviceId, harness) { AccountUsageState(core, deviceId, harness, scope) }
    val shown = enabled && deviceId.isNotEmpty() && state.rules.signsIn && state.rules.reportsUsage
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(state, shown, lifecycle) {
        if (!shown) return@LaunchedEffect
        // Only while the app is on screen: a phone in a pocket doesn't probe the provider.
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            // Paint what the computer has, then ask for fresh figures.
            if (state.logins == null) state.load(force = false)
            while (isActive) {
                state.load(force = true)
                delay(POLL_INTERVAL_MS)
            }
        }
    }
    return state.takeIf { shown }
}

/** Whether [PlanUsageChip] shows; when it does it carries the chat's context window too. */
fun planUsageShown(state: AccountUsageState?): Boolean = state?.active != null

/**
 * The composer's one usage pill: a ring and the busiest window of the login
 * the chat's agent runs on. Tapping opens a quick switcher — that login's
 * windows, this chat's context window, then each of the agent's logins as one
 * compact row; a tap on a row switches the computer to it.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun PlanUsageChip(state: AccountUsageState?, context: ContextUsage?, onError: (String) -> Unit, onManage: () -> Unit) {
    if (state == null) return
    val active = state.active ?: return
    val fraction = active.peakUsage
    var open by remember { mutableStateOf(false) }
    ContextChip(
        pillLabel(active),
        leading = { UsageRing(fraction ?: 0f, Modifier.size(15.dp)) },
        onClick = {
            open = true
            state.refresh()
        },
        tint = if (usageLevel(fraction ?: 0f) == UsageLevel.Critical) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
    ) {
        DropdownMenuPopup(expanded = open, onDismissRequest = { open = false }) {
            DropdownMenuGroup(shapes = MenuDefaults.groupShape(0, 1)) {
                Column(Modifier.width(300.dp).padding(vertical = 4.dp)) {
                    // The login in use, window by window.
                    Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            HarnessMark(active.harness, 16.dp)
                            Spacer(Modifier.width(8.dp))
                            Text(deviceAccountTitle(active), style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                            active.planLabel?.let { Text(it, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                        }
                        if (active.usage.isEmpty()) {
                            Text(active.usageError ?: "Usage unavailable", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(top = 8.dp))
                        } else {
                            active.usage.forEach { w -> CompactMeter(w.label, w.usedFraction, w.resetsAtMs?.let { "Resets ${resetLabel(it)}" }) }
                            active.usageError?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(top = 6.dp)) }
                        }
                    }
                    // This chat's context window: one usage pill, not two.
                    contextFraction(context)?.let { f ->
                        HorizontalDivider(Modifier.padding(vertical = 4.dp), color = MaterialTheme.colorScheme.outlineVariant)
                        Column(Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) {
                            Text("This chat", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary)
                            CompactMeter("Context", f, contextDetail(context))
                        }
                    }
                    val logins = switchChoices(state.logins.orEmpty(), active, state.rules.switches)
                    if (logins.isNotEmpty()) {
                        HorizontalDivider(Modifier.padding(vertical = 4.dp), color = MaterialTheme.colorScheme.outlineVariant)
                        Text("Switch account", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary, modifier = Modifier.padding(start = 16.dp, top = 4.dp, bottom = 2.dp))
                        logins.forEach { a -> QuickAccountRow(a, busy = state.busy == a.id) { state.activate(a, onError) } }
                    }
                    HorizontalDivider(Modifier.padding(vertical = 4.dp), color = MaterialTheme.colorScheme.outlineVariant)
                    DropdownMenuItem(
                        text = { Text("Manage accounts") },
                        leadingIcon = { ZIcon(ZIcons.Key, null, Modifier.size(20.dp)) },
                        onClick = {
                            open = false
                            onManage()
                        },
                    )
                }
            }
        }
    }
}

/** The share of the context window in use, when the agent reported both sides. */
fun contextFraction(context: ContextUsage?): Float? {
    val tokens = context?.tokens ?: return null
    val window = context.window ?: return null
    if (window == 0uL) return null
    return (tokens.toDouble() / window.toDouble()).toFloat().coerceIn(0f, 1f)
}

/**
 * The separate context chip's share: shown from half full, and folded into
 * the usage pill (its switcher) when that shows.
 */
fun contextChipFraction(context: ContextUsage?, pillShown: Boolean): Float? =
    if (pillShown) null else contextFraction(context)?.takeIf { it >= 0.5f }

private fun contextDetail(context: ContextUsage?): String? {
    val tokens = context?.tokens ?: return null
    val window = context.window ?: return null
    return "${compactTokens(tokens)} of ${compactTokens(window)} tokens"
}

private fun compactTokens(n: ULong): String {
    val v = n.toDouble()
    return when {
        v >= 1_000_000 -> String.format(java.util.Locale.US, "%.1fM", v / 1_000_000).replace(".0M", "M")
        v >= 1_000 -> "${(v / 1_000).toInt()}k"
        else -> n.toString()
    }
}

/** A small ring tinted by level. */
@Composable
private fun UsageRing(fraction: Float, modifier: Modifier = Modifier) {
    val f = fraction.coerceIn(0f, 1f)
    CircularProgressIndicator(
        progress = { if (f > 0f) maxOf(f, 0.04f) else 0f },
        modifier = modifier,
        color = usageTone(f),
        trackColor = MaterialTheme.colorScheme.outlineVariant,
        strokeWidth = 2.5.dp,
        gapSize = 0.dp,
    )
}

/** One window on a line: label, bar, percent, with when it resets under it. */
@Composable
private fun CompactMeter(label: String, fraction: Float, detail: String?) {
    val f = fraction.coerceIn(0f, 1f)
    Column(Modifier.padding(top = 8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(label, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, modifier = Modifier.width(64.dp))
            UsageLine(f, Modifier.weight(1f))
            Text(percent(f), style = MaterialTheme.typography.labelMedium, color = if (usageLevel(f) != UsageLevel.Normal) usageTone(f) else MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.End, modifier = Modifier.width(44.dp))
        }
        detail?.let { Text(it, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.outline, modifier = Modifier.padding(start = 64.dp)) }
    }
}

@Composable
private fun UsageLine(fraction: Float, modifier: Modifier) {
    LinearProgressIndicator(
        progress = { if (fraction > 0f) maxOf(fraction, 0.015f) else 0f },
        modifier = modifier.height(4.dp),
        color = usageTone(fraction),
        trackColor = MaterialTheme.colorScheme.surfaceContainerHighest,
        strokeCap = StrokeCap.Round,
        gapSize = 0.dp,
        drawStopIndicator = {},
    )
}

/** A login in the quick switcher: who, plan, its busiest window; the one in use ticked. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun QuickAccountRow(a: DeviceAccountInfo, busy: Boolean, onSwitch: () -> Unit) {
    val f = a.peakUsage
    Row(
        Modifier
            .fillMaxWidth()
            .clickable(enabled = !a.active && a.switchable && !busy, onClick = onSwitch)
            .padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(20.dp), contentAlignment = Alignment.Center) {
            when {
                busy -> LoadingIndicator(Modifier.size(20.dp))
                a.active -> ZIcon(ZIcons.Check, "In use", Modifier.size(18.dp), tint = MaterialTheme.colorScheme.primary)
            }
        }
        Spacer(Modifier.width(10.dp))
        Column(Modifier.weight(1f)) {
            Text(
                deviceAccountTitle(a),
                style = MaterialTheme.typography.bodyMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                color = if (a.switchable || a.active) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.outline,
            )
            a.planLabel?.let { Text(it, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
        }
        if (f != null) {
            Spacer(Modifier.width(8.dp))
            UsageLine(f, Modifier.width(44.dp))
            Text(percent(f), style = MaterialTheme.typography.labelMedium, color = if (usageLevel(f) != UsageLevel.Normal) usageTone(f) else MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.End, modifier = Modifier.width(44.dp))
        }
    }
}
