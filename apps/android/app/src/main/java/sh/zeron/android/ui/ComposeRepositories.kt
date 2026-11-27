package sh.zeron.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Checkbox
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.CoreClient
import uniffi.zeron_core.DeviceView
import uniffi.zeron_core.LocalRepositories
import uniffi.zeron_core.LocalRepository

/**
 * New project's Repositories: a project of repositories from anywhere on one
 * computer, each staying where it is (docs/reference/composed-projects.md).
 * The computer's repositories, newest worked-in first, a check on each
 * chosen; a search over them; New repository makes one there and chooses it.
 * The name, left empty, follows what's chosen (their GitHub owner when they
 * share one, else the first one's name).
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun ComposeRepositories(client: CoreClient?, device: DeviceView, onCreated: (String) -> Unit) {
    val scope = rememberCoroutineScope()
    var listing by remember(device.id) { mutableStateOf<LocalRepositories?>(null) }
    var failure by remember(device.id) { mutableStateOf<String?>(null) }
    val chosen = remember(device.id) { mutableStateListOf<String>() }
    var name by remember(device.id) { mutableStateOf("") }
    var query by remember(device.id) { mutableStateOf("") }
    var creating by remember { mutableStateOf(false) }
    var status by remember { mutableStateOf<String?>(null) }
    var newRepository by remember { mutableStateOf(false) }

    LaunchedEffect(device.id, client) {
        val c = client ?: return@LaunchedEffect
        runCatching { c.listLocalRepositories(device.id, false) }
            .onSuccess { listing = it; failure = null }
            .onFailure { failure = if (device.online) says(it) else "${device.name} is offline." }
    }

    val all = listing?.repositories.orEmpty()
    val q = query.trim().lowercase()
    val shown = if (q.isEmpty()) all else {
        val byName = all.filter { it.name.lowercase().startsWith(q) }
        val inName = all.filter { !it.name.lowercase().startsWith(q) && it.name.lowercase().contains(q) }
        val elsewhere = all.filter { !it.name.lowercase().contains(q) && (it.path.lowercase().contains(q) || it.github?.lowercase()?.contains(q) == true) }
        byName + inName + elsewhere
    }
    val picked = chosen.mapNotNull { p -> all.firstOrNull { it.path == p } }
    val owners = picked.map { it.github?.substringBefore('/')?.lowercase().orEmpty() }.toSet()
    val suggested = when {
        picked.isEmpty() -> ""
        picked.size > 1 && owners.size == 1 && owners.first().isNotEmpty() -> picked.first().github!!.substringBefore('/')
        else -> picked.first().name
    }
    val projectName = name.trim().ifEmpty { suggested }
    fun tilde(path: String): String {
        val home = listing?.home.orEmpty()
        return if (home.isNotEmpty() && path.startsWith("$home/")) "~" + path.removePrefix(home) else path
    }

    Box(Modifier.fillMaxSize()) {
        LazyColumn(contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 160.dp)) {
            item("name") {
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it },
                    label = { Text("Name") },
                    placeholder = { Text(suggested.ifEmpty { "Project name" }) },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false),
                    modifier = Modifier.fillMaxWidth().padding(bottom = 12.dp).testTag("compose-name"),
                )
            }
            item("search") {
                OutlinedTextField(
                    value = query,
                    onValueChange = { query = it },
                    placeholder = { Text("Search repositories on ${device.name}") },
                    leadingIcon = { ZIcon(ZIcons.Search, null, Modifier.size(18.dp)) },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(autoCorrectEnabled = false),
                    modifier = Modifier.fillMaxWidth().padding(bottom = 12.dp).testTag("compose-search"),
                )
            }
            val count = shown.size + if (listing != null) 1 else 0
            if (listing != null) {
                item("new") {
                    SegmentedListItem(
                        onClick = { newRepository = true },
                        shapes = segmentedShapes(0, count),
                        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                        leadingContent = { ZIcon(ZIcons.Plus, null, Modifier.size(20.dp), tint = MaterialTheme.colorScheme.primary) },
                        modifier = Modifier.testTag("compose-new-repository"),
                    ) { Text("New repository…", style = MaterialTheme.typography.titleMedium, color = MaterialTheme.colorScheme.primary) }
                }
            }
            if (shown.isEmpty()) {
                item("status") {
                    Text(
                        failure ?: when {
                            listing == null -> "Looking for repositories…"
                            q.isEmpty() -> "No repositories found on ${device.name}."
                            else -> "Nothing matches “$query”."
                        },
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        textAlign = TextAlign.Center,
                        modifier = Modifier.fillMaxWidth().padding(vertical = 24.dp).testTag("compose-status"),
                    )
                }
            }
            itemsIndexed(shown, key = { _, r -> r.path }) { i, r ->
                val on = r.path in chosen
                fun flip() = if (on) chosen.remove(r.path) else chosen.add(r.path)
                SegmentedListItem(
                    onClick = { flip() },
                    shapes = segmentedShapes(i + if (listing != null) 1 else 0, count),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { ZIcon(ZIcons.Branch, null, Modifier.size(20.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant) },
                    trailingContent = { Checkbox(checked = on, onCheckedChange = { flip() }) },
                    supportingContent = {
                        Text(
                            detail(r, tilde(r.path)),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    },
                    modifier = Modifier.padding(bottom = ListItemDefaults.SegmentedGap).testTag("compose-repository-${r.name}"),
                ) { Text(r.name, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis) }
            }
        }
        Column(
            Modifier.align(Alignment.BottomCenter).fillMaxWidth().navigationBarsPadding().imePadding().padding(horizontal = 20.dp, vertical = 12.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            status?.let {
                Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error, textAlign = TextAlign.Center)
            }
            Button(
                onClick = {
                    val c = client ?: return@Button
                    creating = true
                    status = null
                    scope.launch {
                        runCatching { c.composeProject(device.id, projectName, chosen.toList()) }
                            .onSuccess { onCreated(it) }
                            .onFailure { status = says(it) }
                        creating = false
                    }
                },
                enabled = !creating && chosen.isNotEmpty() && projectName.isNotEmpty(),
                shapes = ButtonDefaults.shapes(),
                contentPadding = PaddingValues(horizontal = 24.dp, vertical = 16.dp),
                modifier = Modifier.fillMaxWidth().testTag("compose-create"),
            ) {
                val n = chosen.size
                if (creating) {
                    LoadingIndicator(Modifier.size(20.dp))
                } else {
                    Text(if (n == 0) "Choose repositories" else "Create project · $n ${if (n == 1) "repository" else "repositories"}")
                }
            }
        }
    }

    if (newRepository) {
        NewRepositoryDialog(client, device, listing?.canPublish == true, query.trim(), onDismiss = { newRepository = false }) { made ->
            listing = listing?.let { l -> l.copy(repositories = listOf(made) + l.repositories.filter { it.path != made.path }) }
            chosen.add(made.path)
            query = ""
            newRepository = false
        }
    }
}

/** `~/code/api · main · acme/api` */
private fun detail(r: LocalRepository, where: String): String =
    listOfNotNull(where, if (r.hasCommits) (r.branch ?: "Detached") else "No commits yet", r.github).joinToString(" · ")

