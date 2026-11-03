package sh.zeron.android.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.CloudStore
import sh.zeron.android.design.ZIcons

/**
 * Settings → Cloud (iOS `CloudSettingsViewController`): GitHub and the
 * repositories Cloud sessions can open, the agents' Cloud logins, and the
 * plan and credits that keep its machines running.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun CloudSettingsScreen(
    model: AppModel,
    onBack: () -> Unit,
    onOpenBilling: () -> Unit,
    onOpenAccounts: (AccountsTarget) -> Unit,
    onOpenRepoPicker: () -> Unit,
) {
    val cloud = model.cloud
    val context = LocalContext.current
    var connecting by remember { mutableStateOf(false) }
    var githubMenu by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) { cloud.refresh() }

    val installUrl = cloud.vault?.githubInstallUrl
    val subtitle = when {
        cloud.info == null -> null
        !cloud.available -> "Not available"
        else -> cloud.credits?.let { "${CloudStore.credits(it)} credits" }
    }

    CloudPage("Cloud", subtitle, onBack) {
        val info = cloud.info
        if (info == null) {
            if (cloud.error != null) {
                cloudSection("Status", key = "error") {
                    CloudRow(0, 1, "Couldn't reach Cloud", cloud.error, leading = { IconTile(ZIcons.Offline) }, onClick = { cloud.refresh() }, trailing = { Text("Retry", color = MaterialTheme.colorScheme.primary) })
                }
            } else {
                loadingItem()
            }
            return@CloudPage
        }
        if (!info.available) {
            cloudSection("Status", key = "unavailable") {
                CloudRow(
                    0, 1, "Cloud isn't available",
                    cloud.error ?: "This account can't use Cloud yet.",
                    leading = { IconTile(ZIcons.Cloud, MaterialTheme.colorScheme.surfaceContainerHighest, MaterialTheme.colorScheme.onSurfaceVariant) },
                )
            }
            return@CloudPage
        }
        cloud.problem?.let { problem -> item("problem") { ProblemCard(problem, onBilling = onOpenBilling) } }

        // GitHub: the account Cloud clones and pushes as.
        cloudSection("GitHub", footer = "Cloud agents clone and push to the repositories the Zeron GitHub App is installed on.") {
            Box {
                CloudRow(
                    0, 1,
                    cloud.githubAccount?.let { "@$it" } ?: "GitHub",
                    if (cloud.githubConnected) "Connected" else "Connect so Cloud agents can clone and push",
                    leading = { IconTile(ZIcons.GitHub, chipContainer(), MaterialTheme.colorScheme.onSurface) },
                    trailing = if (cloud.githubConnected) null else {
                        { Text("Connect", style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary) }
                    },
                    onClick = { if (cloud.githubConnected) githubMenu = true else connecting = true },
                )
                ActionMenu(
                    githubMenu,
                    { githubMenu = false },
                    listOfNotNull(
                        installUrl?.let { url -> MenuAction("Choose repositories on GitHub", ZIcons.Link) { model.browser.open(context, url) } },
                        MenuAction("Reconnect", ZIcons.Refresh) { connecting = true },
                    ),
                )
            }
        }

        // Repositories Cloud reaches (the GitHub App's installs).
        if (cloud.githubConnected) {
            val repos = cloud.repos.orEmpty().sortedByDescending { it.pushedAtMs }
            val shown = repos.take(8)
            val more = repos.size - shown.size
            val summary = when (val n = cloud.repos?.size) {
                null -> "Checking which repositories Cloud can open…"
                0 -> "None yet: choose the repositories Cloud sessions can open."
                1 -> "Cloud sessions can open 1 repository."
                else -> "Cloud sessions can open $n repositories."
            }
            cloudSection("Repositories", footer = "A Cloud project is a repository; each of its sessions runs on its own Cloud machine.") {
                val count = shown.size + (if (more > 0) 1 else 0) + 2
                var i = 0
                for (repo in shown) {
                    CloudRow(
                        i++, count, repo.fullName,
                        listOfNotNull(if (repo.private) "Private" else null, repo.description?.takeIf { it.isNotBlank() }).joinToString(" · ").ifEmpty { null },
                        leading = { IconTile(if (repo.private) ZIcons.Key else ZIcons.Folder, chipContainer(), MaterialTheme.colorScheme.onSurfaceVariant) },
                    )
                }
                if (more > 0) {
                    CloudRow(i++, count, "$more more", leading = { IconTile(ZIcons.More, chipContainer(), MaterialTheme.colorScheme.onSurfaceVariant) })
                }
                CloudRow(
                    i++, count, "New Cloud project", "Pick a repository to run sessions on Cloud alone",
                    leading = { IconTile(ZIcons.Plus, MaterialTheme.colorScheme.primaryContainer, MaterialTheme.colorScheme.onPrimaryContainer) },
                    titleColor = MaterialTheme.colorScheme.primary,
                    onClick = onOpenRepoPicker,
                )
                CloudRow(
                    i, count, "Choose on GitHub", summary,
                    leading = { IconTile(ZIcons.Link, chipContainer(), MaterialTheme.colorScheme.onSurface) },
                    enabled = installUrl != null,
                    onClick = installUrl?.let { url -> { model.browser.open(context, url) } },
                )
            }
        }

        // The agents Cloud runs, and the login each uses there.
        cloudSection("Agents", footer = "Cloud runs these agents on your own subscriptions or API keys.") {
            CloudStore.agents.forEachIndexed { i, agent ->
                val active = cloud.activeAccount(agent)
                val detail = when {
                    active != null -> listOfNotNull(
                        active.email ?: active.displayName ?: if (active.kind == "key") agent.keyName else null,
                        active.plan,
                    ).joinToString(" · ").ifEmpty { "Signed in" } + if (active.needsReconnect) " · Sign in again" else ""
                    cloud.hasAccount(agent) -> "No login selected"
                    else -> "Not signed in"
                }
                CloudRow(
                    i, CloudStore.agents.size, agent.name, detail,
                    leading = { AgentTile(agent.harness) },
                    trailing = if (active == null || active.needsReconnect) {
                        {
                            Text(
                                if (active?.needsReconnect == true) "Reconnect" else "Sign in",
                                style = MaterialTheme.typography.labelLarge,
                                color = MaterialTheme.colorScheme.primary,
                            )
                        }
                    } else {
                        { Chevron() }
                    },
                    onClick = { onOpenAccounts(AccountsTarget.Cloud(agent)) },
                )
            }
        }

        // Plan and credits (the machines run on them).
        cloudSection("Billing") {
            val billing = cloud.billing
            val plan = billing?.plan?.let { "${it.name} plan" }
            val credits = cloud.credits?.let { "${CloudStore.credits(it)} credits left" }
            CloudRow(
                0, 1,
                plan ?: "Plan & credits",
                listOfNotNull(credits, billing?.planStatus?.takeIf { it != "active" }?.replace('_', ' ')).joinToString(" · ").ifEmpty { "Plans, credits and invoices" },
                leading = { IconTile(ZIcons.Wallet) },
                trailing = if (billing == null && cloud.loading) {
                    { LoadingIndicator(Modifier.size(24.dp)) }
                } else {
                    null
                },
                onClick = onOpenBilling,
            )
        }
    }

    if (connecting) {
        GitHubConnectSheet(model, onDismiss = { connecting = false }, onConnected = { cloud.refresh() })
    }
}
