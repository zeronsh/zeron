package sh.zeron.android.ui

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.LinearWavyProgressIndicator
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.toShape
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.material3.pulltorefresh.PullToRefreshDefaults
import androidx.compose.material3.pulltorefresh.rememberPullToRefreshState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.PhoneEngine
import sh.zeron.android.design.GeistMono
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import sh.zeron.android.feedback.Cue
import sh.zeron.android.feedback.Haptic
import sh.zeron.android.feedback.OpenCloseFeedback
import sh.zeron.android.feedback.feedbackAction
import sh.zeron.android.feedback.pullFeedback
import sh.zeron.android.feedback.tapAction
import sh.zeron.runtime.RuntimePermissions
import sh.zeron.runtime.RuntimeState

/**
 * Ask for what keeps this phone's engine useful, the first time the user
 * commits (first run): the notification permission (engine status, finished
 * sessions), then — when [battery] — the battery exemption. `then` runs once
 * the notification dialog is answered (never over it: it may open a browser).
 */
@Composable
fun rememberPermissionAsk(): (battery: Boolean, then: () -> Unit) -> Unit {
    val activity = LocalContext.current as Activity
    var pending by remember { mutableStateOf<Pair<Boolean, () -> Unit>?>(null) }
    val notifications = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        pending?.let { (battery, then) ->
            pending = null
            if (battery) RuntimePermissions.requestIgnoreBatteryOptimizations(activity)
            then()
        }
    }
    return remember(activity) {
        { battery, then ->
            if (Build.VERSION.SDK_INT >= 33 && RuntimePermissions.needsNotificationPermission(activity)) {
                pending = battery to then
                // Asked here, in context: the "first message" prompt (Composer) must not ask a second time.
                (activity.application as? sh.zeron.android.ZeronApplication)?.model?.notificationsAsked = true
                notifications.launch(Manifest.permission.POST_NOTIFICATIONS)
            } else {
                if (battery) RuntimePermissions.requestIgnoreBatteryOptimizations(activity)
                then()
            }
        }
    }
}

/**
 * Settings → This phone: the device's engine like any computer's — state,
 * start/stop/reset, keeping it alive, its coding agents, the log.
 */
