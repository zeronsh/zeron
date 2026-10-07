package sh.zeron.android.ui

import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.res.pluralStringResource
import sh.zeron.android.design.BackButton
import sh.zeron.android.design.consumeBlankTaps
import sh.zeron.android.R
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
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
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch
import sh.zeron.android.core.Machine
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.AnchoredMenu
import sh.zeron.android.design.BrandMark
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.Glyphs
import sh.zeron.android.design.LocalZeronColors
import sh.zeron.android.design.MenuEntry
import sh.zeron.android.design.ZeronType
import sh.zeron.android.design.menuSection
import uniffi.zeron_core.BusyPolicy
import uniffi.zeron_core.CatalogSource
import uniffi.zeron_core.CoreClient
import uniffi.zeron_core.DirectPhase
import uniffi.zeron_core.HarnessCatalog
import uniffi.zeron_core.ModelCatalog
import uniffi.zeron_core.ModelInfo
import uniffi.zeron_core.ModelOption
import uniffi.zeron_core.NewSession
import uniffi.zeron_core.RepoRef
import uniffi.zeron_core.SendRequest
import uniffi.zeron_core.SessionTarget
import uniffi.zeron_core.WorktreeSpec
import uniffi.zeron_core.fileMentionLink
import uniffi.zeron_core.harnessLabel
import uniffi.zeron_core.modelLabel
import uniffi.zeron_core.reasoningLabel

/** One model on one harness, as the model menu lists them (iOS ModelChoice). */
internal data class ModelChoice(
    val harness: String,
    val harnessLabel: String,
    val id: String,
    val label: String,
    val efforts: List<String>,
    /** What else the model offers (Codex service tier, Claude's 1M context…). */
    val options: List<ModelOption> = emptyList(),
    /**
     * Who serves the model when the harness routes several providers (pi's
     * `AG.20`/`AG.50`, opencode's): the row's description like the desktop
     * picker, else the `provider/` prefix of its id. Shown only where it
     * tells variants apart (see [ambiguousRows]).
     */
    val provider: String? = null,
) {
    /** The model id without its `provider/` prefix (`AG.20/gpt-6-astra` → `gpt-6-astra`). */
    val baseId: String get() = id.substringAfter('/')

    companion object {
        fun of(harness: String, harnessLabel: String, m: ModelInfo) =
            ModelChoice(harness, harnessLabel, m.id, m.label, m.reasoningLevels, m.options, providerOf(m, harnessLabel))
    }
}

/**
 * A model row's provider text: its description (desktop pickers.rs shows
 * that one, unless it only repeats the harness name), else the `provider/`
 * prefix of its id (pi-acp sends no description: `AG.20/gpt-6-astra`).
 */
internal fun providerOf(m: ModelInfo, harnessLabel: String): String? =
    m.description?.trim()?.takeIf { it.isNotEmpty() && !it.equals(harnessLabel, ignoreCase = true) }
        ?: m.id.substringBefore('/', "").trim().takeIf { it.isNotEmpty() }

/**
 * The rows (harness, id) that need their provider to be told apart, like
 * the desktop's `mark_ambiguous` but wider: another row of the same harness
 * has the same name, or the same model under another provider (same id
 * after the `provider/` prefix).
 */
internal fun ambiguousRows(rows: List<ModelChoice>): Set<Pair<String, String>> {
    val names = rows.groupingBy { it.harness to it.label.trim() }.eachCount()
    val bases = rows.groupingBy { it.harness to it.baseId }.eachCount()
    return rows
        .filter { names.getValue(it.harness to it.label.trim()) > 1 || bases.getValue(it.harness to it.baseId) > 1 }
        .mapTo(HashSet()) { it.harness to it.id }
}

/** The provider line under a row: only for a row [ambiguousRows] flagged. */
internal fun ModelChoice.providerLine(ambiguous: Set<Pair<String, String>>): String? =
    provider?.takeIf { (harness to id) in ambiguous }

/** The name with its provider when it needs one ("GPT-6 Astra · AG.20"), for the chip and the CLI row. */
internal fun ModelChoice.titleAmong(ambiguous: Set<Pair<String, String>>): String =
    providerLine(ambiguous)?.let { "$label · $it" } ?: label

/**
 * A host's model menu: every offered harness's models, plus the harnesses
 * still on the built-in list (never read from this computer: 「没能从电脑读取
 * 最新列表 · 重试」 / "Couldn't get the latest list · Retry"). A list the computer gave (live now, or saved from an
 * earlier read) is the real one and gets no retry row.
 */
