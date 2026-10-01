package sh.zeron.android.ui

import sh.zeron.android.feedback.tapAction
import sh.zeron.android.feedback.feedbackAction
import sh.zeron.android.feedback.LocalFeedback
import sh.zeron.android.feedback.Haptic
import sh.zeron.android.feedback.Cue
import sh.zeron.android.feedback.OpenCloseFeedback
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.CallSplit
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.outlined.Computer
import androidx.compose.material.icons.outlined.Inbox
import androidx.compose.material.icons.outlined.Speed
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.draw.clip
import androidx.compose.foundation.background
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.DeviceIdentity
import sh.zeron.android.core.MachineGroup
import sh.zeron.android.core.Machines
import sh.zeron.android.core.PhoneEngine
import sh.zeron.android.core.ProjectSource
import uniffi.zeron_core.DeviceView
import sh.zeron.android.core.userMessage
import sh.zeron.android.design.HarnessMark
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.ModelInfo
import uniffi.zeron_core.fallbackHarnesses
import uniffi.zeron_core.fallbackModels
import uniffi.zeron_core.modelLabel
import uniffi.zeron_core.harnessLabel
import uniffi.zeron_core.reasoningLabel

/** The last catalog each host reported, so chips open on real model names. */
private val modelCache = HashMap<String, List<ModelChoice>>()

private fun catalogModels(): List<ModelChoice> = fallbackHarnesses().filter { it.offered }.flatMap { h ->
    fallbackModels(h.id).map { ModelChoice(h.id, h.label, it) }
}

