package sh.zeron.android.ui

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
import uniffi.zeron_core.CoreClient
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
private data class ModelChoice(
    val harness: String,
    val harnessLabel: String,
    val id: String,
    val label: String,
    val efforts: List<String>,
)

/** The last catalog each host reported: the chip opens on a real model name at once. */
private val modelCache = HashMap<String, List<ModelChoice>>()

private fun catalogModels(): List<ModelChoice> =
    uniffi.zeron_core.fallbackHarnesses().filter { it.offered }.flatMap { h ->
        uniffi.zeron_core.fallbackModels(h.id).map { ModelChoice(h.id, h.label, it.id, it.label, it.reasoningLevels) }
    }

/** Every offered harness on the host and its models (ListHarnesses + ListModels). */
private suspend fun hostModels(client: CoreClient, device: String): List<ModelChoice> = coroutineScope {
    client.listHarnesses(device).filter { it.offered }.map { h ->
        async {
            val models = client.listModels(device, h.id).ifEmpty { uniffi.zeron_core.fallbackModels(h.id) }
            models.map { ModelChoice(h.id, h.label, it.id, it.label, it.reasoningLevels) }
        }
    }.awaitAll().flatten()
}

/**
 * New Session, like iOS NewSessionViewController: the composer's chips pick
 * the project (projects grouped by computer, No Project, New Project…), the
 * host when there's no project, the branch or a new worktree for git
 * projects, the model (grouped by harness) and the reasoning effort.
 */
