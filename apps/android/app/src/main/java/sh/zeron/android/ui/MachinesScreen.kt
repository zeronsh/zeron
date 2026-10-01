package sh.zeron.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch
import sh.zeron.android.core.Machine
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.BackChevron
import sh.zeron.android.design.LocalZeronColors
import sh.zeron.android.design.PlusMark
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronType
import sh.zeron.android.design.glassSurface
import uniffi.zeron_core.ConnectivityState
import uniffi.zeron_core.SshException

/** Saved SSH machines + the edge "Zeron Cloud" account + Demo. */
@Composable
fun MachinesScreen(model: ZeronModel) {
    val colors = LocalZeronColors.current
    val clipboard = LocalClipboardManager.current
    LaunchedEffect(model.machines) { model.probeMachines() }
    val phoneKey = remember { model.phonePublicKey() }
    Column(Modifier.fillMaxSize().background(colors.background).statusBarsPadding().navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(44.dp).glassSurface(colors, 22.dp).clickable { model.back(); if (model.phase !is ZeronModel.Phase.Ready) model.showMachines = false }, contentAlignment = Alignment.Center) {
                BackChevron(colors.text, Modifier.size(18.dp))
            }
            Spacer(Modifier.width(10.dp))
            Text("Machines", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp, modifier = Modifier.weight(1f))
            Box(Modifier.size(44.dp).glassSurface(colors, 22.dp).clickable { model.editMachine = Machine() }, contentAlignment = Alignment.Center) {
                PlusMark(colors.text, Modifier.size(18.dp))
            }
        }
        LazyColumn(Modifier.weight(1f).padding(horizontal = 16.dp)) {
            item { GroupLabel(colors, "Your computers (SSH)") }
            if (model.machines.isEmpty()) {
                item {
                    SettingRow(colors, "Add a computer", "Connect straight to Zeron on your PC over SSH: no cloud account needed.", onClick = { model.editMachine = Machine() })
                }
            }
            items(model.machines, key = { it.id }) { machine ->
                val active = model.activeMachine == machine.id
                val link = if (active) model.directStatus else null
                val online: Boolean? = when {
                    active && link?.phase == uniffi.zeron_core.DirectPhase.LIVE -> true
                    active && link?.phase == uniffi.zeron_core.DirectPhase.FAILED -> false
                    active -> null
                    else -> model.machineOnline[machine.id]
                }
                val subtitle = buildString {
                    append("${machine.user}@${machine.host}:${machine.port}")
                    if (active) {
                        val summary = directSummary(link)
                        if (link?.phase == uniffi.zeron_core.DirectPhase.FAILED) append("\n$summary") else append("  ·  $summary")
                    } else if (machine.hostKey == null) {
                        append("  ·  not verified yet")
                    }
                }
                MachineRow(colors, machine.title(), subtitle, online, active, onClick = {
                    if (machine.hostKey == null) model.editMachine = machine else model.connectMachine(machine)
                }, onEdit = { model.editMachine = machine })
            }
            item { GroupLabel(colors, "Other workspaces") }
            item {
                val active = model.activeMachine == "cloud"
                MachineRow(
                    colors,
                    "Zeron Cloud",
                    if (active && model.client != null) "Signed in · ${model.client?.orgId()}" else "Your Zeron account, through the relay",
                    online = if (active) model.connectivity?.state == ConnectivityState.CONNECTED else null,
                    active = active,
                    onClick = { model.useCloud() },
                    onEdit = null,
                )
            }
            item {
                MachineRow(colors, "Demo", "Offline sample workspace", online = null, active = model.activeMachine == "demo", onClick = { model.enterDemo() }, onEdit = null)
            }
            item { GroupLabel(colors, "This phone's SSH key") }
            item {
                Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(colors.elevated).padding(14.dp)) {
                    Text(phoneKey, color = colors.text, fontFamily = ZeronType.Mono, fontSize = 12.sp)
                    Spacer(Modifier.height(10.dp))
                    Row {
                        Pill(colors, "Copy public key") {
                            clipboard.setText(AnnotatedString(phoneKey))
                            model.showToast("Public key copied")
                        }
                    }
                    Spacer(Modifier.height(8.dp))
                    Text("Add it to administrators_authorized_keys on Windows (see docs/ssh-direct.md).", color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 12.sp)
                }
            }
            item { Spacer(Modifier.height(32.dp)) }
        }
    }
}

