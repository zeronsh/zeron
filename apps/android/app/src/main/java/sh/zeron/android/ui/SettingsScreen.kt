package sh.zeron.android.ui

import sh.zeron.android.feedback.OpenCloseFeedback
import sh.zeron.android.feedback.tapAction
import sh.zeron.android.feedback.toggleAction
import sh.zeron.android.feedback.feedbackAction
import sh.zeron.android.feedback.LocalFeedback
import sh.zeron.android.feedback.Haptic
import sh.zeron.android.feedback.Cue
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ButtonGroupDefaults
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.ToggleButton
import androidx.compose.material3.ToggleButtonDefaults
import androidx.compose.material3.toShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext
import sh.zeron.android.core.Account
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.DeviceIdentity
import sh.zeron.android.core.SignIn
import sh.zeron.runtime.CustomServer
import sh.zeron.runtime.RuntimeState
import sh.zeron.android.design.ThemeMode
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.coreVersion
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SettingsScreen(model: AppModel, onOpen: (String) -> Unit) {
    val appearance by model.appearance.collectAsState()
    val account by model.account.collectAsState()
    val signIn by model.signIn.collectAsState()
    // Kept composed behind the Sessions tab: while hidden it listens to nothing (see TabPage).
    val engine = model.phone.state.collectAsStateWhile()
    val developer by model.developer.collectAsState()
    val client = model.client.collectAsStateWhile()
    val demo = client?.isDemo() == true
    val workspace = model.workspace.collectAsStateWhile()
    // This phone first, like the desktop's device list.
    val devices = workspace?.devices.orEmpty().sortedByDescending { it.isSelf }
    val context = LocalContext.current
    val ask = rememberPermissionAsk()
    var editingServer by remember { mutableStateOf(false) }
    var versionTaps by remember { mutableIntStateOf(0) }
    val orgs by model.orgChoice.collectAsState()
    val list = androidx.compose.foundation.lazy.rememberLazyListState()
    val fb = LocalFeedback.current
    Box(Modifier.fillMaxSize()) {
    LazyColumn(
        Modifier.fillMaxSize(),
        state = list,
        contentPadding = PaddingValues(top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 8.dp, bottom = 140.dp),
    ) {
        item { ScreenHeader("Settings", null) }
        item {
            // Account: a tonal hero card with a shaped monogram.
            Surface(
                shape = RoundedCornerShape(32.dp),
                color = MaterialTheme.colorScheme.primaryContainer,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
            ) {
                Row(Modifier.padding(20.dp), verticalAlignment = Alignment.CenterVertically) {
                    Box(
                        Modifier.size(64.dp).clip(MaterialShapes.Cookie9Sided.toShape()).background(MaterialTheme.colorScheme.primary),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(model.accountName.take(1).uppercase(), style = MaterialTheme.typography.headlineSmallEmphasized, color = MaterialTheme.colorScheme.onPrimary)
                    }
                    Spacer(Modifier.width(16.dp))
                    Column {
                        Text(model.accountName, style = MaterialTheme.typography.titleLargeEmphasized, color = MaterialTheme.colorScheme.onPrimaryContainer)
                        Text(model.accountDetail, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onPrimaryContainer.copy(alpha = 0.8f))
                    }
                }
            }
        }
        if (!demo) {
            item { AccountRows(model, account, signIn, onSignIn = {
                ask(false) {
                    model.signIn { url ->
                        androidx.browser.customtabs.CustomTabsIntent.Builder().setShowTitle(true).build()
                            .launchUrl(context, android.net.Uri.parse(url))
                    }
                }
            }, onServer = { editingServer = true }) }
            section("This phone")
            item {
                // This phone is one of the account's devices: its engine, its agents.
                Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                    SegmentedListItem(
                        onClick = tapAction { onOpen(Routes.ENGINE) },
                        shapes = segmentedShapes(0, 2),
                        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                        leadingContent = { IconTile(ZIcons.Phone) },
                        supportingContent = { Text(engineStatusLine(engine), maxLines = 2) },
                        trailingContent = {
                            Box(
                                Modifier.size(10.dp).clip(CircleShape).background(
                                    when (engine) {
                                        is RuntimeState.Running -> successColor()
                                        is RuntimeState.Failed -> MaterialTheme.colorScheme.error
                                        else -> MaterialTheme.colorScheme.outlineVariant
                                    },
                                ),
                            )
                        },
                    ) { Text("Engine") }
                    SegmentedListItem(
                        onClick = tapAction { onOpen(Routes.AGENTS) },
                        shapes = segmentedShapes(1, 2),
                        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                        leadingContent = { IconTile(ZIcons.Bot) },
                        supportingContent = { Text("Install agents and sign in to their accounts") },
                        trailingContent = { ZIcon(ZIcons.ChevronRight, null, Modifier.size(20.dp)) },
                    ) { Text("Coding agents") }
                }
            }
            section("Files")
            item {
                // Transfers go through this phone's engine (docs/android.md § File transfers).
                val transfers = model.transfers.list.collectAsStateWhile()
                val live = transfers.count { it.state.live }
                Column(Modifier.padding(horizontal = 16.dp)) {
                    SegmentedListItem(
                        onClick = tapAction { onOpen(Routes.TRANSFERS) },
                        shapes = segmentedShapes(0, 1),
                        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                        leadingContent = { IconTile(ZIcons.ArrowDown) },
                        supportingContent = {
                            Text(if (live > 0) "$live in progress" else "Send and receive files with your other devices")
                        },
                        trailingContent = { ZIcon(ZIcons.ChevronRight, null, Modifier.size(20.dp)) },
                    ) { Text("Transfers") }
                }
            }
        }
        section("Feedback")
        item {
            Column(Modifier.padding(horizontal = 16.dp)) {
                SegmentedListItem(
                    onClick = tapAction { onOpen(Routes.SOUNDS) },
                    shapes = segmentedShapes(0, 1),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Volume) },
                    supportingContent = { Text("Chimes, interface sounds, volume and vibration") },
                    trailingContent = { ZIcon(ZIcons.ChevronRight, null, Modifier.size(20.dp)) },
                ) { Text("Sounds & haptics") }
            }
        }
        section("Appearance")
        item {
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                Surface(shape = segmentedShapes(0, 2).shape, color = cardColor()) {
                    Column(Modifier.padding(16.dp)) {
                        Text("Theme", style = MaterialTheme.typography.titleMedium)
                        Spacer(Modifier.height(12.dp))
                        Row(horizontalArrangement = Arrangement.spacedBy(ButtonGroupDefaults.ConnectedSpaceBetween)) {
                            val modes = listOf(
                                Triple(ThemeMode.System, "System", ZIcons.Monitor),
                                Triple(ThemeMode.Light, "Light", ZIcons.Sun),
                                Triple(ThemeMode.Dark, "Dark", ZIcons.Moon),
                            )
                            modes.forEachIndexed { index, (mode, label, icon) ->
                                ToggleButton(
                                    checked = appearance.mode == mode,
                                    onCheckedChange = {
                                        if (appearance.mode != mode) fb.both(Haptic.Select, Cue.Select)
                                        model.setAppearance(appearance.copy(mode = mode))
                                    },
                                    modifier = Modifier.weight(1f).semantics { role = Role.RadioButton },
                                    shapes = when (index) {
                                        0 -> ButtonGroupDefaults.connectedLeadingButtonShapes()
                                        modes.lastIndex -> ButtonGroupDefaults.connectedTrailingButtonShapes()
                                        else -> ButtonGroupDefaults.connectedMiddleButtonShapes()
                                    },
                                ) {
                                    ZIcon(icon, null, Modifier.size(ToggleButtonDefaults.IconSize))
                                    Spacer(Modifier.size(ToggleButtonDefaults.IconSpacing))
                                    Text(label)
                                }
                            }
                        }
                    }
                }
                val dynamic = toggleAction { model.setAppearance(appearance.copy(dynamicColor = it)) }
                SegmentedListItem(
                    onClick = tapAction { dynamic(!appearance.dynamicColor) },
                    shapes = segmentedShapes(1, 2),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Magic) },
                    supportingContent = { Text("Tint Zeron with your wallpaper's palette") },
                    trailingContent = { Switch(appearance.dynamicColor, dynamic) },
                ) { Text("Wallpaper colors") }
            }
        }
        section("Wallpaper")
        item { WallpaperSettings(model) }
        if (devices.isNotEmpty()) {
            section("Devices")
            item {
                Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                    devices.forEachIndexed { i, device ->
                        SegmentedListItem(
                            onClick = tapAction {},
                            shapes = segmentedShapes(i, devices.size),
                            colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                            leadingContent = { IconTile(DeviceIdentity.icon(device.platform)) },
                            supportingContent = {
                                Text(
                                    listOfNotNull(
                                        if (device.isSelf) "This device" else if (device.online) "Online" else "Offline",
                                        device.version?.let { "v$it" },
                                        when (device.sessionCount) {
                                            0u -> null
                                            1u -> "1 session"
                                            else -> "${device.sessionCount} sessions"
                                        },
                                    ).joinToString(" · "),
                                )
                            },
                            trailingContent = {
                                Box(
                                    Modifier.size(10.dp).clip(CircleShape).background(
                                        if (device.online || device.isSelf) successColor() else MaterialTheme.colorScheme.outlineVariant,
                                    ),
                                )
                            },
                        ) { Text(device.name) }
                    }
                }
            }
        }
        if (developer && !demo) {
            section("Developer")
            item {
                val server = model.phone.customServer
                SegmentedListItem(
                    onClick = tapAction { editingServer = true },
                    shapes = segmentedShapes(0, 1),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Server) },
                    supportingContent = { Text(server?.edgeUrl ?: "Zeron (default)") },
                    trailingContent = { ZIcon(ZIcons.ChevronRight, null, Modifier.size(20.dp)) },
                    modifier = Modifier.padding(horizontal = 16.dp),
                ) { Text("Custom server") }
            }
        }
        section("About")
        item {
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                val rows = if (demo) 2 else 1
                SegmentedListItem(
                    // Seven taps reveal the developer options.
                    onClick = tapAction { if (++versionTaps >= 7 && !developer) { model.setDeveloper(true); fb.both(Haptic.Confirm, Cue.Open) } },
                    shapes = segmentedShapes(0, rows),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Info) },
                    supportingContent = { Text("Zeron for Android · core ${coreVersion()}") },
                ) { Text("Version") }
                if (demo) {
                    SegmentedListItem(
                        onClick = feedbackAction(Haptic.Confirm, Cue.Close) { model.leaveDemo() },
                        shapes = segmentedShapes(1, rows),
                        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                        leadingContent = { IconTile(ZIcons.Logout, MaterialTheme.colorScheme.errorContainer, MaterialTheme.colorScheme.onErrorContainer) },
                    ) { Text("Leave demo", color = MaterialTheme.colorScheme.error) }
                }
            }
        }
    }
    StatusBarScrim(scrolled = list.firstVisibleItemIndex > 0 || list.firstVisibleItemScrollOffset > 0)
    }
    if (editingServer) CustomServerDialog(model.phone.customServer, onDismiss = { editingServer = false }) {
        editingServer = false
        model.setCustomServer(it)
    }
    orgs?.let { (list, choice) -> OrgDialog(list, onPick = { choice.complete(it) }) }
}

