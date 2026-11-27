# Projects of repositories, and a device's repositories

A project doesn't need a folder that already holds its repositories. A project can be **composed** of repositories from anywhere on its device, each staying where it is. Zeron also lists, makes and clones the repositories on each of your computers.

```text
~/.zeron/projects/acme/           (the project's folder, made by Zeron)
  .zeron-project.json             names what it includes
  api  → ~/code/api               a repository, included (a link)
  web  → ~/work/web-app           included under another name
  AGENTS.md                       the project's own (anyone may add files)
```

Everything that reads a project of repositories from its folder works the same for one composed this way: [project worktrees](project-worktrees.md) (one worktree of the repositories chosen, on one branch), the Git panel's repository chip, `zeron.repositories(folder)` for plugins, and iOS and Android's checkout sheet.

## Making one

**Desktop:** New project → a computer → **Repositories…** opens New project:

- **Name.** Left empty, it follows what's chosen: their GitHub owner when they share one (`acme`), else the first one's name. The folder is `~/.zeron/projects/<name>`, or `<name> 2`… when that's taken.
- **Repositories.** The computer's repositories, newest worked-in first, each with its path (`~/code/api`), its GitHub repository and its branch. A check marks each one chosen. The search matches by name first, then by path or GitHub repository. Enter on a search with one match chooses it. A path typed (`~/vendor/sdk`) offers **Use this repository**: it's added to the computer's list and chosen.
- **New** and **Clone** turn the dialog to their form (below). The repository made there comes back chosen.
- **Create project with N repositories** makes the folder on the computer (`ComposeProject`), adds the project (`createSpace`, named) and opens a new chat in it.

New project → a computer also offers **New repository…** and **Clone repository…** on their own. The repository made becomes a project, and a chat opens in it.

**iOS and Android:** New Project's **Folder | Repositories** switch. Repositories works the same way: a name, a search, a check on each repository chosen, **New Repository…**, and **Create Project · N Repositories**.

## Changing one

**Edit project → Repositories** (desktop) lists what the project holds:

- **Included from elsewhere:** its path, its branch, and × to take it out. One no longer at its path reads *Not found at ~/code/docs* (moved or deleted). It stays listed, never checked out, until it's taken out or included again from its new place.
- **In the project's folder** (a folder of your own that holds repositories): listed as they are. They're the folder's, so they can't be taken out here.
- **Add** opens the same picker as New project, without the repositories the project already holds, and offers New and Clone. Those added read *Added on save*.

Nothing changes until **Save**, which saves the name, the icon and the repositories together (`SetProjectRepositories`). Taking a repository out removes its link, never the repository.

A project that is **one repository** says so and offers **Combine with other repositories…**: New project from repositories, with that one chosen. The project itself stays as it is. A **Cloud project** is its GitHub repository and says so. A host **too old** to do this asks you to update Zeron on it. An **offline** one says it's offline.

## New repository, Clone

- **New repository:** a name (letters, digits, `.`, `_`, `-`; anything else becomes `-`), a **Location** (where most of the computer's repositories are by default, else a usual code folder such as `~/Developer` or `~/code`, else home), and **Start with a README** (its first commit, on `main`: worktrees branch from it). **Also create it on GitHub** appears only where the computer's GitHub CLI is signed in, with Private or Public, and makes it the repository's `origin`. If git on the computer has no name and email yet, the repository is made without a first commit and Zeron says why. If GitHub refuses, it's made on the computer and Zeron says GitHub refused.
- **Clone:** an HTTPS or SSH address and a location (the same default). The hint shows where it will land.

## Settings → Repositories

Each computer's repositories, one computer at a time (this one first), newest worked-in first. Each row shows its path, branch and GitHub repository, the projects it's in, and when it was last worked in. Its ⋯ menu offers:

- **New chat in it** (its project, else it added as one)
- **Show in folder** (this computer's)
- **Copy path**
- **Add to project** (each project of repositories on that computer)
- **Forget**: no longer listed, and nothing on disk changes. Adding it again (a path typed, a clone) lists it again.

**New repository**, **Clone** and **Look again** (a fresh look) stand at the top.

## Where repositories are found

The host looks through home, up to four folders deep (`~/src/github.com/owner/repo`). It never looks in a dot folder, in a repository (its nested ones are its own), in the places dependencies and builds pile up (`node_modules`, `target`, `vendor`, `venv`, `Pods`, `DerivedData`, Go's `go/pkg`…), or in the OS's own folders (`Library`, `Applications`, `Pictures`…). It also skips Zeron's own folders (worktrees, projects, data). A look takes at most 6 seconds, 25,000 folders or 400 repositories, and is reused for a minute. Repositories added, cloned or made through Zeron are kept in the list wherever they are (`repos.json`). Each one's branch, GitHub repository and last change are read from its `.git` without running git, so a long list comes back quickly.

## On the host

- `ComposeProject {name, repositories: [{path, name?}], parent?}` → `{path, repositories}`. Each repository must be a repository's own top folder: not a folder inside one, a linked worktree or a submodule. It must sit outside the project's folder and must not hold it. Each one is included once. Any mistake is refused before a folder is made.
- `SetProjectRepositories {path, repositories}` → the project's listing. The project includes exactly those. A name is the repository's folder name, or the one asked, made unique against the folder's own entries (`api`, `api-2`). A repository kept keeps its name. Only links named in `.zeron-project.json` are ever removed. The file is written whole (a temporary file renamed over it), and removed when nothing's included. A folder that is itself a repository is refused: such a project needs a folder of its own (Combine).
- `ListLocalRepositories {refresh?}`, `CreateLocalRepository {name, parent?, empty?, publish?: {private}}`, `ForgetLocalRepository {path}`.
- All of them are relay-forwardable, so any of your devices can ask the computer that owns the project.
- On Windows, a link needs the right to create symbolic links. Without it, including a repository fails and says so.

## Cloud

A Cloud session runs on a machine of its own, which clones its project's GitHub repository. A Cloud project is one repository, so its Edit project says so and offers nothing to change. See [project worktrees](project-worktrees.md#cloud) for how Cloud handles a project of several repositories.

## Compatibility

- `capabilities::COMPOSED_PROJECTS_V1` (`composed-projects-v1`). A device that doesn't say it gets none of these flows: New project offers only folders on it, Edit project's Repositories asks you to update Zeron there, and Settings says the same.
- **Wire and storage:** new methods only, plus additive, defaulted fields on `ProjectRepository` (`location`, `missing`, `github`). Nothing changes in the workspace registry, the session documents or SQLite. A project composed this way is an ordinary project whose folder Zeron made. A host too old to read `.zeron-project.json` sees links it skips (as it always has), so it lists none of those repositories, and nothing else breaks.