/**
 * "What are we building?" — a composer-first canvas. Context is set with the
 * composer's chips (project, branch or host, model, effort); the prompt is
 * focused at once so the common case is: tap, type, send.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NewSessionScreen(model: AppModel, onClose: () -> Unit, onCreated: (String) -> Unit) {
    val workspace by model.workspace.collectAsState()
    val client by model.client.collectAsState()
    var draft by remember { mutableStateOf(model.lastDraft) }
    val composer = remember { ComposerModel() }
    val focus = remember { FocusRequester() }
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    val fb = LocalFeedback.current

    val projects = workspace?.projects.orEmpty()
    val hosts = workspace?.devices.orEmpty().filter { it.isExecutionHost }
    val project = projects.firstOrNull { it.id == draft.projectId }
    LaunchedEffect(workspace) {
        // A remembered project or host from before a reset (or another mode) is gone.
        if (workspace != null && draft.projectId != null && projects.none { it.id == draft.projectId }) draft = draft.copy(projectId = null)
        if (workspace != null && draft.hostId != null && hosts.none { it.id == draft.hostId }) draft = draft.copy(hostId = null)
        if (draft.projectId == null && draft.hostId == null) {
            draft = draft.copy(projectId = (projects.firstOrNull { it.deviceOnline } ?: projects.firstOrNull())?.id)
        }
        if (draft.projectId == null && draft.hostId == null) {
            // No projects yet: run on the first reachable host.
            draft = draft.copy(hostId = (hosts.firstOrNull { it.online } ?: hosts.firstOrNull())?.id)
        }
    }
    val deviceId = project?.deviceId ?: draft.hostId ?: ""

    // Models for the draft's host: cached (or the built-in catalog) at once, then the host's own list.
    var models by remember(deviceId) { mutableStateOf(modelCache[deviceId] ?: catalogModels()) }
    var modelsLoading by remember(deviceId) { mutableStateOf(false) }
    var modelsError by remember(deviceId) { mutableStateOf<String?>(null) }
    var reload by remember(deviceId) { mutableIntStateOf(0) }
    LaunchedEffect(deviceId, client, reload) {
        val c = client ?: return@LaunchedEffect
        if (deviceId.isEmpty()) return@LaunchedEffect
        modelsLoading = true
        modelsError = null
        val harnesses = runCatching { c.listHarnesses(deviceId) }.getOrElse {
            modelsError = it.userMessage()
            modelsLoading = false
            return@LaunchedEffect
        }
        val failures = mutableListOf<String>()
        val fresh = harnesses.filter { it.offered }.flatMap { h ->
            val result = runCatching { c.listModels(deviceId, h.id) }
            result.exceptionOrNull()?.let { failures += "${h.label}: ${it.userMessage()}" }
            (result.getOrNull() ?: fallbackModels(h.id)).map { ModelChoice(h.id, h.label, it) }
        }
        modelsError = failures.firstOrNull()
        modelsLoading = false
        if (fresh.isNotEmpty()) {
            modelCache[deviceId] = fresh
            models = fresh
            // The live list holds only harnesses installed on this device: a
            // fresh phone engine may have OpenCode but not the Claude Code
            // default, and sending to a missing harness just fails the turn.
            if (fresh.none { it.harness == draft.harness }) {
                val first = fresh.first()
                draft = draft.copy(harness = first.harness, model = first.model.id, effort = null, options = emptyMap())
            }
        }
    }
    val choice = models.firstOrNull { it.harness == draft.harness && it.model.id == draft.model }
        ?: models.firstOrNull { it.harness == draft.harness }

    LaunchedEffect(Unit) { focus.requestFocus() }

    // New projects: cloned or created empty by the draft's device's engine.
    var adding by remember { mutableStateOf<ProjectSource?>(null) }
    // Reported by the branch chip (it reloads, and re-reports, per project).
    var currentBranch by remember { mutableStateOf<String?>(null) }
    // `::create` is handed to the composer once: read what it needs as it is now.
    val latestProject by androidx.compose.runtime.rememberUpdatedState(project)
    val latestChoice by androidx.compose.runtime.rememberUpdatedState(choice)
    val targetDevice = deviceId.ifEmpty { hosts.firstOrNull { it.online }?.id ?: hosts.firstOrNull()?.id.orEmpty() }
    val target = hosts.firstOrNull { it.id == targetDevice }

    fun create() {
        model.lastDraft = draft
        // No pick: the session runs on (and is labelled with) the checked-out branch.
        val branch = draft.branch ?: currentBranch.takeIf { latestProject?.gitDetected == true }
        val id = model.createSession(draft.copy(model = draft.model ?: latestChoice?.model?.id, branch = branch), composer.encoded(), composer.images.map { it.outgoing })
        if (id != null) onCreated(id) else {
            fb.both(Haptic.Error, Cue.Error)
            scope.launch { snackbar.showSnackbar("Choose a project or a host that can run it.") }
        }
    }

    var composerBounds by remember { mutableStateOf<androidx.compose.ui.geometry.Rect?>(null) }
    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
    sh.zeron.android.design.WallpaperHero(model.wallpaper, cutout = { composerBounds })
    Scaffold(
        containerColor = androidx.compose.ui.graphics.Color.Transparent,
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = {
            TopAppBar(
                title = { Text("New session", style = MaterialTheme.typography.titleLargeEmphasized, modifier = Modifier.padding(start = 12.dp)) },
                navigationIcon = { TonalCircleButton(ZIcons.Close, "Close", onClick = onClose, modifier = Modifier.padding(start = 8.dp), size = 44.dp) },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = androidx.compose.ui.graphics.Color.Transparent),
            )
        },
    ) { padding ->
        Column(
            Modifier
                .padding(top = padding.calculateTopPadding())
                .fillMaxSize()
                .imePadding()
                .navigationBarsPadding(),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            // The headline lives in the free space above the composer.
            Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    HarnessMark(draft.harness, 40.dp, tint = MaterialTheme.colorScheme.onSurface)
                    Text(
                        "What are we building?",
                        style = MaterialTheme.typography.headlineSmall,
                        textAlign = TextAlign.Center,
                        modifier = Modifier.padding(horizontal = 32.dp),
                    )
                }
            }
            MentionSuggestions(
                composer,
                search = { q ->
                    val c = client
                    if (c == null || project == null) emptyList() else c.searchFiles(project.deviceId, null, project.id, q)
                },
                modifier = Modifier.padding(horizontal = 12.dp).widthIn(max = 768.dp),
            )
            ComposerSurface(
                model = composer,
                placeholder = "Describe the task",
                action = ComposerAction.Send,
                onAction = ::create,
                alwaysCard = true,
                focusRequester = focus,
                modifier = Modifier
                    .padding(horizontal = 12.dp)
                    .padding(bottom = 8.dp)
                    .widthIn(max = 768.dp)
                    .onGloballyPositioned { composerBounds = it.boundsInRoot() },
            ) {
                ProjectChip(
                    draft,
                    projects,
                    hosts,
                    onPick = { draft = it },
                    onAdd = if (model.isDemo || target == null) null else { source -> adding = source },
                    target = target,
                )
                if (project != null) {
                    if (project.gitDetected) BranchChip(model, draft, project.deviceId, project.path, onCurrent = { currentBranch = it }) { draft = it }
                } else {
                    HostChip(draft, hosts) { draft = it }
                }
                ModelChip(
                    model, draft, choice, models,
                    statuses = when {
                        modelsError != null -> listOf(CatalogStatus("", "Models", modelsError))
                        modelsLoading -> listOf(CatalogStatus("", "models"))
                        else -> emptyList()
                    },
                    onRetry = { reload++ },
                ) { change -> draft = change(draft) }
            }
        }
    }
    adding?.let { source ->
        NewProjectDialog(source, target, onDismiss = { adding = null }) { input ->
            model.addProject(targetDevice, source, input).onSuccess { id ->
                fb.both(Haptic.Success, Cue.UploadReady)
                adding = null
                draft = draft.copy(projectId = id, hostId = null, branch = null, cwd = null)
            }.exceptionOrNull()?.userMessage()
        }
    }
}
}

/**
 * Clone a repository or start an empty one on `device` — its engine does it
 * (this phone's into /home/zeron/projects) — then make it a project.
 * `submit` returns an error to show, or null once it's done.
 */
