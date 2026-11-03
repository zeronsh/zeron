package sh.zeron.android.ui

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.view.HapticFeedbackConstants
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.annotation.DrawableRes
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonGroupDefaults
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.ToggleButton
import androidx.compose.material3.ToggleButtonDefaults
import androidx.compose.material3.toShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.CloudStore
import sh.zeron.android.core.WallpaperStore
import sh.zeron.android.core.signInToCloud
import sh.zeron.android.design.LocalDarkTheme
import sh.zeron.android.design.ThemeMode
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import kotlin.coroutines.cancellation.CancellationException

/** First run, in the desktop's steps (iOS `OnboardingViewController.Step`). */
private enum class OnboardingStep(val label: String) {
    Appearance("appearance"), Sync("sync"), Github("github"), Agents("agents"), Notifications("notifications"), Done("done");

    /** "2 of 5" (Done has no number). */
    val number: Int? get() = if (this == Done) null else ordinal + 1
    val previous: OnboardingStep? get() = entries.getOrNull(ordinal - 1)
    val next: OnboardingStep get() = entries.getOrNull(ordinal + 1) ?: Done

    companion object {
        const val TOTAL = 5
        fun named(name: String?) = entries.firstOrNull { it.label == name }
    }
}

private fun onboardingPrefs(context: Context) = context.getSharedPreferences("onboarding", 0)

private fun onboardedKey(model: AppModel): String? = runCatching { model.client.value?.userId() }.getOrNull()?.let { "onboarded:$it" }

/** First run after signing in: this account hasn't been through onboarding on this device (never the demo). */
fun shouldShowOnboarding(context: Context, model: AppModel): Boolean {
    if (model.client.value == null || model.isDemo) return false
    val key = onboardedKey(model) ?: return false
    return !onboardingPrefs(context).getBoolean(key, false)
}

/** Onboarding is done for the signed-in account. */
fun markOnboardingSeen(context: Context, model: AppModel) {
    val key = onboardedKey(model) ?: return
    onboardingPrefs(context).edit().putBoolean(key, true).apply()
}

/** "Claude Code", "Claude Code and Codex", "A, B and C". */
private fun join(names: List<String>): String = when (names.size) {
    0 -> ""
    1 -> names[0]
    else -> names.dropLast(1).joinToString(", ") + " and " + names.last()
}