/** A host's refusal, in a sentence. */
internal fun says(error: Throwable): String {
    val text = error.message.orEmpty()
    return when {
        text.contains("unknown method", ignoreCase = true) -> "Update Zeron on that computer to do this."
        text.isBlank() -> "Something went wrong."
        else -> text.substringAfter("message=").removeSuffix(")").ifBlank { text }
    }
}

/** A new repository on a computer: its name, a README as its first commit,
 *  and (where its GitHub CLI is signed in) a copy on GitHub. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun NewRepositoryDialog(
    client: CoreClient?,
    device: DeviceView,
    canPublish: Boolean,
    initial: String,
    onDismiss: () -> Unit,
    onCreated: (LocalRepository) -> Unit,
) {
    val scope = rememberCoroutineScope()
    var name by remember { mutableStateOf(initial) }
    var readme by remember { mutableStateOf(true) }
    var publish by remember { mutableStateOf(false) }
    var private by remember { mutableStateOf(true) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val clean = name.trim().map { if (it.isLetterOrDigit() && it.code < 128 || it == '.' || it == '_' || it == '-') it else '-' }
        .joinToString("").trim('-', '.')
    AlertDialog(
        onDismissRequest = { if (!busy) onDismiss() },
        title = { Text("New repository") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it; error = null },
                    label = { Text("Name") },
                    placeholder = { Text("my-repository") },
                    singleLine = true,
                    isError = error != null,
                    supportingText = { Text(error ?: if (clean.isEmpty()) "Made where most of ${device.name}'s repositories are." else "Made as “$clean” on ${device.name}.") },
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false),
                    modifier = Modifier.fillMaxWidth().testTag("new-repository-name"),
                )
                SwitchRow("Start with a README", "Its first commit, on main: worktrees branch from it.", readme) { readme = it }
                if (canPublish) {
                    SwitchRow("Also create it on GitHub", "With the GitHub CLI signed in on ${device.name}.", publish) { publish = it }
                    if (publish) {
                        SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                            listOf(true to "Private", false to "Public").forEachIndexed { i, (value, label) ->
                                SegmentedButton(selected = private == value, onClick = { private = value }, shape = SegmentedButtonDefaults.itemShape(i, 2), icon = {}) { Text(label) }
                            }
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(
                onClick = {
                    val c = client ?: return@TextButton
                    busy = true
                    scope.launch {
                        runCatching { c.createLocalRepository(device.id, clean, null, !readme, if (canPublish && publish) private else null) }
                            .onSuccess { onCreated(it.repository) }
                            .onFailure { error = says(it) }
                        busy = false
                    }
                },
                enabled = clean.isNotEmpty() && !busy,
                modifier = Modifier.testTag("new-repository-create"),
            ) { Text(if (busy) "Creating…" else "Create") }
        },
        dismissButton = { TextButton(onClick = onDismiss, enabled = !busy) { Text("Cancel") } },
    )
}

@Composable
private fun SwitchRow(title: String, detail: String, on: Boolean, onChange: (Boolean) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.titleSmall)
            Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Switch(checked = on, onCheckedChange = onChange)
    }
}