/** The account rows: sign in (local), sign out, or the custom server. */
@Composable
private fun AccountRows(model: AppModel, account: Account, signIn: SignIn, onSignIn: () -> Unit, onServer: () -> Unit) {
    Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
        when (account) {
            is Account.Server -> SegmentedListItem(
                onClick = tapAction(onServer),
                shapes = segmentedShapes(0, 1),
                colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                leadingContent = { IconTile(ZIcons.Server) },
                supportingContent = { Text("This phone syncs with a development server") },
                trailingContent = { ZIcon(ZIcons.ChevronRight, null, Modifier.size(20.dp)) },
            ) { Text("Custom server") }
            is Account.SignedIn -> SegmentedListItem(
                onClick = feedbackAction(Haptic.Confirm, Cue.Close) { model.signOut() },
                enabled = signIn !is SignIn.Busy,
                shapes = segmentedShapes(0, 1),
                colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                leadingContent = { IconTile(ZIcons.Logout, MaterialTheme.colorScheme.errorContainer, MaterialTheme.colorScheme.onErrorContainer) },
                supportingContent = { Text("This phone goes back to its local workspace") },
            ) { Text("Sign out", color = MaterialTheme.colorScheme.error) }
            else -> SegmentedListItem(
                onClick = tapAction(onSignIn),
                enabled = signIn !is SignIn.Busy,
                shapes = segmentedShapes(0, 1),
                colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                leadingContent = { IconTile(ZIcons.Cloud) },
                supportingContent = {
                    Text(
                        when (signIn) {
                            SignIn.Preparing -> "Preparing this phone…"
                            SignIn.Completing, SignIn.Restarting -> "Signing in…"
                            is SignIn.Failed -> signIn.message
                            else -> "Use this phone with your computers, and them with it"
                        },
                    )
                },
                trailingContent = { ZIcon(ZIcons.ChevronRight, null, Modifier.size(20.dp)) },
            ) { Text("Sign in") }
        }
    }
}