@Composable
fun NewSessionSheet(model: ZeronModel, onDismiss: () -> Unit) {
    val colors = LocalZeronColors.current
    val client = model.client ?: return
    val projects = model.workspace?.projects.orEmpty()
    val hosts = remember(model.workspace) { client.executionDevices() }
    var text by remember { mutableStateOf("") }
    var projectId by remember { mutableStateOf(model.newSessionProject ?: projects.firstOrNull()?.id) }
    var hostId by remember { mutableStateOf(if (projectId == null) (hosts.firstOrNull { it.online } ?: hosts.firstOrNull())?.id else null) }
    var harness by remember { mutableStateOf("claude-code") }
    var modelId by remember { mutableStateOf<String?>(null) }
    var effort by remember { mutableStateOf<String?>(null) }
    var branch by remember { mutableStateOf<String?>(null) }
    var worktree by remember { mutableStateOf(false) }
    var browsing by remember { mutableStateOf(false) }
    var addedNames by remember { mutableStateOf<Map<String, String>>(emptyMap()) }
    var chipMenu by remember { mutableStateOf<Pair<String, Rect>?>(null) }
    var refs by remember { mutableStateOf<List<RepoRef>?>(null) }
    LaunchedEffect(Unit) { model.newSessionProject = null }

    val project = projects.firstOrNull { it.id == projectId }
    val device = project?.deviceId ?: hostId ?: hosts.firstOrNull()?.id ?: ""
    var models by remember { mutableStateOf(modelCache[device] ?: catalogModels()) }
    LaunchedEffect(device) {
        models = modelCache[device] ?: catalogModels()
        if (device.isEmpty()) return@LaunchedEffect
        val fresh = runCatching { hostModels(client, device) }.getOrDefault(emptyList())
        if (fresh.isNotEmpty()) {
            modelCache[device] = fresh
            models = fresh
        }
    }
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
            val name = project?.name ?: addedNames[projectId] ?: "Project"
            add(Chip("project", name, colorIndex = project?.colorIndex?.toInt() ?: 0))
            if (project?.gitDetected == true) add(Chip("branch", if (worktree) "New worktree" else branch ?: "Current branch"))
        } else {
            add(Chip("project", "No project"))
            add(Chip("host", hosts.firstOrNull { it.id == hostId }?.name ?: "Choose host"))
        }
        // Never the harness name in place of a model.
        val modelTitle = current?.label ?: modelId?.let { modelLabel(harness, it) } ?: harnessLabel(harness)
        add(Chip("model", modelTitle, harness = harness))
        val efforts = current?.efforts.orEmpty()
        if (efforts.isNotEmpty()) add(Chip("effort", reasoningLabel(effort ?: efforts[efforts.size / 2])))
    }

    Box(Modifier.fillMaxSize()) {
        Column(
            Modifier
                .fillMaxSize()
                .background(colors.background)
                .statusBarsPadding()
                .windowInsetsPadding(WindowInsets.ime.union(WindowInsets.navigationBars))
                .padding(horizontal = 12.dp),
        ) {
            Row(Modifier.fillMaxWidth().padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("Close", color = colors.text, fontFamily = ZeronType.Sans, fontSize = 16.sp, modifier = Modifier.clip(RoundedCornerShape(10.dp)).clickable(onClick = onDismiss).padding(10.dp))
                Text("New Session", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp, modifier = Modifier.weight(1f), textAlign = androidx.compose.ui.text.style.TextAlign.Center)
                Spacer(Modifier.width(60.dp))
            }
            Column(Modifier.weight(1f).fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center) {
                BrandMark(harness, colors, 34.dp)
                Spacer(Modifier.height(14.dp))
                Text("What are we building?", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 22.sp)
            }
            Box(Modifier.fillMaxWidth().padding(bottom = 8.dp), contentAlignment = Alignment.Center) {
                Box(Modifier.widthIn(max = 768.dp).fillMaxWidth()) {
                    ComposerBar(
                        colors = colors,
                        text = text,
                        onText = { text = it },
                        placeholder = "Describe the task",
                        running = false,
                        canSteer = false,
                        focused = true,
                        onFocus = {},
                        chips = chips,
                        onChip = { chip, rect -> chipMenu = chip.id to rect },
                        images = emptyList(),
                        onRemoveImage = {},
                        onSend = send@{
                            val body = text.trim()
                            if (body.isEmpty()) return@send
                            val target = when {
                                projectId != null -> SessionTarget.Project(projectId!!)
                                hostId != null -> SessionTarget.Projectless(hostId!!)
                                else -> {
                                    model.showToast("Choose a project or a host that can run it.")
                                    return@send
                                }
                            }
                            try {
                                val id = client.createSession(NewSession(target, model.defaultConfig(harness, modelId, effort), if (worktree) null else branch, null, null))
                                val handle = client.openSession(id)
                                val spec = if (worktree && project != null) WorktreeSpec(project.path, branch ?: "HEAD", project.id) else null
                                handle.send(SendRequest(body, emptyList(), spec, BusyPolicy.QUEUE))
                                handle.close()
                                onDismiss()
                                model.openSession(id)
                            } catch (t: Throwable) {
                                model.showToast(t.message ?: "Couldn't start the session")
                            }
                        },
                        mentionSearch = search@{ q ->
                            val p = project ?: return@search emptyList()
                            runCatching { client.searchFiles(p.deviceId, null, p.id, q) }.getOrDefault(emptyList())
                        },
                        onMention = { path, dir -> text = text.replace(Regex("@[^\\s]*$"), fileMentionLink(path, dir) + " ") },
                    )
                }
            }
        }
        chipMenu?.let { (id, anchor) ->
            val close = { chipMenu = null }
            val (title, entries) = when (id) {
                "project" -> "Project" to buildList {
                    // Projects grouped by computer, like the iOS inline sections.
                    projects.groupBy { it.deviceId }.entries
                        .sortedBy { (_, list) -> list.first().deviceName ?: "" }
                        .forEach { (_, list) ->
                            val head = list.first()
                            add(menuSection((head.deviceName ?: "Host") + if (head.deviceOnline) "" else " · offline"))
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
                    add(MenuEntry("No Project…", checked = projectId == null, icon = { c -> Glyph(Glyphs.Tray, 17.dp, c) }) {
                        projectId = null
                        hostId = hostId ?: (hosts.firstOrNull { it.online } ?: hosts.firstOrNull())?.id
                    })
                    add(MenuEntry("New Project…", icon = { c -> Glyph(Glyphs.FolderPlus, 17.dp, c) }) { browsing = true })
                }
                "host" -> "Run on" to hosts.map { h ->
                    MenuEntry(h.name, subtitle = if (h.online) "Online" else "Offline", checked = h.id == hostId, icon = { c -> Glyph(Glyphs.Computer, 17.dp, c) }) { hostId = h.id }
                }
                "branch" -> "Checkout" to buildList {
                    add(MenuEntry("New worktree", checked = worktree) { worktree = !worktree })
                    add(menuSection("Branch"))
                    val list = refs.orEmpty()
                    list.forEach { ref ->
                        add(MenuEntry(ref.name, subtitle = if (ref.current) "Checked out" else null, checked = ref.name == (branch ?: list.firstOrNull()?.name)) { branch = ref.name })
                    }
                    if (refs != null && list.isEmpty()) add(MenuEntry("No branches found") {})
                }
                "model" -> "Model" to buildList {
                    models.groupBy { it.harness }.entries.sortedBy { it.key }.forEach { (h, list) ->
                        add(menuSection(list.first().harnessLabel))
                        list.forEach { m ->
                            add(MenuEntry(m.label, checked = m.harness == harness && m.id == modelId, icon = { _ -> BrandMark(h, colors, 16.dp) }) {
                                harness = m.harness
                                modelId = m.id
                                effort = null
                            })
                        }
                    }
                }
                "effort" -> "Reasoning effort" to current?.efforts.orEmpty().let { levels ->
                    // The chip shows the middle level while nothing is picked
                    // (the engine default); check that same row so they agree.
                    val shown = effort ?: levels.getOrNull(levels.size / 2)
                    levels.map { e -> MenuEntry(reasoningLabel(e), checked = e == shown) { effort = e } }
                }
                else -> null to emptyList()
            }
            AnchoredMenu(colors, anchor, title, entries, loading = id == "branch" && refs == null, onDismiss = close)
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
