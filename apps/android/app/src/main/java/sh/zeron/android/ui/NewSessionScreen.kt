package sh.zeron.android.ui

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
import sh.zeron.android.core.ProjectSource
import sh.zeron.android.core.userMessage
import sh.zeron.android.design.HarnessMark
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.DeviceView
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

    val projects = workspace?.projects.orEmpty()
    val hosts = workspace?.devices.orEmpty().filter { it.isExecutionHost }
    val project = projects.firstOrNull { it.id == draft.projectId }
    LaunchedEffect(workspace) {
        // A remembered project or host that is gone (removed, another account).
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
    LaunchedEffect(deviceId, client) {
        val c = client ?: return@LaunchedEffect
        if (deviceId.isEmpty()) return@LaunchedEffect
        val harnesses = runCatching { c.listHarnesses(deviceId) }.getOrNull() ?: return@LaunchedEffect
        val fresh = harnesses.filter { it.offered }.flatMap { h ->
            (runCatching { c.listModels(deviceId, h.id) }.getOrNull() ?: fallbackModels(h.id)).map { ModelChoice(h.id, h.label, it) }
        }
        if (fresh.isNotEmpty()) {
            modelCache[deviceId] = fresh
            models = fresh
            // The live list holds only harnesses installed on this device: a
            // computer may have OpenCode but not the Claude Code default, and
            // sending to a missing harness just fails the turn.
            if (fresh.none { it.harness == draft.harness }) {
                val first = fresh.first()
                draft = draft.copy(harness = first.harness, model = first.model.id, effort = null)
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
        if (id != null) onCreated(id) else scope.launch { snackbar.showSnackbar("Choose a project or a host that can run it.") }
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
                    onPick = { draft = it },
                    onAdd = if (model.isDemo || target == null) null else { source -> adding = source },
                    target = target,
                )
                if (project != null) {
                    if (project.gitDetected) BranchChip(model, draft, project.deviceId, project.path, onCurrent = { currentBranch = it }) { draft = it }
                } else {
                    HostChip(draft, hosts) { draft = it }
                }
                ModelChip(model, draft, choice, models) { draft = it }
                val efforts = choice?.model?.reasoningLevels.orEmpty()
                if (efforts.isNotEmpty()) {
                    val effort = draft.effort?.takeIf { it in efforts } ?: choice?.model?.defaultReasoning ?: efforts[efforts.size / 2]
                    EffortChip(effort, efforts) { draft = draft.copy(effort = it) }
                }
            }
        }
    }
    adding?.let { source ->
        NewProjectDialog(source, target, onDismiss = { adding = null }) { input ->
            model.addProject(targetDevice, source, input).onSuccess { id ->
                adding = null
                draft = draft.copy(projectId = id, hostId = null, branch = null, cwd = null)
            }.exceptionOrNull()?.userMessage()
        }
    }
}
}

/**
 * Clone a repository or start an empty one on `device` — its engine does it
 * (`CloneRepo` / `CreateRepo`) — then make it a project. `submit` returns an
 * error to show, or null once it's done.
 */
@Composable
private fun NewProjectDialog(source: ProjectSource, device: DeviceView?, onDismiss: () -> Unit, submit: suspend (String) -> String?) {
    var text by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val clone = source == ProjectSource.Clone
    val valid = if (clone) ProjectNames.repoName(text) != null else ProjectNames.folderName(text) != null
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
            androidx.compose.material3.TextButton(onClick = ::go, enabled = valid && !busy) { Text(if (clone) "Clone" else "Create") }
        },
        dismissButton = { androidx.compose.material3.TextButton(onClick = onDismiss, enabled = !busy) { Text("Cancel") } },
    )
}

/** Where a new project lands, in words. */
private fun projectDestination(source: ProjectSource, device: DeviceView?): String {
    val where = device?.name ?: "the device"
    return when (source) {
        ProjectSource.Clone -> "Cloned into $where by its engine."
        ProjectSource.Empty -> "An empty git repository in $where."
    }
}

/** What the new-project dialog accepts (JVM-tested). */
object ProjectNames {
    /**
     * Folder name a `git clone <url>` creates: the last path segment without
     * `.git` (`git@host:org/repo.git` → `repo`). `null` for nothing usable.
     */
    fun repoName(url: String): String? {
        val trimmed = url.trim().trimEnd('/').removeSuffix(".git")
        // A bare word is not a repository URL.
        if (!trimmed.contains('/') && !trimmed.contains(':')) return null
        return folderName(trimmed.substringAfterLast('/').substringAfterLast(':'))
    }

    /** A safe single folder name, or null. */
    fun folderName(name: String): String? =
        name.trim().takeIf { it.isNotEmpty() && it != "." && it != ".." && it.all { c -> c.isLetterOrDigit() || c in "._-" } }
}

@Composable
private fun ProjectChip(
    draft: sh.zeron.android.core.NewSessionDraft,
    projects: List<uniffi.zeron_core.ProjectView>,
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
            projects,
            onDismiss = { open = false },
            onAdd = onAdd?.let { add -> { source: ProjectSource -> open = false; add(source) } },
            target = target,
        ) {
            onPick(it)
            open = false
        }
    }
}

