package sh.zeron.android.ui

import android.view.HapticFeedbackConstants
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.CloudStore
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons

/**
 * New Cloud Project (iOS `CloudRepoPickerViewController`): the GitHub
 * repositories Cloud reaches. Picking one makes it a project that lives on
 * Cloud alone — every session in it runs on its own Cloud machine, cloning
 * the repository.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun CloudRepoPickerScreen(model: AppModel, onBack: () -> Unit, onCreated: (projectId: String) -> Unit) {
    val cloud = model.cloud
    val context = LocalContext.current
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    var query by remember { mutableStateOf("") }
    var creating by remember { mutableStateOf<String?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }
    var connecting by remember { mutableStateOf(false) }
    // The repository read is in flight (`cloud.repos` stays null on failure).
    var loading by remember { mutableStateOf(true) }
    var attempt by remember { mutableIntStateOf(0) }
    LaunchedEffect(attempt) {
        loading = true
        cloud.refreshGitHub()
        loading = false
    }

    val all = cloud.repos.orEmpty().sortedByDescending { it.pushedAtMs }
    val q = query.trim().lowercase()
    val repos = if (q.isEmpty()) all else all.filter { it.fullName.lowercase().contains(q) || it.description?.lowercase()?.contains(q) == true }

    fun create(repo: String) {
        if (creating != null) return
        creating = repo
        scope.launch {
            try {
                val id = cloud.createProject(repo)
                view.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
                onCreated(id)
            } catch (e: Exception) {
                failure = CloudStore.message(e)
            } finally {
                creating = null
            }
        }
    }

    CloudPage("New Cloud project", "Pick a repository: its sessions run on Cloud machines", onBack) {
        if (all.isNotEmpty()) {
            item("search") {
                OutlinedTextField(
                    query,
                    { query = it },
                    placeholder = { Text("Search repositories") },
                    leadingIcon = { ZIcon(ZIcons.Search, null, Modifier.size(20.dp)) },
                    trailingIcon = if (query.isNotEmpty()) {
                        { TextButton(onClick = { query = "" }) { Text("Clear") } }
                    } else {
                        null
                    },
                    singleLine = true,
                    shape = RoundedCornerShape(28.dp),
                    colors = OutlinedTextFieldDefaults.colors(
                        unfocusedContainerColor = cardColor(),
                        focusedContainerColor = cardColor(),
                        unfocusedBorderColor = MaterialTheme.colorScheme.outlineVariant,
                    ),
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
                )
            }
        }
        when {
            repos.isNotEmpty() -> cloudSection(
                if (q.isEmpty()) "Repositories" else "Matching \"${query.trim()}\"",
                footer = "Each session in a Cloud project runs on its own Cloud machine.",
                key = "repos",
            ) {
                repos.forEachIndexed { i, repo ->
                    val busy = creating == repo.fullName
                    CloudRow(
                        i, repos.size, repo.fullName,
                        listOfNotNull(if (repo.private) "Private" else null, repo.description?.takeIf { it.isNotBlank() }).joinToString(" · ").ifEmpty { null },
                        leading = { IconTile(ZIcons.GitHub, chipContainer(), MaterialTheme.colorScheme.onSurface) },
                        trailing = {
                            if (busy) {
                                LoadingIndicator(Modifier.size(28.dp))
                            } else if (repo.pushedAtMs > 946_684_800_000L) { // a real push time (the demo sends only sort keys)
                                Text(CloudStore.shortDate(repo.pushedAtMs), style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.outline)
                            }
                        },
                        enabled = creating == null,
                        onClick = { create(repo.fullName) },
                    )
                }
            }
            all.isNotEmpty() -> item("no-match") {
                Text(
                    "No repositories match \"${query.trim()}\".",
                    style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = 28.dp, vertical = 24.dp),
                )
            }
            // Still reading, or the read failed: say which, never a blank page.
            cloud.repos == null && loading -> loadingItem()
            cloud.repos == null -> item("failed") {
                EmptyRepos(
                    "Couldn't load repositories",
                    "Check your connection and try again.",
                    icon = ZIcons.Offline,
                ) { SecondaryCapsule("Try again", onClick = { attempt++ }, icon = ZIcons.Refresh) }
            }
            else -> item("empty") {
                val connected = cloud.githubConnected
                EmptyRepos(
                    if (connected) "No repositories" else "Connect GitHub",
                    if (connected) "Cloud reaches the repositories the Zeron GitHub App is installed on." else "Cloud projects are GitHub repositories. Connect GitHub to pick one.",
                    icon = ZIcons.GitHub,
                ) {
                    if (connected) {
                        cloud.vault?.githubInstallUrl?.let { url ->
                            SecondaryCapsule("Choose on GitHub", onClick = { model.browser.open(context, url) }, icon = ZIcons.Link)
                        }
                    } else {
                        PrimaryCapsule("Connect GitHub", onClick = { connecting = true }, icon = ZIcons.GitHub)
                    }
                }
            }
        }
    }

    if (connecting) {
        GitHubConnectSheet(model, onDismiss = { connecting = false }, onConnected = { attempt++ })
    }
    failure?.let { message ->
        AlertDialog(
            onDismissRequest = { failure = null },
            icon = { ZIcon(ZIcons.Warning, null) },
            title = { Text("Couldn't add the project") },
            text = { Text(message) },
            confirmButton = { TextButton(onClick = { failure = null }) { Text("OK") } },
        )
    }
}

/** A centred empty state: a tile, a title over a muted line, and its one way forward. */
@Composable
private fun EmptyRepos(title: String, body: String, icon: Int, action: @Composable () -> Unit) {
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 32.dp, vertical = 40.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(20.dp),
    ) {
        Box(Modifier.size(64.dp).clip(CircleShape).background(chipContainer()), contentAlignment = Alignment.Center) {
            ZIcon(icon, null, Modifier.size(30.dp), tint = MaterialTheme.colorScheme.onSurface)
        }
        PageIntro(title, body, centered = true)
        action()
    }
}
