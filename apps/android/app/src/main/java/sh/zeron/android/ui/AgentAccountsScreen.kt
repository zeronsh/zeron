package sh.zeron.android.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.view.HapticFeedbackConstants
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.BrowserReturns
import sh.zeron.android.core.CloudStore
import sh.zeron.android.core.signInToCloud
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.CoreException
import uniffi.zeron_core.DeviceAccountInfo
import uniffi.zeron_core.LoginOptionInfo
import uniffi.zeron_core.UsageWindowInfo
import uniffi.zeron_core.agentLoginRules
import kotlin.coroutines.cancellation.CancellationException

/** One account, whichever device keeps it. */
private data class Account(
    val id: String,
    val title: String,
    val detail: String?,
    val usage: List<UsageWindowInfo>,
    val note: String?,
    val active: Boolean,
    val switchable: Boolean,
    val isKey: Boolean,
    /** Cloud's vault provider (`claude`, `anthropic-key`, …). */
    val cloudProvider: String?,
)

/** A sign-in under way: which option, and what to show while it waits. */
private data class SignInState(val option: String, val message: String? = null)

/**
 * One agent's accounts on one device — a computer or Cloud (iOS
 * `AgentAccountsViewController`): every login with its plan and usage, the
 * one in use, switching and removing, and adding more per login option. A
 * computer's sign-in runs on that computer; the phone opens the page and
 * relays the browser's return to it. Cloud's are signed in by the phone
 * itself and kept in Cloud's vault, which also takes an API key.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun AgentAccountsScreen(model: AppModel, target: AccountsTarget, signIn: Boolean, onBack: () -> Unit) {
    val context = LocalContext.current
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    val client by model.client.collectAsState()
    val cloud = model.cloud
    val rules = remember(target.harness) { agentLoginRules(target.harness) }
    val deviceName = when (target) {
        is AccountsTarget.Cloud -> "Cloud"
        is AccountsTarget.Device -> target.name
    }
    val title = when (target) {
        is AccountsTarget.Cloud -> target.agent.name
        is AccountsTarget.Device -> rules.name.ifEmpty { target.label }
    }

    var deviceAccounts by remember { mutableStateOf<List<DeviceAccountInfo>?>(null) }
    var loadError by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf<String?>(null) }
    var signing by remember { mutableStateOf<SignInState?>(null) }
    var signJob by remember { mutableStateOf<Job?>(null) }
    var askKey by remember { mutableStateOf(false) }
    var codeRequest by remember { mutableStateOf<CompletableDeferred<String?>?>(null) }
    var removing by remember { mutableStateOf<Account?>(null) }

    suspend fun load() {
        when (target) {
            is AccountsTarget.Cloud -> cloud.refreshAccounts()
            is AccountsTarget.Device -> {
                val c = client ?: return
                try {
                    deviceAccounts = c.deviceAgentAccounts(target.deviceId)
                    loadError = null
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    loadError = CloudStore.message(e)
                }
            }
        }
    }
    LaunchedEffect(target, client) { load() }

    val accounts: List<Account> = when (target) {
        is AccountsTarget.Cloud -> {
            val agent = target.agent
            val logins = cloud.accounts[agent.login].orEmpty().map { a ->
                val key = "${agent.login}:${a.slot}"
                val usage = cloud.usage[key]
                // The live plan from the usage reply beats the sign-in's.
                Account(
                    id = key,
                    title = a.email ?: a.displayName ?: "${agent.name} account",
                    detail = listOfNotNull(usage?.plan ?: a.plan, a.organization).joinToString(" · ").ifEmpty { null },
                    usage = usage?.windows.orEmpty(),
                    note = if (a.needsReconnect) "Sign in again" else if (usage?.windows?.isNotEmpty() == true) null else cloud.usageErrors[key],
                    active = a.active, switchable = true, isKey = false, cloudProvider = agent.login,
                )
            }
            val keys = cloud.accounts[agent.key].orEmpty().map { a ->
                Account("${agent.key}:${a.slot}", agent.keyName, null, emptyList(), null, a.active, true, true, agent.key)
            }
            logins + keys
        }
        is AccountsTarget.Device -> deviceAccounts.orEmpty().filter { it.harness == target.harness }.map { a ->
            val option = a.provider?.let { p -> if (rules.options.size > 1) rules.options.firstOrNull { it.provider == p }?.label else null }
            Account(
                id = a.id,
                title = deviceAccountTitle(a),
                detail = listOfNotNull(a.planLabel, a.organization, option).joinToString(" · ").ifEmpty { null },
                usage = if (rules.reportsUsage) a.usage else emptyList(),
                note = a.usageError,
                active = a.active,
                switchable = a.switchable && rules.switches,
                isKey = a.apiKey,
                cloudProvider = null,
            )
        }
    }

    /** The list's last rows: connect the first account or add another, per option; Cloud also takes an API key. */
    val addRows: List<Pair<String, LoginOptionInfo?>> = buildList {
        if (accounts.isEmpty() || !rules.keepsOneLogin) {
            rules.options.forEach { add((if (accounts.isEmpty()) it.firstTitle else it.moreTitle) to it) }
        }
        if (target is AccountsTarget.Cloud) add("Add ${target.agent.keyName}" to null)
    }

    fun cloudAccount(a: Account) = a.cloudProvider?.let { p -> cloud.accounts[p]?.firstOrNull { "$p:${it.slot}" == a.id } }

    fun run(key: String, work: suspend () -> Unit) {
        if (busy != null) return
        busy = key
        scope.launch {
            try {
                work()
                view.performHapticFeedback(HapticFeedbackConstants.CLOCK_TICK)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                model.toast(CloudStore.message(e))
            } finally {
                busy = null
            }
        }
    }

    fun activate(a: Account) {
        if (a.active || !a.switchable) return
        val c = client ?: return
        run(a.id) {
            when (target) {
                is AccountsTarget.Cloud -> { cloudAccount(a)?.let { cloud.activate(it, a.cloudProvider!!) } }
                is AccountsTarget.Device -> { deviceAccounts = c.deviceActivateAccount(target.deviceId, target.harness, a.id) }
            }
        }
    }

    fun remove(a: Account) {
        val c = client ?: return
        run(a.id) {
            when (target) {
                is AccountsTarget.Cloud -> { cloudAccount(a)?.let { cloud.remove(it, a.cloudProvider!!) } }
                is AccountsTarget.Device -> { deviceAccounts = c.deviceForgetAccount(target.deviceId, target.harness, a.id) }
            }
        }
    }

    /** Asks for the code the sign-in page shows (Claude's paste fallback). */
    suspend fun askForCode(): String? {
        val request = CompletableDeferred<String?>()
        codeRequest = request
        return try {
            request.await()
        } finally {
            codeRequest = null
        }
    }

    /** A computer's sign-in: started there, finished in the browser here; the core relays the return. */
    suspend fun deviceSignIn(t: AccountsTarget.Device, option: LoginOptionInfo) {
        val c = client ?: throw CoreException.Closed()
        val done = BrowserReturns.SIGNED_IN
        val start = c.deviceLoginStart(t.deviceId, t.harness, option.provider, done)
        try {
            if (start.pasteCode) {
                model.browser.open(context, start.url)
                val code = askForCode() ?: throw CancellationException("No code")
                deviceAccounts = c.deviceLoginComplete(t.deviceId, start.loginId, code)
                return
            }
            var opened = false
            if (start.url.isNotEmpty()) {
                model.browser.open(context, start.url)
                opened = true
            }
            val deadline = System.currentTimeMillis() + 10 * 60_000
            while (System.currentTimeMillis() < deadline) {
                delay(1_500)
                val poll = c.deviceLoginPoll(t.deviceId, start.loginId, done)
                when (poll.status) {
                    "done" -> {
                        load()
                        return
                    }
                    "error" -> throw IllegalStateException(poll.message ?: "The sign-in failed.")
                    else -> {
                        val url = poll.url
                        if (url != null && !opened) {
                            model.browser.open(context, url)
                            opened = true
                        }
                        poll.message?.let { note ->
                            signing = signing?.copy(message = note)
                            // "Enter the code ABCD-1234 when asked": have it ready.
                            note.split(' ').firstOrNull { it.contains('-') && it.length >= 7 }?.let { code ->
                                context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("Code", code.trim('.', ',')))
                            }
                        }
                    }
                }
            }
            throw IllegalStateException("The sign-in timed out.")
        } catch (e: Throwable) {
            // Called off (here or by leaving the page), failed or timed out: end it on the computer too.
            runCatching { c.deviceLoginCancel(t.deviceId, start.loginId) }
            throw e
        }
    }

    fun startSignIn(option: LoginOptionInfo) {
        if (signing != null || busy != null) return
        signing = SignInState(option.provider ?: "")
        signJob = scope.launch {
            try {
                when (target) {
                    is AccountsTarget.Cloud -> model.signInToCloud(context, target.agent)
                    is AccountsTarget.Device -> deviceSignIn(target, option)
                }
                view.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
            } catch (e: CancellationException) {
                // Closed or cancelled.
            } catch (e: Exception) {
                model.toast(CloudStore.message(e))
            } finally {
                signing = null
                signJob = null
            }
        }
    }

    // Open straight into this provider's sign-in (an error card's "Log in").
    LaunchedEffect(Unit) { if (signIn) rules.options.firstOrNull()?.let(::startSignIn) }
    // Leaving the page ends a computer's sign-in rather than polling behind it.
    DisposableEffect(Unit) { onDispose { signJob?.cancel() } }

    CloudPage(title, deviceName, onBack) {
        val footer = listOfNotNull(
            rules.note,
            when (target) {
                is AccountsTarget.Cloud -> "Cloud sessions use the account in use. Sign-ins and keys are kept in Cloud, never on this phone."
                is AccountsTarget.Device -> "Account changes affect new sessions on ${target.name}. Signing in runs on ${target.name}; you finish in the browser here."
            },
        ).joinToString("\n\n")
        cloudSection("Accounts on $deviceName", footer = footer) {
            val placeholder: Pair<String, String?>? = when {
                target is AccountsTarget.Device && deviceAccounts == null ->
                    (if (loadError == null) "Loading accounts…" else "Couldn't load accounts") to loadError
                loadError != null && accounts.isEmpty() -> "Couldn't load accounts" to loadError
                else -> null
            }
            val count = (if (placeholder != null) 1 else accounts.size) + addRows.size + (if (signing != null) 1 else 0)
            var i = 0
            if (placeholder != null) {
                val (text, detail) = placeholder
                CloudRow(
                    i++, count, text, detail,
                    leading = {
                        if (detail == null) LoadingIndicator(Modifier.size(32.dp))
                        else IconTile(ZIcons.Warning, MaterialTheme.colorScheme.errorContainer, MaterialTheme.colorScheme.onErrorContainer)
                    },
                )
            } else {
                for (a in accounts) {
                    AccountRow(
                        a, target.harness, i++, count,
                        busy = busy == a.id,
                        onSwitch = { activate(a) },
                        onRemove = if (a.switchable && busy == null) ({ removing = a }) else null,
                    )
                }
            }
            val disabled = signing != null || busy != null
            for ((label, option) in addRows) {
                CloudRow(
                    i++, count, label,
                    leading = { IconTile(ZIcons.Plus, chipContainer(), MaterialTheme.colorScheme.onSurfaceVariant) },
                    trailing = {},
                    enabled = !disabled,
                    titleColor = if (disabled) MaterialTheme.colorScheme.outline else Color.Unspecified,
                    onClick = {
                        if (option == null) askKey = true else startSignIn(option)
                    },
                )
            }
            signing?.let { s ->
                // A computer's sign-in can be called off from here; a Cloud one ends with its browser page.
                val device = target is AccountsTarget.Device
                CloudRow(
                    i, count, "Finish signing in in the browser…",
                    listOfNotNull(s.message, if (device) "Tap to cancel" else null).joinToString(" · ").ifEmpty { null },
                    leading = { LoadingIndicator(Modifier.size(32.dp)) },
                    trailing = {},
                    onClick = if (device) ({ signJob?.cancel() }) else null,
                )
            }
        }
    }

    if (askKey && target is AccountsTarget.Cloud) {
        ApiKeyDialog(target.agent, onDismiss = { askKey = false }) { key ->
            askKey = false
            run("api-key") { cloud.saveKey(key, target.agent) }
        }
    }
    codeRequest?.let { request ->
        PasteCodeDialog(onDismiss = { request.complete(null) }, onCode = { request.complete(it) })
    }
    removing?.let { a ->
        AlertDialog(
            onDismissRequest = { removing = null },
            icon = { ZIcon(ZIcons.Delete, null) },
            title = { Text("Remove account?") },
            text = {
                Text(
                    if (target is AccountsTarget.Cloud) "“${a.title}” will be removed from Cloud."
                    else "“${a.title}” will be signed out on $deviceName.",
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    removing = null
                    remove(a)
                }) { Text("Remove", color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { TextButton(onClick = { removing = null }) { Text("Cancel") } },
        )
    }
}

/** One account: mark, name and plan with "In use" or Switch beside them, and the plan's usage underneath. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun AccountRow(a: Account, harness: String, index: Int, count: Int, busy: Boolean, onSwitch: () -> Unit, onRemove: (() -> Unit)?) {
    val view = LocalView.current
    var menu by remember { mutableStateOf(false) }
    val fixed = !a.active && !a.switchable
    Box {
        SegmentedListItem(
            onClick = { if (!a.active && a.switchable && !busy) onSwitch() },
            onLongClick = if (onRemove != null) ({
                view.performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
                menu = true
            }) else null,
            shapes = segmentedShapes(index, count),
            colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
            leadingContent = {
                if (a.isKey) IconTile(ZIcons.Key, chipContainer(), MaterialTheme.colorScheme.onSurface) else AgentTile(harness)
            },
            supportingContent = if (a.detail != null || a.usage.isNotEmpty() || a.note != null) {
                {
                    Column {
                        a.detail?.let { Text(it, maxLines = 2, overflow = TextOverflow.Ellipsis) }
                        if (a.usage.isNotEmpty()) {
                            Spacer(Modifier.height(10.dp))
                            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                                a.usage.take(3).forEach { UsageBar(it) }
                            }
                        }
                        a.note?.let {
                            Spacer(Modifier.height(6.dp))
                            Text(it, style = MaterialTheme.typography.bodySmall, color = Color(0xFFD97706))
                        }
                    }
                }
            } else {
                null
            },
            trailingContent = {
                when {
                    busy -> LoadingIndicator(Modifier.size(32.dp))
                    a.active -> InUseBadge()
                    a.switchable -> FilledTonalButton(onClick = onSwitch) { Text("Switch") }
                }
            },
        ) {
            Text(
                a.title,
                maxLines = 1,
                overflow = TextOverflow.MiddleEllipsis,
                color = if (fixed) MaterialTheme.colorScheme.outline else MaterialTheme.colorScheme.onSurface,
            )
        }
        Box(Modifier.align(Alignment.TopEnd)) {
            ActionMenu(
                menu,
                { menu = false },
                listOfNotNull(
                    if (!a.active && a.switchable) MenuAction("Switch to this account", ZIcons.Refresh) { onSwitch() } else null,
                    onRemove?.let { MenuAction("Remove account", ZIcons.Delete, destructive = true) { it() } },
                ),
            )
        }
    }
}

@Composable
private fun InUseBadge() {
    val tone = successColor()
    Surface(shape = RoundedCornerShape(8.dp), color = tone.copy(alpha = 0.12f), contentColor = tone) {
        Text("In use", style = MaterialTheme.typography.labelLarge, modifier = Modifier.padding(horizontal = 8.dp, vertical = 3.dp))
    }
}

/** Cloud sessions use this key instead of a subscription sign-in. */
@Composable
private fun ApiKeyDialog(agent: CloudStore.Agent, onDismiss: () -> Unit, onSave: (String) -> Unit) {
    var key by remember { mutableStateOf("") }
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    AlertDialog(
        onDismissRequest = onDismiss,
        icon = { ZIcon(ZIcons.Key, null) },
        title = { Text(agent.keyName) },
        text = {
            Column {
                Text("Cloud sessions use this key instead of a subscription sign-in.", style = MaterialTheme.typography.bodyMedium)
                Spacer(Modifier.height(12.dp))
                OutlinedTextField(
                    key, { key = it.trim() },
                    singleLine = true,
                    placeholder = { Text(if (agent.key == "anthropic-key") "sk-ant-…" else "sk-…") },
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false),
                    shape = RoundedCornerShape(16.dp),
                    modifier = Modifier.fillMaxWidth().focusRequester(focus),
                )
            }
        },
        confirmButton = { TextButton(onClick = { onSave(key) }, enabled = key.isNotEmpty()) { Text("Save key") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** Claude's paste fallback: the code the sign-in page shows. */
@Composable
private fun PasteCodeDialog(onDismiss: () -> Unit, onCode: (String) -> Unit) {
    var code by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Paste the code") },
        text = {
            Column {
                Text("Copy the code the sign-in page shows, then paste it here.", style = MaterialTheme.typography.bodyMedium)
                Spacer(Modifier.height(12.dp))
                OutlinedTextField(
                    code, { code = it },
                    singleLine = true,
                    placeholder = { Text("Authorization code") },
                    keyboardOptions = KeyboardOptions(autoCorrectEnabled = false),
                    shape = RoundedCornerShape(16.dp),
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        },
        confirmButton = { TextButton(onClick = { onCode(code.trim()) }, enabled = code.isNotBlank()) { Text("Continue") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