internal data class HostCatalog(
    val models: List<ModelChoice>,
    val stale: Map<String, CatalogSource> = emptyMap(),
    /** Why each stale harness's live read failed (shown under its retry row). */
    val errors: Map<String, String> = emptyMap(),
) {
    /**
     * [harness]'s list swapped for [fresh] (other harnesses untouched). A
     * built-in list never replaces one the computer gave: a failed read
     * keeps what shows.
     */
    fun with(harness: String, fresh: List<ModelChoice>, source: CatalogSource, error: String? = null): HostCatalog {
        val builtIn = source == CatalogSource.STATIC
        if (builtIn && harness !in stale && models.any { it.harness == harness }) return this
        val at = models.indexOfFirst { it.harness == harness }.takeIf { it >= 0 } ?: models.size
        val rest = models.filter { it.harness != harness }
        val merged = rest.take(at.coerceAtMost(rest.size)) + fresh + rest.drop(at.coerceAtMost(rest.size))
        return HostCatalog(
            merged,
            if (builtIn) stale + (harness to source) else stale - harness,
            if (!builtIn || error == null) errors - harness else errors + (harness to error),
        )
    }

    /**
     * A background read's answer laid over what shows: its CLIs, in its
     * order; each CLI's new list unless that is the built-in one and a list
     * from the computer already shows.
     */
    /**
     * A forced re-read's answer ([fresh], or [thrown] when the call itself
     * failed) laid over what shows, with why it failed (null: the computer
     * answered). A live list replaces [harness]'s. A failed one never
     * downgrades: a list the computer gave stays exactly as shown; a CLI
     * still on the built-in list takes the saved one if the core has it,
     * else keeps the reason under its retry row.
     */
    fun refreshed(harness: String, fresh: HarnessList?, thrown: String? = null): Pair<HostCatalog, String?> {
        if (fresh != null && fresh.source == CatalogSource.LIVE && fresh.models.isNotEmpty()) {
            return with(harness, fresh.models, CatalogSource.LIVE) to null
        }
        val why = fresh?.error ?: thrown ?: "no answer from the computer"
        if (harness !in stale) return this to why
        val next = if (fresh != null && fresh.models.isNotEmpty()) with(harness, fresh.models, fresh.source, why) else this
        return (if (harness in next.stale) next.copy(errors = next.errors + (harness to why)) else next) to why
    }

    fun merge(fresh: HostCatalog): HostCatalog =
        fresh.models.map { it.harness }.distinct().fold(HostCatalog(emptyList())) { out, h ->
            val shown = models.filter { it.harness == h }
            if (h in fresh.stale && h !in stale && shown.isNotEmpty()) {
                out.with(h, shown, CatalogSource.SAVED)
            } else {
                out.with(h, fresh.models.filter { it.harness == h }, fresh.stale[h] ?: CatalogSource.LIVE, fresh.errors[h])
            }
        }

    companion object {
        /** The core's saved lists for a computer (disk only), as the menu shows them. */
        fun fromSaved(saved: List<HarnessCatalog>): HostCatalog =
            saved.filter { it.harness.offered }.fold(HostCatalog(emptyList())) { out, part ->
                val h = part.harness
                val list = part.catalog.models.ifEmpty { uniffi.zeron_core.fallbackModels(h.id) }
                out.with(h.id, list.map { ModelChoice.of(h.id, h.label, it) }, part.catalog.source, part.catalog.error)
            }

        /** The core's failure text, short enough for a menu subtitle. */
        fun reason(error: String): String {
            val plain = error.removePrefix("host unavailable: ").removePrefix("host error: ").trim()
            return if (plain.length > 120) plain.take(119) + "…" else plain
        }
    }
}

/** Screenshot tests: stand-ins for the host's ListModels answers. */
internal object CatalogHooks {
    var models: ((harness: String, force: Boolean) -> ModelCatalog)? = null
}

private fun catalogModels(): List<ModelChoice> =
    uniffi.zeron_core.fallbackHarnesses().filter { it.offered }.flatMap { h ->
        uniffi.zeron_core.fallbackModels(h.id).map { ModelChoice.of(h.id, h.label, it) }
    }

/** One harness's models, where they came from, and why a live read failed. */
internal data class HarnessList(val models: List<ModelChoice>, val source: CatalogSource, val error: String?)

