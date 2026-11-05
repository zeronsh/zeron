package sh.zeron.android.ui

import android.view.HapticFeedbackConstants
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.CloudStore
import sh.zeron.android.core.signInToCloud
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.DeviceAccountInfo
import uniffi.zeron_core.HarnessInfo
import uniffi.zeron_core.agentLoginRules
import kotlin.coroutines.cancellation.CancellationException

private const val CLOUD = "cloud"

/**
 * Settings → Providers (iOS `ProvidersViewController`), as on the desktop:
 * pick a device (a computer or Cloud), then turn its agents on or off and
 * choose the account each runs on. A computer's agents and logins live on
 * that computer; Cloud's live in its vault.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun ProvidersScreen(model: AppModel, target: String?, onBack: () -> Unit, onOpenAccounts: (AccountsTarget) -> Unit, onOpenCloud: () -> Unit) {
    val context = LocalContext.current
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    val workspace by model.workspace.collectAsState()
    val client by model.client.collectAsState()
    val cloud = model.cloud
    val prefs = remember { context.getSharedPreferences("settings", 0) }
    val computers = workspace?.devices.orEmpty().filter { it.isExecutionHost }
    val cloudOffered = model.cloudOffered

    // The device shown: the one asked for, else the last one, else a computer online.
    var selected by remember {
        val remembered = prefs.getString("providersDevice", null)?.takeIf { r -> r == CLOUD || computers.any { it.id == r } }
        mutableStateOf(target ?: remembered ?: computers.firstOrNull { it.online }?.id ?: computers.firstOrNull()?.id ?: CLOUD)
    }
    // Remembered Cloud, but this account isn't offered it: a computer.
    if (selected == CLOUD && !model.isDemo && cloud.info != null && !cloud.available && computers.isNotEmpty()) selected = computers.first().id
    val onCloud = selected == CLOUD
    val device = computers.firstOrNull { it.id == selected }
    val name = if (onCloud) "Cloud" else device?.name ?: model.deviceName(selected)
    val online = onCloud || device?.online == true

    val harnesses = remember { mutableStateMapOf<String, List<HarnessInfo>>() }
    val accounts = remember { mutableStateMapOf<String, List<DeviceAccountInfo>>() }
    val failed = remember { mutableStateMapOf<String, String>() }
    val toggling = remember { mutableStateMapOf<String, Boolean>() }
    var signingIn by remember { mutableStateOf<String?>(null) }
    var refreshing by remember { mutableStateOf(false) }
    var toggleError by remember { mutableStateOf<Pair<String, String>?>(null) }

    suspend fun loadAccounts(id: String) {
        val c = client ?: return
        if (computers.firstOrNull { it.id == id }?.online != true) return
        try {
            accounts[id] = c.deviceAgentAccounts(id)
            failed.remove(id)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            failed[id] = CloudStore.message(e)
        }
    }

    suspend fun load(id: String) {
        if (id == CLOUD) {
            if (cloud.info == null) cloud.refresh()
            cloud.refreshAccounts()
            return
        }
        val c = client ?: return
        runCatching { c.listHarnesses(id) }.onSuccess { harnesses[id] = it }
        loadAccounts(id)
    }

    // Each device on first show; back from an agent's accounts re-reads them.
    LaunchedEffect(selected, client) { load(selected) }

    fun select(id: String) {
        if (id == selected) return
        view.performHapticFeedback(HapticFeedbackConstants.CLOCK_TICK)
        selected = id
        prefs.edit().putString("providersDevice", id).apply()
    }

    fun setEnabled(h: HarnessInfo, on: Boolean) {
        val c = client ?: return
        val id = selected
        toggling[h.id] = true
        scope.launch {
            try {
                harnesses[id] = c.setHarnessEnabled(id, h.id, on)
                view.performHapticFeedback(HapticFeedbackConstants.CLOCK_TICK)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                toggleError = (if (on) "Couldn't turn it on" else "Couldn't turn it off") to CloudStore.message(e)
            } finally {
                toggling.remove(h.id)
            }
        }
    }

    fun startSignIn(agent: CloudStore.Agent) {
        if (signingIn != null) return
        signingIn = agent.login
        scope.launch {
            try {
                model.signInToCloud(context, agent)
                view.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
            } catch (e: CancellationException) {
                // The page was closed.
            } catch (e: Exception) {
                model.toast(CloudStore.message(e))
            } finally {
                signingIn = null
                cloud.refreshAccounts()
            }
        }
    }

    PullToRefreshBox(
        isRefreshing = refreshing,
        onRefresh = {
            refreshing = true
            scope.launch {
                load(selected)
                refreshing = false
            }
        },
        modifier = Modifier.fillMaxSize(),
    ) {
        CloudPage("Providers", "Agents and their accounts on each device", onBack) {
            item("devices") {
                // The device switcher: every computer that runs agents, then Cloud.
                Row(
                    Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    for (c in computers) {
                        Pill(c.name + if (c.online) "" else " · offline", c.id == selected, onClick = { select(c.id) }, icon = sh.zeron.android.voice.platformIcon(c))
                    }
                    if (cloudOffered) Pill("Cloud", onCloud, onClick = { select(CLOUD) }, icon = ZIcons.Cloud)
                }
            }
            val footer = when {
                onCloud -> "Cloud agents use the account marked in use. Signing in here stores the login in Cloud, not on this phone."
                !online -> "$name is offline — showing what it last reported."
                else -> "Agents run with the logins on $name. Switch accounts here; to add one, sign in from Zeron on that computer."
            }
            cloudSection("Agents", footer = footer, key = "agents-$selected") {
                if (onCloud) {
                    CloudAgentRows(model, cloudOffered, signingIn, onSignIn = ::startSignIn, onOpen = { onOpenAccounts(AccountsTarget.Cloud(it)) }, onOpenCloud = onOpenCloud)
                } else {
                    val list = harnesses[selected]
                    if (list == null) {
                        CloudRow(0, 1, "Loading…", leading = { LoadingIndicator(Modifier.size(32.dp)) })
                    } else {
                        val logins = accounts[selected]
                        val enabledCount = list.count { it.offered }
                        val sorted = list.sortedWith(compareBy({ if (it.installed) 0 else 1 }, { it.label }))
                        sorted.forEachIndexed { i, h ->
                            val mine = logins.orEmpty().filter { it.harness == h.id }
                            val rules = remember(h.id) { agentLoginRules(h.id) }
                            val detail = when {
                                !h.installed -> "Not installed"
                                logins == null -> failed[selected] ?: if (online) "Installed" else "Offline"
                                else -> accountSummary(mine.firstOrNull { it.active }?.let(::deviceAccountTitle), mine.size, showsNone = rules.signsIn)
                            }
                            // Every agent with logins opens its accounts — to add the first one too.
                            val opens = h.installed && (rules.signsIn || mine.isNotEmpty())
                            CloudRow(
                                i, sorted.size, h.label, detail,
                                leading = { Box(Modifier.alpha(if (h.installed) 1f else 0.45f)) { AgentTile(h.id) } },
                                trailing = {
                                    if (toggling[h.id] == true) {
                                        LoadingIndicator(Modifier.size(32.dp))
                                    } else {
                                        Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                            Switch(
                                                checked = h.installed && h.offered,
                                                onCheckedChange = { setEnabled(h, it) },
                                                // The last agent standing stays on: new sessions need one.
                                                enabled = online && h.installed && !(h.offered && enabledCount <= 1),
                                            )
                                            if (opens) Chevron()
                                        }
                                    }
                                },
                                titleColor = if (h.installed) androidx.compose.ui.graphics.Color.Unspecified else MaterialTheme.colorScheme.outline,
                                onClick = if (opens) ({ onOpenAccounts(AccountsTarget.Device(selected, name, h.id, h.label)) }) else null,
                            )
                        }
                        if (sorted.isEmpty()) CloudRow(0, 1, "No agents", if (online) "$name hasn't reported any agents." else "$name is offline.")
                    }
                }
            }
        }
    }

    toggleError?.let { (title, message) ->
        AlertDialog(
            onDismissRequest = { toggleError = null },
            title = { Text(title) },
            text = { Text(message) },
            confirmButton = { TextButton(onClick = { toggleError = null }) { Text("OK") } },
        )
    }
}

/** Cloud's agents: each with the login (or key) it uses, or a way to sign in. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun CloudAgentRows(
    model: AppModel,
    offered: Boolean,
    signingIn: String?,
    onSignIn: (CloudStore.Agent) -> Unit,
    onOpen: (CloudStore.Agent) -> Unit,
    onOpenCloud: () -> Unit,
) {
    val cloud = model.cloud
    if (!offered && !model.isDemo) {
        if (cloud.info == null) {
            CloudRow(0, 1, "Loading…", leading = { LoadingIndicator(Modifier.size(32.dp)) })
        } else {
            CloudRow(0, 1, "No computers yet", "Install Zeron on a computer to run agents and manage their accounts here.", leading = { IconTile(ZIcons.Monitor) })
        }
        return
    }
    CloudStore.agents.forEachIndexed { i, agent ->
        val count = cloud.accounts[agent.login].orEmpty().size + cloud.accounts[agent.key].orEmpty().size
        val busy = signingIn == agent.login
        val active = cloud.activeAccount(agent)?.let { a ->
            listOfNotNull(a.email ?: if (a.kind == "key") agent.keyName else null, a.plan).joinToString(" · ").ifEmpty { null }
        }
        CloudRow(
            i, CloudStore.agents.size, agent.name,
            if (busy) "Finishing sign-in…" else accountSummary(active, count),
            leading = { AgentTile(agent.harness) },
            trailing = when {
                busy -> ({ LoadingIndicator(Modifier.size(32.dp)) })
                count == 0 -> ({ FilledTonalButton(onClick = { onSignIn(agent) }) { Text("Sign in") } })
                else -> null
            },
            onClick = { onOpen(agent) },
        )
    }
}

fun deviceAccountTitle(a: DeviceAccountInfo): String = a.email ?: a.displayName ?: if (a.apiKey) "API key" else "Account"

/** "you@acme.dev · 2 accounts", "1 account", "No account". */
fun accountSummary(active: String?, count: Int, showsNone: Boolean = true): String? = when {
    active != null && count > 1 -> "$active · $count accounts"
    active != null -> active
    count == 0 -> if (showsNone) "No account" else null
    count == 1 -> "1 account"
    else -> "$count accounts"
}