/** Developer: point this phone's engine at a `zeron local-edge` (or back at Zeron). */
@Composable
private fun CustomServerDialog(current: CustomServer?, onDismiss: () -> Unit, onSave: (CustomServer?) -> Unit) {
    var url by remember { mutableStateOf(current?.edgeUrl ?: "http://10.0.2.2:27700") }
    var token by remember { mutableStateOf(current?.token.orEmpty()) }
    val problem = CustomServer.problem(url, token)
    val fb = LocalFeedback.current
    var invalid by remember { mutableStateOf(false) }
    androidx.compose.material3.AlertDialog(
        onDismissRequest = onDismiss,
        icon = { ZIcon(ZIcons.Server, null) },
        title = { Text("Custom server") },
        text = {
            OpenCloseFeedback()
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(
                    "This phone's engine joins a development edge (`zeron local-edge`) instead of Zeron, as its single user. It restarts to switch.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                androidx.compose.material3.OutlinedTextField(
                    url, { url = it },
                    label = { Text("Edge URL") },
                    singleLine = true,
                    shape = RoundedCornerShape(16.dp),
                    modifier = Modifier.fillMaxWidth(),
                )
                androidx.compose.material3.OutlinedTextField(
                    token, { token = it },
                    label = { Text("Token") },
                    singleLine = true,
                    shape = RoundedCornerShape(16.dp),
                    textStyle = MaterialTheme.typography.bodyLarge.copy(fontFamily = sh.zeron.android.design.GeistMono),
                    modifier = Modifier.fillMaxWidth(),
                )
                if (problem != null && (token.isNotEmpty() || invalid)) {
                    Text(problem, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                }
            }
        },
        confirmButton = {
            androidx.compose.material3.TextButton(onClick = {
                if (problem != null) {
                    // Refused: say why, and let the refusal be felt.
                    invalid = true
                    fb.both(Haptic.Error, Cue.Error)
                } else {
                    fb.both(Haptic.Confirm, Cue.Select)
                    onSave(CustomServer.of(url, token))
                }
            }) { Text("Use server") }
        },
        dismissButton = {
            androidx.compose.material3.TextButton(onClick = tapAction { if (current != null) onSave(null) else onDismiss() }) {
                Text(if (current != null) "Use Zeron" else "Cancel")
            }
        },
    )
}