@Composable
private fun NewProjectDialog(source: ProjectSource, device: DeviceView?, onDismiss: () -> Unit, submit: suspend (String) -> String?) {
    var text by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val clone = source == ProjectSource.Clone
    val valid = if (clone) PhoneEngine.repoName(text) != null else PhoneEngine.folderName(text) != null
    fun go() {
        if (!valid || busy) return
        busy = true
        error = null
        scope.launch {
            error = submit(text.trim())
            busy = false
        }
    }
    androidx.compose.material3.AlertDialog(
        onDismissRequest = { if (!busy) onDismiss() },
        icon = { ZIcon(if (clone) ZIcons.Branch else ZIcons.Folder, null) },
        title = { Text(if (clone) "Clone a repository" else "New project") },
        text = {
            OpenCloseFeedback()
            Column {
                Text(
                    projectDestination(source, device),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(12.dp))
                androidx.compose.material3.OutlinedTextField(
                    text,
                    {
                        text = it
                        error = null
                    },
                    placeholder = { Text(if (clone) "https://github.com/org/repo.git" else "my-app") },
                    singleLine = true,
                    enabled = !busy,
                    isError = error != null,
                    supportingText = error?.let { { Text(it) } },
                    shape = androidx.compose.foundation.shape.RoundedCornerShape(16.dp),
                    textStyle = MaterialTheme.typography.bodyLarge.copy(fontFamily = sh.zeron.android.design.GeistMono),
                    keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(
                        keyboardType = if (clone) androidx.compose.ui.text.input.KeyboardType.Uri else androidx.compose.ui.text.input.KeyboardType.Text,
                        imeAction = androidx.compose.ui.text.input.ImeAction.Go,
                    ),
                    keyboardActions = androidx.compose.foundation.text.KeyboardActions(onGo = { go() }),
                    modifier = Modifier.fillMaxWidth(),
                )
                if (busy) {
                    Spacer(Modifier.height(8.dp))
                    androidx.compose.material3.LinearWavyProgressIndicator(Modifier.fillMaxWidth())
                }
            }
        },
        confirmButton = {
            androidx.compose.material3.TextButton(onClick = feedbackAction(Haptic.Confirm, Cue.Select, ::go), enabled = valid && !busy) { Text(if (clone) "Clone" else "Create") }
        },
        dismissButton = { androidx.compose.material3.TextButton(onClick = tapAction(action = onDismiss), enabled = !busy) { Text("Cancel") } },
    )
}

/** Where a new project lands, in words. */
private fun projectDestination(source: ProjectSource, device: DeviceView?): String {
    val where = when {
        device == null -> "the device"
        device.isSelf -> "${PhoneEngine.PROJECTS_ROOT} on this phone"
        else -> device.name
    }
    return when (source) {
        ProjectSource.Clone -> "Cloned into $where by its engine."
        ProjectSource.Empty -> "An empty git repository in $where."
    }
}