@Composable
fun EngineScreen(model: AppModel, onBack: () -> Unit, onAgents: () -> Unit) {
    val phone = model.phone
    val state by phone.state.collectAsState()
    val start = { model.startEngine() }
    val activity = LocalContext.current as Activity
    val clipboard = LocalClipboardManager.current
    var confirmReset by remember { mutableStateOf(false) }
    // Permission rows re-read when the user comes back from a system screen.
    var resumes by remember { mutableIntStateOf(0) }
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) { resumes++ }
    val battery = remember(resumes) { RuntimePermissions.isIgnoringBatteryOptimizations(activity) }
    val notify = remember(resumes) { model.notifier.permitted }
    val notifications = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { resumes++ }
    var log by remember { mutableStateOf("") }
    LaunchedEffect(Unit) {
        while (true) {
            log = withContext(Dispatchers.IO) { runCatching { phone.logTail(160) }.getOrDefault("") }
            delay(2000)
        }
    }

    SubPage(title = "This phone", subtitle = "Its engine and coding agents", onBack = onBack) {
        item { EngineCard(phone, state, Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) }
        item {
            Row(Modifier.padding(horizontal = 16.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                when (state) {
                    is RuntimeState.Running, RuntimeState.Starting, is RuntimeState.Bootstrapping ->
                        FilledTonalButton(onClick = feedbackAction(Haptic.Confirm, Cue.ToggleOff) { model.stopEngine() }, modifier = Modifier.weight(1f), shapes = ButtonDefaults.shapes()) {
                            ZIcon(ZIcons.Stop, null, Modifier.size(ButtonDefaults.IconSize))
                            Spacer(Modifier.size(ButtonDefaults.IconSpacing))
                            Text("Stop")
                        }
                    else -> Button(onClick = feedbackAction(Haptic.Confirm, Cue.ToggleOn, start), enabled = phone.isSupportedAbi, modifier = Modifier.weight(1f), shapes = ButtonDefaults.shapes()) {
                        ZIcon(ZIcons.Restart, null, Modifier.size(ButtonDefaults.IconSize))
                        Spacer(Modifier.size(ButtonDefaults.IconSpacing))
                        Text(if (state == RuntimeState.NotInstalled) "Set up" else "Start")
                    }
                }
                OutlinedButton(onClick = tapAction { confirmReset = true }, modifier = Modifier.weight(1f), shapes = ButtonDefaults.shapes()) {
                    ZIcon(ZIcons.Delete, null, Modifier.size(ButtonDefaults.IconSize), tint = MaterialTheme.colorScheme.error)
                    Spacer(Modifier.size(ButtonDefaults.IconSpacing))
                    Text("Reset…", color = MaterialTheme.colorScheme.error)
                }
            }
        }
        sectionTitle("Agents")
        item {
            SegmentedListItem(
                onClick = tapAction(onAgents),
                shapes = segmentedShapes(0, 1),
                colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                leadingContent = { IconTile(ZIcons.Bot) },
                supportingContent = { Text("Install agents on this phone and sign in to them") },
                trailingContent = { ZIcon(ZIcons.ChevronRight, null, Modifier.size(20.dp)) },
                modifier = Modifier.padding(horizontal = 16.dp),
            ) { Text("Coding agents") }
        }
        sectionTitle("Keep it running")
        item {
            val rows = if (state is RuntimeState.Failed) 3 else 2
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                SegmentedListItem(
                    onClick = tapAction { if (!battery) RuntimePermissions.requestIgnoreBatteryOptimizations(activity) },
                    shapes = segmentedShapes(0, rows),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Restart) },
                    supportingContent = { Text(if (battery) "Allowed" else "Android may pause the engine — tap to allow") },
                    trailingContent = { StatusDot(battery) },
                ) { Text("Unrestricted battery") }
                SegmentedListItem(
                    onClick = tapAction {
                        if (notify) return@tapAction
                        if (Build.VERSION.SDK_INT >= 33 && activity.shouldShowRequestPermissionRationale(Manifest.permission.POST_NOTIFICATIONS)) {
                            notifications.launch(Manifest.permission.POST_NOTIFICATIONS)
                        } else {
                            // Never asked, or denied for good: the system screen always works.
                            activity.startActivity(
                                Intent(android.provider.Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                                    .putExtra(android.provider.Settings.EXTRA_APP_PACKAGE, activity.packageName),
                            )
                        }
                    },
                    shapes = segmentedShapes(1, rows),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Bell) },
                    supportingContent = { Text(if (notify) "Engine status and finished sessions" else "Off — tap to allow") },
                    trailingContent = { StatusDot(notify) },
                ) { Text("Notifications") }
                if (state is RuntimeState.Failed) ChildProcessHint(Modifier, shapes = segmentedShapes(2, rows))
            }
        }
        sectionTitle("Log")
        item {
            Column(Modifier.padding(horizontal = 16.dp)) {
                LogBox(log.ifBlank { "No log yet." }, Modifier.fillMaxWidth())
                Spacer(Modifier.height(8.dp))
                TextButton(onClick = feedbackAction(Haptic.Confirm, Cue.Copy) { clipboard.setText(AnnotatedString(phone.logTail(1000))) }) {
                    ZIcon(ZIcons.Copy, null, Modifier.size(ButtonDefaults.IconSize))
                    Spacer(Modifier.size(ButtonDefaults.IconSpacing))
                    Text("Copy log")
                }
            }
        }
    }
    if (confirmReset) ResetDialog(onDismiss = { confirmReset = false }) { model.resetEngine() }
}