/** One harness's models. `force` re-probes the CLI on the computer. */
private suspend fun harnessModels(client: CoreClient, device: String, harness: String, label: String, force: Boolean): HarnessList {
    val catalog = CatalogHooks.models?.invoke(harness, force) ?: client.modelCatalog(device, harness, force)
    val models = catalog.models.ifEmpty { uniffi.zeron_core.fallbackModels(harness) }
    return HarnessList(models.map { ModelChoice.of(harness, label, it) }, catalog.source, catalog.error)
}

/**
 * What the sheet opens on, at once: the lists saved for this computer
 * (the core reads them from disk, never from the computer), built-in only
 * for what was never read.
 */
private fun savedModels(client: CoreClient, device: String): HostCatalog {
    if (device.isEmpty()) return HostCatalog(catalogModels())
    val saved = CatalogHooks.models?.let { hook ->
        uniffi.zeron_core.fallbackHarnesses().map { h -> HarnessCatalog(h, hook(h.id, false)) }
    } ?: runCatching { client.savedCatalog(device) }.getOrNull()
    return saved?.let { HostCatalog.fromSaved(it) }?.takeIf { it.models.isNotEmpty() } ?: HostCatalog(catalogModels())
}

/** Every offered harness on the host and its models (ListHarnesses + ListModels). */
private suspend fun hostModels(client: CoreClient, device: String): HostCatalog = coroutineScope {
    val harnesses = client.listHarnesses(device).filter { it.offered }
    // One harness failing (or throwing) mustn't take the others' live lists with it.
    val parts = harnesses.map { h ->
        async {
            runCatching { harnessModels(client, device, h.id, h.label, force = false) }.getOrElse { t ->
                HarnessList(
                    uniffi.zeron_core.fallbackModels(h.id).map { m -> ModelChoice.of(h.id, h.label, m) },
                    CatalogSource.STATIC,
                    t.message,
                )
            }
        }
    }.awaitAll()
    // Only a built-in list is stale; a saved one is the computer's own.
    val byId = harnesses.zip(parts).filter { (_, part) -> part.source == CatalogSource.STATIC }
    HostCatalog(
        models = parts.flatMap { it.models },
        stale = byId.associate { (h, part) -> h.id to part.source },
        errors = byId.mapNotNull { (h, part) -> part.error?.let { h.id to it } }.toMap(),
    )
}

/**
 * New Session, like iOS NewSessionViewController: the composer's chips pick
 * the project (projects grouped by computer, No Project, New Project…), the
 * host when there's no project, the branch or a new worktree for git
 * projects, the model (grouped by harness) and the reasoning effort.
 */