/**
 * First run (iOS `OnboardingViewController`): make it yours (a look and a
 * new-chat wallpaper), sync, connect GitHub, your agents, notifications,
 * then "You're all set". Opens on GitHub after signing in (or on `step`);
 * everything can be changed later in Settings. `onFinish(true)` asks for the
 * new-session page.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun OnboardingScreen(model: AppModel, step: String?, onFinish: (startSession: Boolean) -> Unit) {
    val context = LocalContext.current
    val view = LocalView.current
    val cloud = model.cloud
    var current by rememberSaveable { mutableStateOf(OnboardingStep.named(step) ?: OnboardingStep.Github) }
    var github by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) { if (model.client.value != null) cloud.refresh() }
    // The edge has answered that this account has no Cloud: its step (GitHub) and lines are left out.
    val cloudWithheld = cloud.info != null && !model.cloudOffered
    LaunchedEffect(cloudWithheld, current) { if (current == OnboardingStep.Github && cloudWithheld) current = OnboardingStep.Agents }

    fun go(to: OnboardingStep) {
        view.performHapticFeedback(HapticFeedbackConstants.CONTEXT_CLICK)
        current = if (to == OnboardingStep.Github && cloudWithheld) {
            if (current.ordinal > to.ordinal) OnboardingStep.Sync else OnboardingStep.Agents
        } else {
            to
        }
    }

    fun finish(startSession: Boolean) {
        markOnboardingSeen(context, model)
        onFinish(startSession)
    }

    val previous = current.previous
    BackHandler(enabled = previous != null && current != OnboardingStep.Done) { previous?.let(::go) }

    val notifications = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { go(OnboardingStep.Done) }

    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        // The new-chat wallpaper behind every step, softened so text over it stays legible.
        WallpaperBackdrop(model.wallpaper, softened = current != OnboardingStep.Appearance)
        Column(
            Modifier
                .fillMaxSize()
                .statusBarsPadding()
                .navigationBarsPadding()
                .padding(horizontal = 24.dp)
                .widthIn(max = 560.dp)
                .align(Alignment.TopCenter),
        ) {
            Progress(current.number, Modifier.padding(top = 14.dp, bottom = 18.dp))
            val fadeSpec = MaterialTheme.motionScheme.defaultEffectsSpec<Float>()
            val fastFade = MaterialTheme.motionScheme.fastEffectsSpec<Float>()
            val slideSpec = MaterialTheme.motionScheme.defaultSpatialSpec<androidx.compose.ui.unit.IntOffset>()
            AnimatedContent(
                current,
                transitionSpec = {
                    val forward = targetState.ordinal > initialState.ordinal
                    (fadeIn(fadeSpec) + slideInHorizontally(slideSpec) { if (forward) it / 6 else -it / 6 }) togetherWith
                        (fadeOut(fastFade) + slideOutHorizontally(slideSpec) { if (forward) -it / 6 else it / 6 })
                },
                modifier = Modifier.weight(1f).fillMaxWidth(),
                label = "onboarding",
            ) { page ->
                Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(top = 6.dp, bottom = 16.dp), verticalArrangement = Arrangement.spacedBy(28.dp)) {
                    when (page) {
                        OnboardingStep.Appearance -> AppearancePage(model)
                        OnboardingStep.Sync -> SyncPage(model)
                        OnboardingStep.Github -> GithubPage(model) { github = true }
                        OnboardingStep.Agents -> AgentsPage(model, cloudWithheld)
                        OnboardingStep.Notifications -> NotificationsPage()
                        OnboardingStep.Done -> DonePage(model, cloudWithheld, ::finish)
                    }
                }
            }
            if (current != OnboardingStep.Done) {
                // Back on the left; Skip and the way forward on the right.
                Row(Modifier.fillMaxWidth().padding(vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                    if (previous != null) {
                        TextButton(onClick = { go(previous) }) { Text("Back", style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                    }
                    Spacer(Modifier.weight(1f))
                    when (current) {
                        OnboardingStep.Appearance -> Forward("Continue") { go(OnboardingStep.Sync) }
                        OnboardingStep.Sync -> Forward("Continue") { go(OnboardingStep.Github) }
                        OnboardingStep.Github -> if (cloud.githubConnected) {
                            Forward("Continue") { go(OnboardingStep.Agents) }
                        } else {
                            Ghost("Skip for now") { go(OnboardingStep.Agents) }
                        }
                        OnboardingStep.Agents -> Forward("Continue") { go(OnboardingStep.Notifications) }
                        OnboardingStep.Notifications -> {
                            Ghost("Not now") { go(OnboardingStep.Done) }
                            Spacer(Modifier.width(8.dp))
                            Forward("Turn on") {
                                val needed = Build.VERSION.SDK_INT >= 33 &&
                                    context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
                                if (needed) notifications.launch(Manifest.permission.POST_NOTIFICATIONS) else go(OnboardingStep.Done)
                            }
                        }
                        OnboardingStep.Done -> Unit
                    }
                }
            }
        }
    }

    if (github) {
        GitHubConnectSheet(model, onDismiss = { github = false }, onConnected = {
            github = false
            view.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
        })
    }
}

/** The wallpaper behind the steps; blurred and veiled except where it's being picked. */
@Composable
private fun WallpaperBackdrop(store: WallpaperStore, softened: Boolean) {
    val state by store.state.collectAsState()
    val dark = LocalDarkTheme.current
    val render by produceState<WallpaperStore.Render?>(null, state, dark) { value = store.render(dark) }
    val r = render ?: return
    val veil by androidx.compose.animation.core.animateFloatAsState(if (softened) 0.82f else 0.55f, label = "veil")
    Image(
        r.image,
        null,
        contentScale = ContentScale.Crop,
        alpha = r.opacity,
        modifier = Modifier.fillMaxSize().then(if (softened) Modifier.blur(28.dp) else Modifier),
    )
    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background.copy(alpha = veil)))
}

/** The desktop's progress: a short bar per step, then "2 of 5". */
@Composable
private fun Progress(number: Int?, modifier: Modifier = Modifier) {
    Row(modifier.heightIn(min = 20.dp), verticalAlignment = Alignment.CenterVertically) {
        if (number == null) return@Row
        for (ix in 1..OnboardingStep.TOTAL) {
            val color by animateColorAsState(
                if (ix <= number) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.surfaceContainerHighest,
                MaterialTheme.motionScheme.defaultEffectsSpec(),
                label = "progress",
            )
            Box(Modifier.padding(end = 4.dp).size(width = 22.dp, height = 4.dp).clip(CircleShape).background(color))
        }
        Spacer(Modifier.width(8.dp))
        Text("$number of ${OnboardingStep.TOTAL}", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.outline)
    }
}