private fun LazyListScope.section(title: String) {
    item {
        Text(
            title,
            style = MaterialTheme.typography.titleSmallEmphasized,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.padding(start = 28.dp, top = 24.dp, bottom = 8.dp),
        )
    }
}

/** A leading glyph on a tonal rounded tile. */
@Composable
fun IconTile(
    icon: Int,
    container: Color = MaterialTheme.colorScheme.secondaryContainer,
    content: Color = MaterialTheme.colorScheme.onSecondaryContainer,
) {
    Box(Modifier.size(40.dp).clip(RoundedCornerShape(14.dp)).background(container), contentAlignment = Alignment.Center) {
        ZIcon(icon, null, Modifier.size(22.dp), tint = content)
    }
}

/** Wallpaper: shown behind new sessions and the sessions list (iOS parity). */
@Composable
private fun WallpaperSettings(model: AppModel) {
    val store = model.wallpaper
    val state by store.state.collectAsState()
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    val picker = androidx.activity.compose.rememberLauncherForActivityResult(
        androidx.activity.result.contract.ActivityResultContracts.PickVisualMedia(),
    ) { uri -> if (uri != null) scope.launch { store.set(uri, "Photo") } }
    var effects by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf(false) }
    val rows = if (state.set) 3 else 1
    Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
        SegmentedListItem(
            onClick = tapAction {
                picker.launch(androidx.activity.result.PickVisualMediaRequest(androidx.activity.result.contract.ActivityResultContracts.PickVisualMedia.ImageOnly))
            },
            shapes = segmentedShapes(0, rows),
            colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
            leadingContent = { IconTile(ZIcons.Image) },
            supportingContent = { Text(if (state.set) state.name ?: "Photo" else "Shown behind new sessions and the sessions list") },
        ) { Text(if (state.set) "Change wallpaper" else "Choose wallpaper") }
        if (state.set) {
            Box {
                SegmentedListItem(
                    onClick = tapAction { effects = true },
                    shapes = segmentedShapes(1, rows),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Magic) },
                    supportingContent = { Text("${sh.zeron.android.core.WallpaperStore.label(state.effect)} — ${sh.zeron.android.core.WallpaperStore.detail(state.effect)}") },
                ) { Text("Effect") }
                ChoiceMenu(effects, { effects = false }, listOf(MenuSection("Effect", sh.zeron.android.core.WallpaperStore.effects.map { e ->
                    MenuChoice(sh.zeron.android.core.WallpaperStore.label(e), e == state.effect) { store.setEffect(e) }
                })))
            }
            SegmentedListItem(
                onClick = feedbackAction(Haptic.Confirm, Cue.Delete) { store.remove() },
                shapes = segmentedShapes(2, rows),
                colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                leadingContent = { IconTile(ZIcons.Delete, MaterialTheme.colorScheme.errorContainer, MaterialTheme.colorScheme.onErrorContainer) },
            ) { Text("Remove wallpaper", color = MaterialTheme.colorScheme.error) }
        }
    }
}