@Composable
fun NewSessionSheet(model: ZeronModel, onDismiss: () -> Unit, embedded: Boolean = false, autofocus: Boolean = true) {
    val colors = LocalZeronColors.current
    val context = LocalContext.current
    val client = model.client ?: return
    val projects = model.workspace?.projects.orEmpty()
    val hosts = remember(model.workspace) { client.executionDevices() }
    var text by remember { mutableStateOf("") }
    // The project it was opened from, else the last pick on this computer,
    // else the most recently used project (not just the first in the list).
    val opening = remember {
        NewSessionMemory.initial(
            explicit = model.newSessionProject,
            remembered = NewSessionMemory.load(context, model.activeMachine),
            projects = projects,
            hosts = hosts.map { it.id },
            id = { it.id },
            lastUsedMs = { p -> p.sessions.maxOfOrNull { it.lastActivityMs } ?: p.createdAtMs },
        )
    }
    var projectId by remember { mutableStateOf(opening.projectId) }
    var hostId by remember { mutableStateOf(if (projectId == null) opening.hostId ?: (hosts.firstOrNull { it.online } ?: hosts.firstOrNull())?.id else null) }
    // Remember where this one goes when the sheet closes (sent or not), like iOS.
    val picks by androidx.compose.runtime.rememberUpdatedState(NewSessionMemory.Picks(projectId, if (projectId == null) hostId else null))
    androidx.compose.runtime.DisposableEffect(model.activeMachine) {
        val machine = model.activeMachine
        onDispose { NewSessionMemory.save(context, machine, picks) }
    }
    var harness by remember { mutableStateOf("claude-code") }
    var modelId by remember { mutableStateOf<String?>(null) }
    var effort by remember { mutableStateOf<String?>(null) }
    // Model options picked here (option id → choice id); unpicked = the model's default.
    var optionPicks by remember { mutableStateOf<Map<String, String>>(emptyMap()) }
    var branch by remember { mutableStateOf<String?>(null) }
    var worktree by remember { mutableStateOf(false) }
    var browsing by remember { mutableStateOf(false) }
    var addedNames by remember { mutableStateOf<Map<String, String>>(emptyMap()) }
    var chipMenu by remember { mutableStateOf<Pair<String, Rect>?>(null) }
    // Drill-downs inside the open chip menu: a harness's models, the project sort choice.
    var modelHarness by remember { mutableStateOf<String?>(null) }
    var projectSortOpen by remember { mutableStateOf(false) }
    var refs by remember { mutableStateOf<List<RepoRef>?>(null) }
    LaunchedEffect(Unit) { model.newSessionProject = null }
    var scheduleOpen by remember { mutableStateOf(false) }
    val pendingNew = rememberScheduledNewSessions(model.activeMachine)
    val notificationPermission = androidx.activity.compose.rememberLauncherForActivityResult(
        androidx.activity.result.contract.ActivityResultContracts.RequestPermission(),
    ) { }

    val project = projects.firstOrNull { it.id == projectId }
    val device = project?.deviceId ?: hostId ?: hosts.firstOrNull()?.id ?: ""
    // The lists saved for this computer, at once (never waits on it).
    var catalog by remember { mutableStateOf(savedModels(client, device)) }
    val models = catalog.models
    // The core keeps the saved lists current (on connect, then every 30
    // min); this background read only picks up what changed since. A sheet
    // opened while connecting reads again once the link is up.
    val linkUp = model.directStatus?.phase.let { it == null || it == DirectPhase.LIVE || it == DirectPhase.SYNCING }
    var loading by remember(device) { mutableStateOf(false) }
    LaunchedEffect(device, linkUp) {
        catalog = savedModels(client, device)
        if (device.isEmpty()) return@LaunchedEffect
        loading = true
        val fresh = runCatching { hostModels(client, device) }.getOrNull()
        loading = false
        if (fresh != null && fresh.models.isNotEmpty()) catalog = catalog.merge(fresh)
    }
    val scope = rememberCoroutineScope()
    var refreshing by remember(device) { mutableStateOf<Set<String>>(emptySet()) }
    // How the last refresh of a saved/live list went (harness → line under
    // the refresh row), shown for a few seconds.
    var refreshNotes by remember(device) { mutableStateOf<Map<String, String>>(emptyMap()) }
    // Retry (a CLI still on the built-in list) or refresh (a list the computer
    // gave): ask the computer to re-probe one CLI now, past the core's
    // half-hour freshness. A failure keeps what shows.
    val refresh: (String) -> Unit = refresh@{ h ->
        if (h in refreshing || device.isEmpty()) return@refresh
        val label = models.firstOrNull { it.harness == h }?.harnessLabel ?: harnessLabel(h)
        refreshing = refreshing + h
        refreshNotes = refreshNotes - h
        scope.launch {
            val result = runCatching { harnessModels(client, device, h, label, force = true) }
            refreshing = refreshing - h
            val (next, failure) = catalog.refreshed(h, result.getOrNull(), result.exceptionOrNull()?.message)
            catalog = next
            if (h in next.stale) return@launch // its retry row says why
            val note = failure?.let { context.getString(R.string.model_list_refresh_failed, HostCatalog.reason(it)) }
                ?: context.getString(R.string.model_list_refreshed)
            refreshNotes = refreshNotes + (h to note)
            kotlinx.coroutines.delay(REFRESH_NOTE_MS)
            if (refreshNotes[h] == note) refreshNotes = refreshNotes - h
        }
    }
    val ambiguous = remember(models) { ambiguousRows(models) }
    // Keep the pick valid for this host's catalog: same harness first.
    val current = models.firstOrNull { it.harness == harness && it.id == modelId }
        ?: models.firstOrNull { it.harness == harness }
        ?: models.firstOrNull()
    LaunchedEffect(current) {
        if (current != null && (current.harness != harness || current.id != modelId)) {
            harness = current.harness
            modelId = current.id
        }
    }
    LaunchedEffect(chipMenu?.first, project?.id) {
        if (chipMenu?.first == "branch" && project != null) {
            refs = null
            refs = runCatching { client.listRefs(project.deviceId, project.path) }.getOrDefault(emptyList()).sortedByDescending { it.current }
        }
    }

    val chips = buildList {
        if (projectId != null) {
            val name = project?.name ?: addedNames[projectId] ?: stringResource(R.string.project)
            add(Chip("project", name, colorIndex = project?.colorIndex?.toInt() ?: 0))
            if (project?.gitDetected == true) add(Chip("branch", if (worktree) stringResource(R.string.new_worktree) else branch ?: stringResource(R.string.current_branch)))
        } else {
            add(Chip("project", stringResource(R.string.no_project)))
            add(Chip("host", hosts.firstOrNull { it.id == hostId }?.name ?: stringResource(R.string.choose_host)))
        }
        // Never the harness name in place of a model.
        val modelTitle = current?.titleAmong(ambiguous) ?: modelId?.let { modelLabel(harness, it) } ?: harnessLabel(harness)
        add(Chip("model", modelTitle, harness = harness))
        // One traits chip like desktop's: the effort plus any non-default option ("X-High · Fast").
        val efforts = current?.efforts.orEmpty()
        val options = current?.options.orEmpty().filter { it.choices.size > 1 }
        if (efforts.isNotEmpty() || options.isNotEmpty()) {
            val parts = buildList {
                if (efforts.isNotEmpty()) add(reasoningLabel(effort ?: efforts[efforts.size / 2]))
                options.forEach { o ->
                    val pick = optionPicks[o.id]?.takeIf { it != o.defaultChoice }
                    pick?.let { id -> o.choices.firstOrNull { it.id == id } }?.let { add(it.label) }
                }
            }
            add(Chip("effort", parts.joinToString(" · ").ifEmpty { options.first().label }))
        }
    }

    Box(Modifier.fillMaxSize()) {
        Column(
            Modifier
                .fillMaxSize()
                .background(colors.background)
                .consumeBlankTaps()
                .statusBarsPadding()
                .windowInsetsPadding(WindowInsets.ime.union(WindowInsets.navigationBars))
                .padding(horizontal = 12.dp),
        ) {
            Row(Modifier.fillMaxWidth().padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                if (embedded) Spacer(Modifier.width(44.dp)) else BackButton(colors, onClick = onDismiss)
                Text(stringResource(R.string.new_session), color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp, modifier = Modifier.weight(1f), textAlign = androidx.compose.ui.text.style.TextAlign.Center)
                Spacer(Modifier.width(44.dp))
            }
            Column(Modifier.weight(1f).fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center) {
                BrandMark(harness, colors, 34.dp)
                Spacer(Modifier.height(14.dp))
                Text(stringResource(R.string.new_session_headline), color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 22.sp)
            }
            Box(Modifier.fillMaxWidth().padding(bottom = 8.dp), contentAlignment = Alignment.Center) {
                Column(Modifier.widthIn(max = 768.dp).fillMaxWidth()) {
                    pendingNew.forEach { message ->
                        ScheduledChip(colors, message, detail = message.newSession?.label) {
                            sh.zeron.android.schedule.ScheduledAlarms.cancel(context, message.id)
                            if (text.isBlank()) text = message.text
                            model.showToast(context.getString(R.string.schedule_cancelled))
                        }
                    }
                    ComposerBar(
                        colors = colors,
                        text = text,
                        onText = { text = it },
                        placeholder = stringResource(R.string.new_session_placeholder),
                        running = false,
                        canSteer = false,
                        focused = autofocus,
                        onFocus = {},
                        chips = chips,
                        onChip = { chip, rect ->
                            modelHarness = null
                            projectSortOpen = false
                            chipMenu = chip.id to rect
                        },
                        images = emptyList(),
                        onRemoveImage = {},
                        onSend = send@{
                            val body = text.trim()
                            if (body.isEmpty()) return@send
                            val target = when {
                                projectId != null -> SessionTarget.Project(projectId!!)
                                hostId != null -> SessionTarget.Projectless(hostId!!)
                                else -> {
                                    model.showToast(context.getString(R.string.choose_project_or_host))
                                    return@send
                                }
                            }
                            try {
                                val id = client.createSession(NewSession(target, model.defaultConfig(harness, modelId, effort, optionsFor(current, optionPicks)), if (worktree) null else branch, null, null))
                                val handle = client.openSession(id)
                                val spec = if (worktree && project != null) WorktreeSpec(project.path, branch ?: "HEAD", project.id) else null
                                handle.send(SendRequest(body, emptyList(), spec, BusyPolicy.QUEUE))
                                handle.close()
                                onDismiss()
                                model.openSession(id)
                            } catch (t: Throwable) {
                                model.showToast(t.message ?: context.getString(R.string.start_session_failed))
                            }
                        },
                        mentionSearch = search@{ q ->
                            val p = project ?: return@search emptyList()
                            runCatching { client.searchFiles(p.deviceId, null, p.id, q) }.getOrDefault(emptyList())
                        },
                        onMention = { path, dir -> text = text.replace(Regex("@[^\\s]*$"), fileMentionLink(path, dir) + " ") },
                        onSchedule = { scheduleOpen = true },
                    )
                }
            }
        }
        chipMenu?.let { (id, anchor) ->
            val close = { chipMenu = null }
            val (title, entries) = when (id) {
                "project" -> if (projectSortOpen) {
                    // The sort toggle beside a computer's name: pick the order, back to the list.
                    stringResource(R.string.sort_projects) to listOf(
                        MenuEntry(stringResource(R.string.project), back = true) { projectSortOpen = false },
                        MenuEntry(stringResource(R.string.sort_recent), checked = model.projectSort == ZeronModel.ProjectSort.Recent, keepOpen = true, icon = { c -> Glyph(Glyphs.Recent, 17.dp, c) }) {
                            model.applyProjectSort(ZeronModel.ProjectSort.Recent)
                            projectSortOpen = false
                        },
                        MenuEntry(stringResource(R.string.sort_name), checked = model.projectSort == ZeronModel.ProjectSort.Name, keepOpen = true, icon = { c -> SortAlphaMark(c) }) {
                            model.applyProjectSort(ZeronModel.ProjectSort.Name)
                            projectSortOpen = false
                        },
                    )
                } else stringResource(R.string.project) to buildList {
                    // Projects grouped by computer, like the iOS inline sections;
                    // within a computer, in the remembered order (ProjectOrder).
                    val sortLabel = stringResource(R.string.sort_projects)
                    projects.groupBy { it.deviceId }.entries
                        .sortedBy { (_, list) -> list.first().deviceName ?: "" }
                        .forEach { (_, unsorted) ->
                            val list = when (model.projectSort) {
                                ZeronModel.ProjectSort.Name -> ProjectOrder.byName(unsorted) { it.name }
                                ZeronModel.ProjectSort.Recent -> ProjectOrder.byRecent(unsorted) { p -> p.sessions.maxOfOrNull { it.lastActivityMs } ?: p.createdAtMs }
                            }
                            val head = list.first()
                            val hostName = head.deviceName ?: stringResource(R.string.host_fallback)
                            add(
                                MenuEntry(
                                    if (head.deviceOnline) hostName else stringResource(R.string.host_offline_suffix, hostName),
                                    header = true,
                                    icon = { c -> Glyph(Glyphs.Sort, 16.dp, c, Modifier.semantics { contentDescription = sortLabel }) },
                                ) { projectSortOpen = true },
                            )
                            list.forEach { p ->
                                add(MenuEntry(p.name, checked = p.id == projectId, icon = { c -> Glyph(Glyphs.Folder, 17.dp, c) }) {
                                    projectId = p.id
                                    hostId = null
                                    branch = null
                                    worktree = false
                                })
                            }
                        }
                    add(menuSection())
                    add(MenuEntry(stringResource(R.string.no_project_ellipsis), checked = projectId == null, icon = { c -> Glyph(Glyphs.Tray, 17.dp, c) }) {
                        projectId = null
                        hostId = hostId ?: (hosts.firstOrNull { it.online } ?: hosts.firstOrNull())?.id
                    })
                    add(MenuEntry(stringResource(R.string.new_project_ellipsis), icon = { c -> Glyph(Glyphs.FolderPlus, 17.dp, c) }) { browsing = true })
                    add(menuSection())
                    add(MenuEntry(stringResource(R.string.add_computer_ellipsis), icon = { c -> Glyph(Glyphs.Computer, 17.dp, c) }) { model.editMachine = Machine() })
                }
                "host" -> stringResource(R.string.run_on) to buildList {
                    hosts.forEach { h ->
                        add(MenuEntry(h.name, subtitle = stringResource(if (h.online) R.string.online else R.string.offline), checked = h.id == hostId, icon = { c -> Glyph(Glyphs.Computer, 17.dp, c) }) { hostId = h.id })
                    }
                    add(menuSection())
                    add(MenuEntry(stringResource(R.string.add_computer_ellipsis), icon = { c -> Glyph(Glyphs.Computer, 17.dp, c) }) { model.editMachine = Machine() })
                }
                "branch" -> stringResource(R.string.checkout) to buildList {
                    add(MenuEntry(stringResource(R.string.new_worktree), checked = worktree) { worktree = !worktree })
                    add(menuSection(stringResource(R.string.branch)))
                    val list = refs.orEmpty()
                    list.forEach { ref ->
                        add(MenuEntry(ref.name, subtitle = if (ref.current) stringResource(R.string.checked_out) else null, checked = ref.name == (branch ?: list.firstOrNull()?.name)) { branch = ref.name })
                    }
                    if (refs != null && list.isEmpty()) add(MenuEntry(stringResource(R.string.no_branches)) {})
                }
                "model" -> stringResource(R.string.model) to buildList {
                    // Two levels: the CLIs first (the current one checked, its
                    // model as the subtitle), then the chosen CLI's models.
                    val byHarness = models.groupBy { it.harness }.entries.sortedBy { it.key }
                    val open = modelHarness?.let { h -> byHarness.firstOrNull { it.key == h } }
                    if (open == null) {
                        if (catalog.stale.isNotEmpty()) {
                            // Short here (the CLI rows set the width); the CLI's own list says which list it shows.
                            val names = catalog.stale.keys.sorted().joinToString("、") { h -> models.firstOrNull { it.harness == h }?.harnessLabel ?: harnessLabel(h) }
                            val reading = loading || catalog.stale.keys.any { it in refreshing }
                            add(
                                MenuEntry(
                                    stringResource(if (reading) R.string.model_list_reading else R.string.model_list_stale_short),
                                    subtitle = stringResource(R.string.model_list_retry_for, names),
                                    keepOpen = true,
                                    icon = { c -> Glyph(Glyphs.Refresh, 17.dp, c) },
                                ) { catalog.stale.keys.forEach(refresh) },
                            )
                        }
                        byHarness.forEach { (h, list) ->
                            val selected = h == harness
                            add(
                                MenuEntry(
                                    list.first().harnessLabel,
                                    subtitle = if (selected) current?.titleAmong(ambiguous) else pluralStringResource(R.plurals.model_count, list.size, list.size),
                                    checked = selected,
                                    submenu = true,
                                    icon = { _ -> BrandMark(h, colors, 16.dp) },
                                ) { modelHarness = h },
                            )
                        }
                    } else {
                        val list = open.value
                        add(MenuEntry(list.first().harnessLabel, back = true) { modelHarness = null })
                        val source = catalog.stale[open.key]
                        if (source != null) {
                            add(staleRow(listOf(source), loading || open.key in refreshing, catalog.errors[open.key]) { refresh(open.key) })
                        } else {
                            add(refreshRow(open.key in refreshing, refreshNotes[open.key]) { refresh(open.key) })
                        }
                        list.forEach { m ->
                            // The provider only where it tells variants apart (same
                            // name, or the same model under another provider).
                            add(MenuEntry(m.label, subtitle = m.providerLine(ambiguous), checked = m.harness == harness && m.id == modelId) {
                                harness = m.harness
                                modelId = m.id
                                effort = null
                                optionPicks = emptyMap()
                            })
                        }
                    }
                }
                "effort" -> stringResource(if (current?.efforts.isNullOrEmpty()) R.string.model_options else R.string.reasoning_effort) to buildList {
                    // The chip shows the middle level while nothing is picked
                    // (the engine default); check that same row so they agree.
                    val levels = current?.efforts.orEmpty()
                    val shown = effort ?: levels.getOrNull(levels.size / 2)
                    levels.forEach { e -> add(MenuEntry(reasoningLabel(e), checked = e == shown) { effort = e }) }
                    // Then what else the model offers (desktop TraitsPicker): Codex's
                    // service tier, Claude's 1M context window…
                    current?.options.orEmpty().filter { it.choices.size > 1 }.forEach { o ->
                        if (isNotEmpty()) add(menuSection(o.label)) else add(MenuEntry(o.label, header = true) {})
                        val picked = optionPicks[o.id] ?: o.defaultChoice
                        o.choices.forEach { c ->
                            add(MenuEntry(c.label, checked = c.id == picked) { optionPicks = optionPicks + (o.id to c.id) })
                        }
                    }
                }
                else -> null to emptyList()
            }
            // A drill-down swaps the whole list: give each level its own panel.
            androidx.compose.runtime.key(id, modelHarness, projectSortOpen) {
                AnchoredMenu(colors, anchor, title, entries, loading = id == "branch" && refs == null, onDismiss = close)
            }
        }
        if (scheduleOpen) {
            ScheduleSendDialog(colors, onDismiss = { scheduleOpen = false }) { atMs ->
                scheduleOpen = false
                val body = text.trim()
                if (body.isEmpty()) return@ScheduleSendDialog
                if (projectId == null && hostId == null) {
                    model.showToast(context.getString(R.string.choose_project_or_host))
                    return@ScheduleSendDialog
                }
                // The same picks an immediate send would use, frozen now.
                val label = project?.name ?: projectId?.let { addedNames[it] } ?: hosts.firstOrNull { it.id == hostId }?.name.orEmpty()
                val spec = sh.zeron.android.schedule.NewSessionSpec(
                    projectId = projectId,
                    hostId = if (projectId == null) hostId else null,
                    harness = harness,
                    model = modelId,
                    effort = effort,
                    branch = branch,
                    worktree = worktree && project != null,
                    projectPath = project?.path,
                    label = label,
                )
                sh.zeron.android.schedule.ScheduledAlarms.schedule(
                    context,
                    sh.zeron.android.schedule.ScheduledMessage(
                        workspace = model.activeMachine,
                        chatId = "",
                        text = body,
                        atMs = atMs,
                        chatTitle = context.getString(R.string.sched_new_session_title, label),
                        newSession = spec,
                    ),
                )
                text = ""
                model.showToast(context.getString(R.string.sched_new_toast, scheduleWhenText(context, atMs)))
                if (android.os.Build.VERSION.SDK_INT >= 33 &&
                    androidx.core.content.ContextCompat.checkSelfPermission(context, android.Manifest.permission.POST_NOTIFICATIONS) != android.content.pm.PackageManager.PERMISSION_GRANTED
                ) {
                    notificationPermission.launch(android.Manifest.permission.POST_NOTIFICATIONS)
                }
            }
        }
        if (browsing) {
            NewProjectScreen(model, initialDeviceId = project?.deviceId ?: hostId, onClose = { browsing = false }, onCreated = { id, name ->
                browsing = false
                addedNames = addedNames + (id to name)
                projectId = id
                hostId = null
                branch = null
                worktree = false
            })
        }
    }
}