@Composable
private fun Forward(text: String, onClick: () -> Unit) {
    Button(onClick = onClick, shape = CircleShape, modifier = Modifier.heightIn(min = 48.dp)) {
        Text(text, style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(horizontal = 8.dp))
    }
}

@Composable
private fun Ghost(text: String, onClick: () -> Unit) {
    TextButton(onClick = onClick, modifier = Modifier.heightIn(min = 48.dp)) {
        Text(text, style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun Heading(title: String, detail: String) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(title, style = MaterialTheme.typography.headlineMediumEmphasized)
        Text(detail, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/** A small muted label over its control. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun Group(title: String, content: @Composable ColumnScope.() -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
        Text(title, style = MaterialTheme.typography.titleSmallEmphasized, color = MaterialTheme.colorScheme.primary, modifier = Modifier.padding(start = 4.dp, bottom = 6.dp))
        content()
    }
}

/** Plain informational rows (tile, title, line) in one segmented card. */
@Composable
private fun InfoRows(rows: List<Triple<Int, String, String>>) {
    Column(verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
        rows.forEachIndexed { i, (icon, title, detail) ->
            CloudRow(i, rows.size, title, detail, leading = { IconTile(icon) })
        }
    }
}

// ── pages ─────────────────────────────────────────────────────────────────

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun AppearancePage(model: AppModel) {
    val appearance by model.appearance.collectAsState()
    val store = model.wallpaper
    val wallpaper by store.state.collectAsState()
    val scope = rememberCoroutineScope()
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
        if (uri != null) scope.launch { store.set(uri, "Photo") }
    }
    var effects by remember { mutableStateOf(false) }
    Heading("Make it yours", "Pick a look. You can change it any time in Settings → Appearance.")
    Group("Color scheme") {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(ButtonGroupDefaults.ConnectedSpaceBetween)) {
            val modes = listOf(
                Triple(ThemeMode.System, "System", ZIcons.Monitor),
                Triple(ThemeMode.Light, "Light", ZIcons.Sun),
                Triple(ThemeMode.Dark, "Dark", ZIcons.Moon),
            )
            modes.forEachIndexed { index, (mode, label, icon) ->
                ToggleButton(
                    checked = appearance.mode == mode,
                    onCheckedChange = { model.setAppearance(appearance.copy(mode = mode)) },
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
    Group("New chat wallpaper") {
        // The wallpaper as a new chat shows it, effect and all.
        AnimatedVisibility(wallpaper.set) {
            val dark = LocalDarkTheme.current
            val render by produceState<WallpaperStore.Render?>(null, wallpaper, dark) { value = store.render(dark) }
            Box(Modifier.fillMaxWidth().height(132.dp).padding(bottom = 8.dp).clip(RoundedCornerShape(24.dp)).background(MaterialTheme.colorScheme.surfaceContainerHigh)) {
                render?.let { Image(it.image, null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize()) }
            }
        }
        val rows = if (wallpaper.set) 2 else 1
        CloudRow(
            0, rows,
            if (wallpaper.set) wallpaper.name ?: "Wallpaper" else "No wallpaper",
            "Shows behind every new chat.",
            leading = { IconTile(ZIcons.Image) },
            trailing = {
                RowButton(if (wallpaper.set) "Change" else "Choose image", prominent = !wallpaper.set) {
                    picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
                }
            },
        )
        if (wallpaper.set) {
            Box {
                CloudRow(
                    1, rows, "Background effect", WallpaperStore.detail(wallpaper.effect),
                    leading = { IconTile(ZIcons.Magic) },
                    trailing = { RowButton(WallpaperStore.label(wallpaper.effect), prominent = false) { effects = true } },
                )
                Box(Modifier.align(Alignment.TopEnd)) {
                    GroupedActionMenu(
                        effects,
                        { effects = false },
                        listOf(
                            WallpaperStore.effects.map { e -> MenuAction(WallpaperStore.label(e), ZIcons.Magic, checked = e == wallpaper.effect) { store.setEffect(e) } },
                            listOf(MenuAction("Remove wallpaper", ZIcons.Delete, destructive = true) { store.remove() }),
                        ),
                    )
                }
            }
        }
    }
}

@Composable
private fun SyncPage(model: AppModel) {
    Heading("Sync your work", "Log in to keep your chats, projects and settings in step everywhere you use Zeron.")
    InfoRows(
        listOf(
            Triple(ZIcons.Phone, "On every device", "Pick up a chat on your phone or another computer, right where you left it."),
            Triple(ZIcons.Laptop, "Run agents anywhere", "Start work on any of your machines from whichever one you're at."),
            Triple(ZIcons.Cloud, "Zeron Cloud", "Hand chats to cloud machines that keep going while your computer sleeps."),
        ),
    )
    val email = model.credentials.profile.email
    Text(
        email?.let { "Signed in as $it" } ?: "You're signed in",
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
private fun GithubPage(model: AppModel, onConnect: () -> Unit) {
    val cloud = model.cloud
    Heading(
        "Connect GitHub",
        "Cloud agents work in your repositories on machines of their own. Connect GitHub so they can clone and push — or skip and do it later in Settings → Cloud.",
    )
    if (cloud.githubConnected) {
        val n = cloud.repos?.size ?: 0
        CloudRow(
            0, 1,
            cloud.githubAccount?.let { "@$it" } ?: "GitHub",
            if (n == 1) "1 repository" else "$n repositories",
            leading = { IconTile(ZIcons.GitHub) },
            trailing = { DoneLabel("Connected") },
        )
    } else {
        CloudRow(
            0, 1, "GitHub", "Clone and push from Cloud",
            leading = { IconTile(ZIcons.GitHub) },
            trailing = { RowButton("Connect", prominent = true, onClick = onConnect) },
        )
    }
}

/**
 * The agents Zeron can run: Cloud's (signed in from here, with your own
 * subscriptions) and the ones found on each of your computers.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun AgentsPage(model: AppModel, cloudWithheld: Boolean) {
    val context = LocalContext.current
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    val cloud = model.cloud
    val workspace by model.workspace.collectAsState()
    val hosts = workspace?.devices.orEmpty().filter { it.isExecutionHost }
    val client by model.client.collectAsState()
    // Each computer's agents (null while asking).
    val found by produceState<Map<String, List<String>?>>(emptyMap(), hosts.map { it.id }, client) {
        val c = client ?: return@produceState
        val out = hosts.associate { it.id to null as List<String>? }.toMutableMap()
        value = out.toMap()
        for (host in hosts) {
            out[host.id] = runCatching { c.listHarnesses(host.id) }.getOrNull().orEmpty()
                .filter { it.installed && it.enabled != false }
                .map { it.label }
            value = out.toMap()
        }
    }
    val cloudShown = model.cloudOffered && !cloudWithheld
    val names = found.values.filterNotNull().flatten().distinct()
    val detail = when {
        cloudShown -> "Cloud runs ${join(CloudStore.agents.map { it.name })} with your own subscriptions. Sign in to any you'd like to use."
        hosts.isEmpty() -> "Zeron runs the coding agents you already use. Install Zeron on a computer and they'll show up here."
        found.values.any { it == null } -> "Looking for coding agents…"
        names.isEmpty() -> "Zeron runs the coding agents you already use, and none are on your computers yet."
        else -> "Found ${if (names.size > 3) "${names.size} agents" else join(names)} on your computers. Turn them on or off any time in Settings → Providers."
    }
    Heading("Your agents", detail)
    if (cloudShown) {
        var signingIn by remember { mutableStateOf<String?>(null) }
        Group("Cloud") {
            CloudStore.agents.forEachIndexed { i, agent ->
                val active = cloud.activeAccount(agent)
                CloudRow(
                    i, CloudStore.agents.size, agent.name,
                    active?.let { it.email ?: it.displayName ?: if (it.kind == "key") agent.keyName else "Signed in" } ?: "Not signed in",
                    leading = { AgentTile(agent.harness) },
                    trailing = {
                        when {
                            signingIn == agent.harness -> LoadingIndicator(Modifier.size(32.dp))
                            active != null -> DoneLabel("Ready")
                            else -> RowButton("Sign in", prominent = true) {
                                signingIn = agent.harness
                                scope.launch {
                                    try {
                                        model.signInToCloud(context, agent)
                                        view.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
                                    } catch (e: CancellationException) {
                                        // The sign-in page was closed.
                                    } catch (e: Exception) {
                                        model.toast(CloudStore.message(e))
                                    } finally {
                                        signingIn = null
                                    }
                                }
                            }
                        }
                    },
                )
            }
        }
    }
    if (hosts.isNotEmpty()) {
        Group("Your computers") {
            hosts.forEachIndexed { i, host ->
                val agents = found[host.id]
                CloudRow(
                    i, hosts.size, host.name,
                    when {
                        agents == null -> "Looking for agents…"
                        agents.isEmpty() -> if (host.online) "No coding agents found" else "Offline"
                        else -> join(agents)
                    },
                    leading = { IconTile(if (host.platform.contains("mac", ignoreCase = true)) ZIcons.Laptop else ZIcons.Monitor) },
                    trailing = {
                        if (agents == null) LoadingIndicator(Modifier.size(28.dp))
                        else if (agents.isNotEmpty()) DoneLabel("${agents.size}")
                    },
                )
            }
        }
    }
}

@Composable
private fun NotificationsPage() {
    Heading("Stay in the loop", "Get a notification when an agent finishes, needs an answer, or runs into trouble.")
    InfoRows(
        listOf(
            Triple(ZIcons.Check, "Finished", "A session is done"),
            Triple(ZIcons.Chat, "Needs you", "An agent is waiting on an answer"),
            Triple(ZIcons.Warning, "Something went wrong", "A run failed"),
        ),
    )
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun DonePage(model: AppModel, cloudWithheld: Boolean, finish: (Boolean) -> Unit) {
    val cloud = model.cloud
    val appearance by model.appearance.collectAsState()
    val wallpaper by model.wallpaper.state.collectAsState()
    Spacer(Modifier.height(12.dp))
    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
        Box(
            Modifier.size(72.dp).clip(MaterialShapes.Cookie9Sided.toShape()).background(MaterialTheme.colorScheme.primaryContainer),
            contentAlignment = Alignment.Center,
        ) { ZIcon(ZIcons.Check, null, Modifier.size(30.dp), tint = MaterialTheme.colorScheme.onPrimaryContainer) }
        Spacer(Modifier.height(20.dp))
        Text("You're all set", style = MaterialTheme.typography.headlineMediumEmphasized, textAlign = TextAlign.Center)
        Spacer(Modifier.height(8.dp))
        Text(
            if (cloudWithheld) "Pick a project and start a session on any of your computers."
            else "Pick a project and start a session — on any of your computers, or on Cloud.",
            style = MaterialTheme.typography.bodyLarge,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
        )
    }
    val style = when (appearance.mode) {
        ThemeMode.System -> "System"
        ThemeMode.Light -> "Light"
        ThemeMode.Dark -> "Dark"
    }
    val lines = buildList {
        add("Appearance" to if (wallpaper.set) "$style · Wallpaper" else style)
        add("Sync" to (model.credentials.profile.email?.let { "Signed in as $it" } ?: "Signed in"))
        if (!cloudWithheld) {
            val agents = CloudStore.agents.filter { cloud.activeAccount(it) != null }.map { it.name }
            add("GitHub" to if (cloud.githubConnected) "Connected" else "Not yet · Settings → Cloud")
            add("Cloud agents" to if (agents.isEmpty()) "Not yet · Settings → Providers" else join(agents))
        }
    }
    Surface(shape = RoundedCornerShape(24.dp), color = cardColor(), modifier = Modifier.fillMaxWidth()) {
        Column {
            lines.forEachIndexed { i, (label, value) ->
                if (i > 0) HorizontalDivider(Modifier.padding(horizontal = 20.dp), color = MaterialTheme.colorScheme.outlineVariant)
                Row(Modifier.padding(horizontal = 20.dp, vertical = 14.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(label, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Spacer(Modifier.width(12.dp))
                    Text(value, style = MaterialTheme.typography.bodyLarge, textAlign = TextAlign.End, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                }
            }
        }
    }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        PrimaryCapsule("Start a session", onClick = { finish(true) }, icon = ZIcons.Plus)
        PlainCapsule("I'll do this later", onClick = { finish(false) })
    }
}

/** A row's trailing action: tonal, or filled when it's the way forward. */
@Composable
private fun RowButton(text: String, prominent: Boolean, @DrawableRes icon: Int? = null, onClick: () -> Unit) {
    val content: @Composable androidx.compose.foundation.layout.RowScope.() -> Unit = {
        icon?.let {
            ZIcon(it, null, Modifier.size(18.dp))
            Spacer(Modifier.width(6.dp))
        }
        Text(text, style = MaterialTheme.typography.labelLarge)
    }
    if (prominent) Button(onClick = onClick, shape = CircleShape, content = content)
    else FilledTonalButton(onClick = onClick, shape = CircleShape, content = content)
}