@Composable
private fun MachineRow(colors: ZeronColors, title: String, subtitle: String, online: Boolean?, active: Boolean, onClick: () -> Unit, onEdit: (() -> Unit)?) {
    Row(
        Modifier.fillMaxWidth().padding(vertical = 3.dp).clip(RoundedCornerShape(14.dp)).background(colors.elevated).clickable(onClick = onClick).padding(start = 14.dp, end = 6.dp, top = 12.dp, bottom = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        StatusDot(colors, online)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(title, color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 16.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(subtitle, color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp, maxLines = 3, overflow = TextOverflow.Ellipsis)
        }
        if (active) Text("✓", color = colors.accent, fontSize = 16.sp, modifier = Modifier.padding(horizontal = 8.dp))
        if (onEdit != null) {
            Text("Edit", color = colors.accent, fontFamily = ZeronType.Sans, fontSize = 14.sp, modifier = Modifier.clip(RoundedCornerShape(10.dp)).clickable(onClick = onEdit).padding(horizontal = 8.dp, vertical = 6.dp))
        }
    }
}

@Composable
private fun StatusDot(colors: ZeronColors, online: Boolean?) {
    val m = Modifier.size(10.dp).clip(CircleShape)
    when (online) {
        true -> Box(m.background(colors.success))
        false -> Box(m.background(colors.tertiary))
        null -> Box(m.border(1.5.dp, colors.tertiary, CircleShape))
    }
}

@Composable
internal fun Pill(colors: ZeronColors, label: String, enabled: Boolean = true, primary: Boolean = false, onClick: () -> Unit) {
    Text(
        label,
        color = if (primary) colors.background else colors.text.copy(alpha = if (enabled) 1f else 0.4f),
        fontFamily = ZeronType.Sans,
        fontWeight = FontWeight.Medium,
        fontSize = 14.sp,
        modifier = Modifier.clip(RoundedCornerShape(18.dp)).background(if (primary) colors.text else colors.controlFill)
            .then(if (enabled) Modifier.clickable(onClick = onClick) else Modifier)
            .padding(horizontal = 14.dp, vertical = 8.dp),
    )
}

private sealed interface TestState {
    data object Idle : TestState
    data object Running : TestState
    data class Ok(val message: String) : TestState
    data class Failed(val message: String) : TestState
}

private sealed interface HostPrompt {
    data class Unknown(val algorithm: String, val fingerprint: String) : HostPrompt
    data class Changed(val algorithm: String, val expected: String, val actual: String) : HostPrompt
}

/** Add / edit one machine: fields, Test (with the host-key confirmation), Save. */
@Composable
fun MachineEditScreen(model: ZeronModel, initial: Machine) {
    val colors = LocalZeronColors.current
    val clipboard = LocalClipboardManager.current
    val scope = rememberCoroutineScope()
    val existing = model.machines.any { it.id == initial.id }
    var name by remember(initial.id) { mutableStateOf(initial.name) }
    var host by remember(initial.id) { mutableStateOf(initial.host) }
    var port by remember(initial.id) { mutableStateOf(initial.port.toString()) }
    var user by remember(initial.id) { mutableStateOf(initial.user) }
    var auth by remember(initial.id) { mutableStateOf(initial.auth) }
    var secret by remember(initial.id) { mutableStateOf("") }
    var enginePort by remember(initial.id) { mutableStateOf(initial.enginePort.toString()) }
    var hostKey by remember(initial.id) { mutableStateOf(initial.hostKey) }
    var test by remember(initial.id) { mutableStateOf<TestState>(TestState.Idle) }
    var prompt by remember { mutableStateOf<HostPrompt?>(null) }
    var confirmDelete by remember { mutableStateOf(false) }
    val phoneKey = remember { model.phonePublicKey() }

    fun draft() = initial.copy(
        name = name.trim(),
        host = host.trim(),
        port = port.toIntOrNull() ?: 22,
        user = user.trim(),
        auth = auth,
        enginePort = enginePort.toIntOrNull() ?: 27654,
        hostKey = hostKey,
    )
    val secretArg: String? = secret.takeIf { it.isNotEmpty() && auth != Machine.AUTH_PHONE }
    val valid = host.isNotBlank() && user.isNotBlank() && (port.toIntOrNull() ?: 0) in 1..65535 &&
        (auth == Machine.AUTH_PHONE || secret.isNotEmpty() || (existing && initial.auth == auth))

    fun runTest(pin: String?, then: (() -> Unit)? = null) {
        test = TestState.Running
        scope.launch {
            try {
                val probe = model.testMachine(draft(), secretArg, pin)
                hostKey = probe.hostKeyFingerprint
                test = TestState.Ok("Connected · Zeron ${probe.engineVersion ?: "engine"} answered in ${probe.latencyMs} ms")
                then?.invoke()
            } catch (e: SshException.HostKeyUnknown) {
                test = TestState.Idle
                prompt = HostPrompt.Unknown(e.algorithm, e.fingerprint)
            } catch (e: SshException.HostKeyMismatch) {
                test = TestState.Failed("The host key changed!")
                prompt = HostPrompt.Changed(e.algorithm, e.expected, e.actual)
            } catch (t: Throwable) {
                test = TestState.Failed(t.message ?: "Couldn't connect")
            }
        }
    }

    fun save(connect: Boolean) {
        val m = draft()
        model.saveMachine(m, secretArg)
        model.editMachine = null
        if (connect) model.connectMachine(m) else model.probeMachines()
    }

    Column(Modifier.fillMaxSize().background(colors.background).statusBarsPadding().navigationBarsPadding().imePadding()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("Cancel", color = colors.text, fontFamily = ZeronType.Sans, fontSize = 16.sp, modifier = Modifier.clip(RoundedCornerShape(12.dp)).clickable { model.editMachine = null }.padding(8.dp))
            Text(if (existing) "Edit Machine" else "Add Machine", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp, modifier = Modifier.weight(1f), textAlign = androidx.compose.ui.text.style.TextAlign.Center)
            Text(
                "Save",
                color = if (valid) colors.accent else colors.tertiary,
                fontFamily = ZeronType.Sans,
                fontWeight = FontWeight.SemiBold,
                fontSize = 16.sp,
                modifier = Modifier.clip(RoundedCornerShape(12.dp)).clickable(enabled = valid) {
                    if (hostKey == null) runTest(null) { save(connect = false) } else save(connect = false)
                }.padding(8.dp),
            )
        }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp)) {
            GroupLabel(colors, "Machine")
            Field(colors, "Name", name, "My PC") { name = it }
            Field(colors, "Host", host, "192.168.1.20 or pc.local", keyboard = KeyboardType.Uri) { host = it; hostKey = if (it.trim() == initial.host) initial.hostKey else null }
            Row {
                Box(Modifier.weight(1f)) { Field(colors, "SSH port", port, "22", keyboard = KeyboardType.Number) { port = it.filter(Char::isDigit).take(5) } }
                Spacer(Modifier.width(8.dp))
                Box(Modifier.weight(1f)) { Field(colors, "Zeron port", enginePort, "27654", keyboard = KeyboardType.Number) { enginePort = it.filter(Char::isDigit).take(5) } }
            }
            Field(colors, "User", user, "Windows user name") { user = it }
            GroupLabel(colors, "Sign in with")
            Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(colors.controlFill).padding(3.dp)) {
                listOf(Machine.AUTH_PHONE to "This phone's key", Machine.AUTH_KEY to "Import key", Machine.AUTH_PASSWORD to "Password").forEach { (kind, label) ->
                    val on = auth == kind
                    Box(
                        Modifier.weight(1f).clip(RoundedCornerShape(10.dp)).background(if (on) colors.elevated else Color.Transparent).clickable { auth = kind; secret = "" }.padding(vertical = 9.dp),
                        contentAlignment = Alignment.Center,
                    ) { Text(label, color = if (on) colors.text else colors.secondary, fontFamily = ZeronType.Sans, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal, fontSize = 13.sp) }
                }
            }
            Spacer(Modifier.height(8.dp))
            when (auth) {
                Machine.AUTH_PHONE -> Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(colors.elevated).padding(14.dp)) {
                    Text("Public key: add this line to the machine's authorized keys", color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp)
                    Spacer(Modifier.height(6.dp))
                    Text(phoneKey, color = colors.text, fontFamily = ZeronType.Mono, fontSize = 12.sp)
                    Spacer(Modifier.height(10.dp))
                    Pill(colors, "Copy public key") {
                        clipboard.setText(AnnotatedString(phoneKey))
                        model.showToast("Public key copied")
                    }
                }
                Machine.AUTH_KEY -> Field(colors, "Private key", secret, if (existing && initial.auth == auth) "Saved (paste to replace)" else "-----BEGIN OPENSSH PRIVATE KEY-----", multiline = true, mono = true) { secret = it }
                else -> Field(colors, "Password", secret, if (existing && initial.auth == auth) "Saved (type to replace)" else "Password", password = true) { secret = it }
            }
            GroupLabel(colors, "Connection")
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(colors.elevated).padding(14.dp)) {
                Text(
                    hostKey?.let { "Host key: $it" } ?: "Host key not verified yet: Test to see it and trust it.",
                    color = colors.secondary,
                    fontFamily = if (hostKey != null) ZeronType.Mono else ZeronType.Sans,
                    fontSize = 12.sp,
                )
                Spacer(Modifier.height(10.dp))
                when (val t = test) {
                    TestState.Running -> Text("Testing…", color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 14.sp)
                    is TestState.Ok -> Row(verticalAlignment = Alignment.CenterVertically) {
                        Box(Modifier.size(8.dp).clip(CircleShape).background(colors.success))
                        Spacer(Modifier.width(8.dp))
                        Text(t.message, color = colors.text, fontFamily = ZeronType.Sans, fontSize = 14.sp)
                    }
                    is TestState.Failed -> Text(t.message, color = colors.danger, fontFamily = ZeronType.Sans, fontSize = 14.sp)
                    TestState.Idle -> Unit
                }
                Spacer(Modifier.height(10.dp))
                Row {
                    Pill(colors, "Test", enabled = valid && test != TestState.Running) { runTest(hostKey) }
                    Spacer(Modifier.width(8.dp))
                    Pill(colors, "Save & Connect", enabled = valid && test != TestState.Running, primary = true) {
                        if (hostKey == null) runTest(null) { save(connect = true) } else save(connect = true)
                    }
                }
            }
            if (existing) {
                Spacer(Modifier.height(18.dp))
                SettingRow(colors, "Delete Machine", null, destructive = true, onClick = { confirmDelete = true })
            }
            Spacer(Modifier.height(40.dp))
        }
    }

    when (val p = prompt) {
        is HostPrompt.Unknown -> AlertDialog(
            onDismissRequest = { prompt = null },
            title = { Text("Trust this machine?") },
            text = {
                Column {
                    Text("First connection to ${host.trim()}:${port}. Check that the fingerprint matches the machine before trusting it.", fontFamily = ZeronType.Sans, fontSize = 14.sp)
                    Spacer(Modifier.height(10.dp))
                    Text(p.algorithm, fontFamily = ZeronType.Mono, fontSize = 12.sp)
                    Text(p.fingerprint, fontFamily = ZeronType.Mono, fontSize = 13.sp)
                    Spacer(Modifier.height(10.dp))
                    Text("On Windows: ssh-keygen -lf C:\\ProgramData\\ssh\\ssh_host_ed25519_key.pub", fontFamily = ZeronType.Mono, fontSize = 11.sp)
                }
            },
            confirmButton = { TextButton(onClick = { prompt = null; hostKey = p.fingerprint; runTest(p.fingerprint) }) { Text("Trust") } },
            dismissButton = { TextButton(onClick = { prompt = null }) { Text("Cancel") } },
        )
        is HostPrompt.Changed -> AlertDialog(
            onDismissRequest = { prompt = null },
            title = { Text("Host key changed") },
            text = {
                Column {
                    Text("The machine presented a different key than the one you trusted. This happens after reinstalling OpenSSH, but it can also mean someone is intercepting the connection.", fontFamily = ZeronType.Sans, fontSize = 14.sp)
                    Spacer(Modifier.height(10.dp))
                    Text("Trusted: ${p.expected}", fontFamily = ZeronType.Mono, fontSize = 12.sp)
                    Text("Now:     ${p.actual}", fontFamily = ZeronType.Mono, fontSize = 12.sp)
                }
            },
            confirmButton = { TextButton(onClick = { prompt = null; hostKey = p.actual; runTest(p.actual) }) { Text("Trust new key", color = colors.danger) } },
            dismissButton = { TextButton(onClick = { prompt = null }) { Text("Cancel") } },
        )
        null -> Unit
    }
    if (confirmDelete) {
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            title = { Text("Delete ${initial.title()}?") },
            text = { Text("Its saved key or password is removed from this phone.") },
            confirmButton = { TextButton(onClick = { confirmDelete = false; model.editMachine = null; model.deleteMachine(initial.id) }) { Text("Delete", color = colors.danger) } },
            dismissButton = { TextButton(onClick = { confirmDelete = false }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun Field(
    colors: ZeronColors,
    label: String,
    value: String,
    placeholder: String,
    keyboard: KeyboardType = KeyboardType.Text,
    password: Boolean = false,
    multiline: Boolean = false,
    mono: Boolean = false,
    onChange: (String) -> Unit,
) {
    Column(Modifier.padding(vertical = 4.dp)) {
        Text(label, color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 12.sp, modifier = Modifier.padding(start = 4.dp, bottom = 3.dp))
        BasicTextField(
            value = value,
            onValueChange = onChange,
            singleLine = !multiline,
            keyboardOptions = KeyboardOptions(keyboardType = if (password) KeyboardType.Password else keyboard, autoCorrectEnabled = false),
            visualTransformation = if (password) PasswordVisualTransformation() else VisualTransformation.None,
            textStyle = TextStyle(color = colors.text, fontFamily = if (mono) ZeronType.Mono else ZeronType.Sans, fontSize = if (mono) 12.sp else 16.sp),
            cursorBrush = SolidColor(colors.accent),
            modifier = Modifier.fillMaxWidth().then(if (multiline) Modifier.heightIn(min = 110.dp) else Modifier).clip(RoundedCornerShape(12.dp)).background(colors.controlFill).padding(horizontal = 14.dp, vertical = 12.dp),
            decorationBox = { inner ->
                Box {
                    if (value.isEmpty()) Text(placeholder, color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = if (mono) 12.sp else 16.sp)
                    inner()
                }
            },
        )
    }
}