/** The engine's state on a tonal card: what it is doing, and how far along. */
@Composable
fun EngineCard(phone: PhoneEngine, state: RuntimeState, modifier: Modifier = Modifier) {
    val (container, content) = when (state) {
        is RuntimeState.Running -> MaterialTheme.colorScheme.primaryContainer to MaterialTheme.colorScheme.onPrimaryContainer
        is RuntimeState.Failed -> MaterialTheme.colorScheme.errorContainer to MaterialTheme.colorScheme.onErrorContainer
        else -> MaterialTheme.colorScheme.surfaceContainerHigh to MaterialTheme.colorScheme.onSurface
    }
    val detail = when (state) {
        RuntimeState.NotInstalled -> "Sets up a small Linux system (Alpine) with git, Node and the engine."
        is RuntimeState.Bootstrapping -> state.step
        RuntimeState.Starting -> "Starting the engine…"
        is RuntimeState.Running -> "${state.deviceName} · ready to run agents"
        RuntimeState.Stopped -> "Sessions on this phone are paused; your other devices still work."
        is RuntimeState.Failed -> state.reason
    }
    val busy = state is RuntimeState.Bootstrapping || state == RuntimeState.Starting
    Surface(shape = RoundedCornerShape(32.dp), color = container, contentColor = content, modifier = modifier.fillMaxWidth()) {
        Column(Modifier.padding(20.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(48.dp), contentAlignment = Alignment.Center) {
                    if (busy) {
                        LoadingIndicator(Modifier.size(48.dp))
                    } else {
                        Box(
                            Modifier.size(48.dp).clip(MaterialShapes.Cookie9Sided.toShape()).background(content.copy(alpha = 0.12f)),
                            contentAlignment = Alignment.Center,
                        ) { ZIcon(if (state is RuntimeState.Failed) ZIcons.Warning else ZIcons.Terminal, null, Modifier.size(24.dp)) }
                    }
                }
                Spacer(Modifier.width(16.dp))
                Column(Modifier.weight(1f)) {
                    Text(PhoneEngine.stateLabel(state), style = MaterialTheme.typography.titleLargeEmphasized)
                    Text(detail, style = MaterialTheme.typography.bodyMedium, color = content.copy(alpha = 0.8f), maxLines = 6)
                }
            }
            if (busy) {
                Spacer(Modifier.height(16.dp))
                val progress = (state as? RuntimeState.Bootstrapping)?.progress
                if (progress != null) {
                    LinearWavyProgressIndicator(progress = { progress }, modifier = Modifier.fillMaxWidth())
                } else {
                    LinearWavyProgressIndicator(Modifier.fillMaxWidth())
                }
            }
            if (!phone.isSupportedAbi) {
                Spacer(Modifier.height(12.dp))
                Text("This build doesn't include the engine for this device's CPU.", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error)
            }
        }
    }
}

/** Android 12+ kills apps' extra processes past a cap; this is the switch that lifts it. */
@Composable
private fun ChildProcessHint(modifier: Modifier, shapes: androidx.compose.material3.ListItemShapes = segmentedShapes(0, 1)) {
    val context = LocalContext.current
    SegmentedListItem(
        onClick = tapAction { runCatching { context.startActivity(RuntimePermissions.developerOptionsIntent()) } },
        shapes = shapes,
        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
        leadingContent = { IconTile(ZIcons.Warning, MaterialTheme.colorScheme.errorContainer, MaterialTheme.colorScheme.onErrorContainer) },
        supportingContent = { Text("If Android stopped the engine, turn off “Disable child process restrictions” in Developer options.") },
        trailingContent = { ZIcon(ZIcons.ChevronRight, null, Modifier.size(20.dp)) },
        modifier = modifier,
    ) { Text("Child process limit") }
}

