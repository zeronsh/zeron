package sh.zeron.android.ui

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.LinearWavyProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONObject
import sh.zeron.android.core.AccountUsage
import sh.zeron.android.core.Agents
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.userMessage
import sh.zeron.android.design.HarnessMark
import sh.zeron.android.design.LocalDarkTheme
import uniffi.zeron_core.CoreClient
import uniffi.zeron_core.harnessLabel

/** One host's snapshot; cancelled requests never leak into a different chat host. */
class AccountUsageState(
    private val clock: () -> Long = { System.nanoTime() / 1_000_000 },
    private val fetch: suspend (Boolean) -> Agents.Accounts,
) {
    var snapshot by mutableStateOf<Agents.Accounts?>(null)
        private set
    var loading by mutableStateOf(true)
        private set
    var error by mutableStateOf<String?>(null)
        private set
    private val mutex = Mutex()
    private var lastForced: Long? = null

    suspend fun refresh(force: Boolean) = mutex.withLock {
        val now = clock()
        if (force && lastForced?.let { now - it < 30_000 } == true) return@withLock
        if (force) lastForced = now
        loading = true
        try {
            snapshot = fetch(force)
            error = null
        } catch (cancelled: CancellationException) {
            if (force) lastForced = null
            throw cancelled
        } catch (failure: Exception) {
            error = failure.userMessage()
        } finally {
            loading = false
        }
    }
}

@Composable
fun rememberAccountUsage(app: AppModel, client: CoreClient, deviceId: String): AccountUsageState {
    val state = remember(app, client, deviceId) {
        AccountUsageState { force ->
            Agents.accounts(app.hostCall(deviceId, Agents.LIST_ACCOUNTS, JSONObject().put("forceUsage", force)))
        }
    }
    val owner = LocalLifecycleOwner.current
    LaunchedEffect(state, owner) {
        owner.lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            // Paint the engine's persisted values before asking the provider for fresh limits.
            state.refresh(force = false)
            state.refresh(force = true)
            while (true) {
                delay(5 * 60_000L)
                state.refresh(force = true)
            }
        }
    }
    return state
}

/** A calm Material wavefront: usage advances right, with a slight rise at its leading edge. */
@Composable
fun Modifier.accountUsageFill(fraction: Float?): Modifier {
    val progress by animateFloatAsState(fraction ?: 0f, tween(600), label = "Account usage")
    val dark = LocalDarkTheme.current
    val violet = if (dark) Color(0xFFB2A5FF) else Color(0xFF6E50D7)
    return drawWithCache {
        fun wave(front: Float, bend: Float): Path = Path().apply {
            val x = size.width * front
            val h = size.height
            moveTo(0f, 0f)
            lineTo(x + bend, 0f)
            cubicTo(x + bend * 1.5f, h * .22f, x - bend * 1.8f, h * .43f, x, h * .58f)
            cubicTo(x + bend * 1.3f, h * .76f, x - bend, h * .91f, x - bend, h)
            lineTo(0f, h)
            close()
        }
        val p = progress.coerceIn(0f, 1f)
        val bend = size.height * .13f * (minOf(p, 1f - p) * 10f).coerceIn(0f, 1f)
        val fill = wave(p, bend)
        val crest = wave((p + .025f).coerceAtMost(1f), bend * .7f)
        val brush = Brush.horizontalGradient(listOf(violet.copy(alpha = .13f), violet.copy(alpha = .30f)))
        onDrawBehind {
            if (p > 0f) {
                drawPath(crest, violet.copy(alpha = .10f))
                drawPath(fill, brush)
            }
        }
    }
}

@Composable
fun AccountUsagePopover(expanded: Boolean, onDismiss: () -> Unit, state: AccountUsageState, harness: String) {
    val accounts = state.snapshot?.forHarness(harness).orEmpty()
    val warning = state.snapshot?.warnings?.get(harness)
    AnchoredPopover(expanded, onDismiss, width = 340.dp, maxHeight = 520.dp) {
        Column(
            Modifier.verticalScroll(rememberScrollState()).padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                HarnessMark(harness, 24.dp)
                Column {
                    Text("${harnessLabel(harness)} usage", style = MaterialTheme.typography.titleMedium)
                    Text("Accounts & subscription", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
            if (state.loading) LinearProgressIndicator(Modifier.fillMaxWidth())
            accounts.forEach { account -> UsageAccountCard(account) }
            if (accounts.isEmpty() && !state.loading && state.error == null && warning == null) {
                Text("No signed-in accounts on this device.", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            listOfNotNull(warning, state.error).distinct().forEach { message ->
                Text(message, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
            }
        }
    }
}

@Composable
private fun UsageAccountCard(account: Agents.Account) {
    Surface(
        shape = RoundedCornerShape(20.dp),
        color = if (account.active) MaterialTheme.colorScheme.surfaceContainerHighest else MaterialTheme.colorScheme.surfaceContainer,
    ) {
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(account.title, style = MaterialTheme.typography.titleSmall)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    account.plan?.let { Text(it, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                    if (account.active) {
                        Surface(shape = RoundedCornerShape(50), color = MaterialTheme.colorScheme.secondaryContainer) {
                            Text("In use", Modifier.padding(horizontal = 8.dp, vertical = 3.dp), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSecondaryContainer)
                        }
                    }
                }
                account.provider?.let { Text("Provider: $it", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
            }
            account.usageWindows.forEach { window ->
                val color = when {
                    window.usedFraction >= .9f -> MaterialTheme.colorScheme.error
                    window.usedFraction >= .75f -> warningColor()
                    else -> MaterialTheme.colorScheme.primary
                }
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(window.label, Modifier.weight(1f), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        Text(AccountUsage.percent(window.usedFraction), style = MaterialTheme.typography.labelMedium, color = color)
                    }
                    LinearWavyProgressIndicator(progress = { window.usedFraction }, color = color, modifier = Modifier.fillMaxWidth())
                    AccountUsage.reset(window.resetsAt)?.let {
                        Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
            account.usageError?.let {
                Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            if (account.usageWindows.isEmpty() && account.usageError == null && account.harness != "antigravity") {
                Text("No usage limits reported.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
}