/** "A↓Z" for the name sort row. */
@Composable
private fun SortAlphaMark(color: androidx.compose.ui.graphics.Color) {
    Text("A–Z", color = color, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 11.sp)
}

/** 「没能从电脑读取最新列表 · 重试」 ("Couldn't get the latest list · Retry"): the list shown isn't the computer's live one. */
@Composable
private fun staleRow(sources: Collection<CatalogSource>, reading: Boolean, error: String? = null, onRetry: () -> Unit): MenuEntry {
    val title = stringResource(if (reading) R.string.model_list_reading else R.string.model_list_stale)
    val showing = stringResource(if (CatalogSource.SAVED in sources) R.string.model_list_showing_saved else R.string.model_list_showing_builtin)
    // The real reason (timed out, ListHarnesses failed…), not just "couldn't read".
    val subtitle = if (error != null && !reading) "$showing\n${HostCatalog.reason(error)}" else showing
    return MenuEntry(title, subtitle = subtitle, keepOpen = true, icon = { c -> Glyph(Glyphs.Refresh, 17.dp, c) }) { onRetry() }
}

/** How long a refresh's outcome stays under the refresh row. */
private const val REFRESH_NOTE_MS = 6_000L

/**
 * 「刷新模型列表」 ("Refresh model list") at the top of a CLI's models when
 * the list is one the computer gave (live or saved): re-reads it from the
 * computer now. [note] is the last refresh's outcome (the real error on
 * failure), shown briefly.
 */
@Composable
private fun refreshRow(reading: Boolean, note: String?, onRefresh: () -> Unit): MenuEntry {
    val title = stringResource(if (reading) R.string.model_list_reading else R.string.model_list_refresh)
    val subtitle = if (reading) null else note ?: stringResource(R.string.model_list_refresh_hint)
    return MenuEntry(title, subtitle = subtitle, keepOpen = true, icon = { c -> Glyph(Glyphs.Refresh, 17.dp, c) }) { onRefresh() }
}

/** The options to start the session with: only ones the model offers, picked away from its default. */
internal fun optionsFor(model: ModelChoice?, picks: Map<String, String>): Map<String, String> {
    val offered = model?.options.orEmpty()
    return picks.filter { (id, choice) -> offered.any { o -> o.id == id && o.defaultChoice != choice && o.choices.any { it.id == choice } } }
}