/** Projects grouped by the machine they live on; the choice keeps its tile and gains a check. */
@Composable
private fun ProjectSheet(
    draft: sh.zeron.android.core.NewSessionDraft,
    projects: List<uniffi.zeron_core.ProjectView>,
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
        androidx.compose.foundation.lazy.LazyColumn(contentPadding = androidx.compose.foundation.layout.PaddingValues(bottom = 24.dp)) {
            item {
                Text(
                    "Project",
                    style = MaterialTheme.typography.headlineSmallEmphasized,
                    modifier = Modifier.padding(start = 24.dp, bottom = 8.dp),
                )
            }
            val byDevice = projects.groupBy { it.deviceId }.values.sortedByDescending { it.first().deviceOnline }
            for (group in byDevice) {
                val host = group.first()
                item("h-${host.deviceId}") {
                    androidx.compose.foundation.layout.Row(
                        Modifier.padding(start = 24.dp, end = 24.dp, top = 16.dp, bottom = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        ZIcon(ZIcons.Laptop, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                        Spacer(Modifier.size(8.dp))
                        Text(host.deviceName ?: "Host", style = MaterialTheme.typography.titleSmallEmphasized, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        Spacer(Modifier.size(8.dp))
                        Box(
                            Modifier.size(8.dp).clip(androidx.compose.foundation.shape.CircleShape).background(
                                if (host.deviceOnline) successColor() else MaterialTheme.colorScheme.outlineVariant,
                            ),
                        )
                        if (!host.deviceOnline) {
                            Spacer(Modifier.size(6.dp))
                            Text("Offline", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.outline)
                        }
                    }
                }
                group.forEachIndexed { i, p ->
                    item(p.id) {
                        ProjectRow(
                            selected = p.id == draft.projectId,
                            index = i,
                            count = group.size,
                            leading = { ProjectTile(p.name, p.colorIndex.toInt(), 40.dp) },
                            title = p.name,
                            supporting = p.path.replace(Regex("^/(Users|home)/[^/]+"), "~"),
                            mono = true,
                        ) { onPick(draft.copy(projectId = p.id, hostId = null, branch = null, cwd = null)) }
                    }
                }
            }
            item("none") {
                Spacer(Modifier.size(16.dp))
                ProjectRow(
                    selected = draft.projectId == null,
                    index = 0,
                    count = 1,
                    leading = { IconTile(ZIcons.Home) },
                    title = "No project",
                    supporting = "Run in a host's home folder",
                    mono = false,
                ) { onPick(draft.copy(projectId = null, hostId = draft.hostId ?: projects.firstOrNull()?.deviceId, worktree = false, branch = null, cwd = null)) }
            }
            if (onAdd != null) {
                val sources = listOf(ProjectSource.Clone, ProjectSource.Empty)
                item("add-title") {
                    Text(
                        "New project${target?.let { " on ${it.name}" }.orEmpty()}",
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
                            supporting = if (clone) "The device's engine clones it" else "A new git repository on the device",
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
        onClick = onClick,
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
    ContextChip(
        hosts.firstOrNull { it.id == draft.hostId }?.name ?: "Choose host",
        leading = { ZIcon(ZIcons.Laptop, null, Modifier.size(16.dp)) },
        onClick = { open = true },
    ) {
        ChoiceMenu(open, { open = false }, listOf(MenuSection("Run on", hosts.map { h ->
            MenuChoice(h.name, h.id == draft.hostId, if (h.online) "Online" else "Offline") { onPick(draft.copy(hostId = h.id)) }
        })))
    }
}

@Composable
private fun ModelChip(
    app: AppModel,
    draft: sh.zeron.android.core.NewSessionDraft,
    choice: ModelChoice?,
    models: List<ModelChoice>,
    onPick: (sh.zeron.android.core.NewSessionDraft) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    val favorites by app.favorites.favorites.collectAsState()
    val latest by androidx.compose.runtime.rememberUpdatedState(draft)
    // Never the harness name in place of a model.
    val title = choice?.model?.label ?: draft.model?.let { modelLabel(draft.harness, it) }
        ?: fallbackModels(draft.harness).firstOrNull()?.label ?: harnessLabel(draft.harness)
    ContextChip(title, leading = { HarnessMark(draft.harness, 14.dp) }, onClick = { open = true }) {
        ModelPickerPopover(
            expanded = open,
            onDismiss = { open = false },
            catalog = models,
            current = choice,
            favorites = favorites,
            onToggleFavorite = app.favorites::toggle,
            onPick = { m -> onPick(latest.copy(harness = m.harness, model = m.model.id, effort = null)) },
            labelFor = { harnessLabel(it) },
        )
    }
}

@Composable
private fun EffortChip(effort: String, efforts: List<String>, onPick: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    ContextChip(reasoningLabel(effort), leading = { ZIcon(ZIcons.Effort, null, Modifier.size(16.dp)) }, onClick = { open = true }) {
        ChoiceMenu(open, { open = false }, listOf(MenuSection("Reasoning effort", efforts.map { e ->
            MenuChoice(reasoningLabel(e), e == effort) { onPick(e) }
        })))
    }
}