@Composable
private fun LogBox(text: String, modifier: Modifier) {
    // Follow the tail as lines arrive.
    val scroll = rememberScrollState()
    val end = scroll.maxValue
    LaunchedEffect(text, end) { scroll.scrollTo(end) }
    Surface(shape = RoundedCornerShape(20.dp), color = MaterialTheme.colorScheme.surfaceContainerHighest, modifier = modifier) {
        Box(
            Modifier
                .heightIn(min = 120.dp, max = 360.dp)
                .verticalScroll(scroll)
                .horizontalScroll(rememberScrollState())
                .padding(14.dp),
        ) {
            Text(text, style = MaterialTheme.typography.bodySmall, fontFamily = GeistMono, color = MaterialTheme.colorScheme.onSurfaceVariant, softWrap = false)
        }
    }
}

@Composable
private fun StatusDot(ok: Boolean) {
    Box(Modifier.size(10.dp).clip(androidx.compose.foundation.shape.CircleShape).background(if (ok) successColor() else MaterialTheme.colorScheme.error))
}

@Composable
private fun ResetDialog(onDismiss: () -> Unit, onReset: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        icon = { ZIcon(ZIcons.Delete, null) },
        title = { Text("Reset this phone's engine?") },
        text = {
            OpenCloseFeedback()
            Text("Deletes the Linux guest with its projects, agents and their sign-ins, this phone's local sessions and its Zeron sign-in. Sessions on your other devices aren't touched.")
        },
        confirmButton = {
            // Irreversible: the heaviest cue; the dialog's own Close stays quiet behind it.
            TextButton(onClick = feedbackAction(Haptic.Heavy, Cue.Delete) {
                onDismiss()
                onReset()
            }) { Text("Reset", color = MaterialTheme.colorScheme.error) }
        },
        dismissButton = { TextButton(onClick = tapAction(onDismiss)) { Text("Cancel") } },
    )
}

/**
 * A pushed settings page: round back button, the expressive title (with
 * optional header [actions]), a list; pull-to-refresh when [onRefresh] is set.
 */
@Composable
fun SubPage(
    title: String,
    subtitle: String?,
    onBack: () -> Unit,
    overlay: @Composable BoxScope.() -> Unit = {},
    actions: @Composable RowScope.() -> Unit = {},
    refreshing: Boolean = false,
    onRefresh: (() -> Unit)? = null,
    content: LazyListScope.() -> Unit,
) {
    val list = rememberLazyListState()
    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        val column = @Composable {
            LazyColumn(
                Modifier.fillMaxSize(),
                state = list,
                contentPadding = PaddingValues(bottom = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 48.dp),
            ) {
                item {
                    Box(Modifier.statusBarsPadding().padding(start = 12.dp, top = 8.dp)) {
                        TonalCircleButton(ZIcons.Back, "Back", onClick = onBack)
                    }
                }
                item { ScreenHeader(title, subtitle, actions = actions) }
                content()
            }
        }
        if (onRefresh == null) {
            column()
        } else {
            val pull = rememberPullToRefreshState()
            PullToRefreshBox(
                isRefreshing = refreshing,
                onRefresh = pullFeedback(pull, onRefresh),
                state = pull,
                modifier = Modifier.fillMaxSize(),
                indicator = {
                    PullToRefreshDefaults.LoadingIndicator(
                        state = pull,
                        isRefreshing = refreshing,
                        modifier = Modifier.align(Alignment.TopCenter).padding(WindowInsets.statusBars.asPaddingValues()),
                    )
                },
            ) { column() }
        }
        StatusBarScrim(scrolled = list.firstVisibleItemIndex > 0 || list.firstVisibleItemScrollOffset > 0)
        overlay()
    }
}

fun LazyListScope.sectionTitle(title: String) {
    item {
        Text(
            title,
            style = MaterialTheme.typography.titleSmallEmphasized,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.padding(start = 28.dp, top = 24.dp, bottom = 8.dp),
        )
    }
}