@Composable
private fun ProjectChip(
    draft: sh.zeron.android.core.NewSessionDraft,
    projects: List<uniffi.zeron_core.ProjectView>,
    hosts: List<DeviceView>,
    onPick: (sh.zeron.android.core.NewSessionDraft) -> Unit,
    onAdd: ((ProjectSource) -> Unit)?,
    target: DeviceView?,
) {
    var open by remember { mutableStateOf(false) }
    val project = projects.firstOrNull { it.id == draft.projectId }
    ContextChip(
        project?.name ?: "No project",
        leading = {
            if (project != null) ProjectTile(project.name, project.colorIndex.toInt(), 18.dp)
            else ZIcon(ZIcons.Home, null, Modifier.size(16.dp))
        },
        onClick = { open = true },
    )
    if (open) {
        ProjectSheet(
            draft,
            Machines.groups(projects, hosts),
            onDismiss = { open = false },
            onAdd = onAdd?.let { add -> { source: ProjectSource -> open = false; add(source) } },
            target = target,
        ) {
            onPick(it)
            open = false
        }
    }
}

/**
 * Projects grouped by the machine they live on — this phone beside your
 * computers — each machine also offering "No project" (a session in its home
 * folder), as the desktop's picker does. The choice gains a check.
 */
@Composable
private fun ProjectSheet(
    draft: sh.zeron.android.core.NewSessionDraft,
    machines: List<MachineGroup>,
    onDismiss: () -> Unit,
    onAdd: ((ProjectSource) -> Unit)?,
    target: DeviceView?,
    onPick: (sh.zeron.android.core.NewSessionDraft) -> Unit,
) {
    val sheet = androidx.compose.material3.rememberModalBottomSheetState(skipPartiallyExpanded = true)
    androidx.compose.material3.ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheet,
        containerColor = MaterialTheme.colorScheme.surfaceContainerLow,
    ) {
        OpenCloseFeedback()
        androidx.compose.foundation.lazy.LazyColumn(contentPadding = androidx.compose.foundation.layout.PaddingValues(bottom = 24.dp)) {
            item {
                Text(
                    "Project",
                    style = MaterialTheme.typography.headlineSmallEmphasized,
                    modifier = Modifier.padding(start = 24.dp, bottom = 8.dp),
                )
            }
            for (machine in machines) {
                item("h-${machine.deviceId}") {
                    androidx.compose.foundation.layout.Row(
                        Modifier.padding(start = 24.dp, end = 24.dp, top = 16.dp, bottom = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        ZIcon(DeviceIdentity.icon(machine.platform), null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                        Spacer(Modifier.size(8.dp))
                        Text(machine.name, style = MaterialTheme.typography.titleSmallEmphasized, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        Spacer(Modifier.size(8.dp))
                        Box(
                            Modifier.size(8.dp).clip(androidx.compose.foundation.shape.CircleShape).background(
                                if (machine.online) successColor() else MaterialTheme.colorScheme.outlineVariant,
                            ),
                        )
                        val note = when {
                            machine.isSelf -> "This phone"
                            !machine.online -> "Offline"
                            else -> null
                        }
                        note?.let {
                            Spacer(Modifier.size(6.dp))
                            Text(it, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.outline)
                        }
                    }
                }
                val rows = machine.projects.size + 1
                machine.projects.forEachIndexed { i, p ->
                    item(p.id) {
                        ProjectRow(
                            selected = p.id == draft.projectId,
                            index = i,
                            count = rows,
                            leading = { ProjectTile(p.name, p.colorIndex.toInt(), 40.dp) },
                            title = p.name,
                            supporting = p.path.replace(Regex("^/(Users|home)/[^/]+"), "~"),
                            mono = true,
                        ) { onPick(draft.copy(projectId = p.id, hostId = null, branch = null, cwd = null)) }
                    }
                }
                item("none-${machine.deviceId}") {
                    ProjectRow(
                        selected = draft.projectId == null && draft.hostId == machine.deviceId,
                        index = rows - 1,
                        count = rows,
                        leading = { IconTile(ZIcons.Home) },
                        title = "No project",
                        supporting = if (machine.isSelf) "Run in this phone's home folder" else "Run in ${machine.name}'s home folder",
                        mono = false,
                    ) { onPick(draft.copy(projectId = null, hostId = machine.deviceId, worktree = false, branch = null, cwd = null)) }
                }
            }
            if (onAdd != null && target != null) {
                val sources = listOf(ProjectSource.Clone, ProjectSource.Empty)
                item("add-title") {
                    Text(
                        if (target.isSelf) "New project on this phone" else "New project on ${target.name}",
                        style = MaterialTheme.typography.titleSmallEmphasized,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(start = 24.dp, top = 16.dp, bottom = 8.dp),
                    )
                }
                sources.forEachIndexed { i, source ->
                    item("add-$source") {
                        val clone = source == ProjectSource.Clone
                        ProjectRow(
                            selected = false,
                            index = i,
                            count = sources.size,
                            leading = { IconTile(if (clone) ZIcons.Branch else ZIcons.Plus) },
                            title = if (clone) "Clone repository" else "Empty project",
                            supporting = projectDestination(source, target),
                            mono = false,
                        ) { onAdd(source) }
                    }
                }
            }
        }
    }
}

@Composable
private fun ProjectRow(
    selected: Boolean,
    index: Int,
    count: Int,
    leading: @Composable () -> Unit,
    title: String,
    supporting: String,
    mono: Boolean,
    onClick: () -> Unit,
) {
    androidx.compose.material3.SegmentedListItem(
        onClick = tapAction(action = onClick),
        shapes = segmentedShapes(index, count),
        colors = androidx.compose.material3.ListItemDefaults.segmentedColors(
            containerColor = if (selected) MaterialTheme.colorScheme.secondaryContainer else cardColor(),
        ),
        leadingContent = leading,
        supportingContent = {
            Text(
                supporting,
                style = MaterialTheme.typography.bodySmall,
                fontFamily = if (mono) sh.zeron.android.design.GeistMono else null,
                maxLines = 1,
                overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
            )
        },
        trailingContent = if (selected) {
            {
                Box(
                    Modifier.size(28.dp).clip(androidx.compose.foundation.shape.CircleShape).background(MaterialTheme.colorScheme.primary),
                    contentAlignment = Alignment.Center,
                ) { ZIcon(ZIcons.Check, "Selected", Modifier.size(16.dp), tint = MaterialTheme.colorScheme.onPrimary) }
            }
        } else {
            null
        },
        modifier = Modifier.padding(horizontal = 16.dp).padding(bottom = androidx.compose.material3.ListItemDefaults.SegmentedGap),
    ) { Text(title, style = MaterialTheme.typography.titleMedium) }
}

@Composable
private fun HostChip(
    draft: sh.zeron.android.core.NewSessionDraft,
    hosts: List<uniffi.zeron_core.DeviceView>,
    onPick: (sh.zeron.android.core.NewSessionDraft) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    val host = hosts.firstOrNull { it.id == draft.hostId }
    ContextChip(
        host?.name ?: "Choose device",
        leading = { ZIcon(DeviceIdentity.icon(host?.platform.orEmpty()), null, Modifier.size(16.dp)) },
        onClick = { open = true },
    ) {
        ChoiceMenu(open, { open = false }, listOf(MenuSection("Run on", hosts.map { h ->
            MenuChoice(
                h.name,
                h.id == draft.hostId,
                if (h.isSelf) "This phone" else if (h.online) "Online" else "Offline",
                leading = { ZIcon(DeviceIdentity.icon(h.platform), null, Modifier.size(18.dp)) },
            ) { onPick(draft.copy(hostId = h.id)) }
        })))
    }
}

@Composable
private fun ModelChip(
    app: AppModel,
    draft: sh.zeron.android.core.NewSessionDraft,
    choice: ModelChoice?,
    models: List<ModelChoice>,
    statuses: List<CatalogStatus>,
    onRetry: () -> Unit,
    update: ((sh.zeron.android.core.NewSessionDraft) -> sh.zeron.android.core.NewSessionDraft) -> Unit,
) {
    val favorites by app.favorites.favorites.collectAsState()
    // Never the harness name in place of a model.
    val title = choice?.model?.label ?: draft.model?.let { modelLabel(draft.harness, it) }
        ?: fallbackModels(draft.harness).firstOrNull()?.label ?: harnessLabel(draft.harness)
    ModelPickerChip(
        catalog = models,
        current = choice,
        harness = choice?.harness ?: draft.harness,
        fallbackLabel = title,
        favorites = favorites,
        onToggleFavorite = app.favorites::toggle,
        // A new model starts from its own defaults.
        onPick = { m -> update { it.copy(harness = m.harness, model = m.model.id, effort = null, options = emptyMap()) } },
        effort = draft.effort,
        onEffort = { level -> update { it.copy(effort = level) } },
        options = draft.options,
        onOptions = { picks -> update { it.copy(options = picks) } },
        statuses = statuses,
        onRetry = { onRetry() },
    )
}
