# Project worktrees

A project's folder doesn't have to be one git repository. A folder that holds several, and isn't one itself, can be checked out whole: a new chat in a **worktree** of such a project gets one folder laid out like the project, with a worktree of each repository chosen in it, all on one branch.

```text
~/work/acme/                      ~/.zeron/worktrees/acme/swift-otter/
  AGENTS.md                         AGENTS.md        → link to ~/work/acme/AGENTS.md
  api/        (repository)          api/             worktree, zeron/swift-otter
  web/        (repository)          web/             worktree, zeron/swift-otter
  docs/       (repository)                           (not chosen: left out)
  packages/   (plain folder)        packages/        a real folder
    ui/       (repository)            ui/            worktree, zeron/swift-otter
    README.md                         README.md      → link
```

## Choosing repositories

In the composer's **Work in** panel, a project folder holding repositories offers **Checkout → Worktree** as a repository does. With a worktree picked, the panel lists the repositories in place of branches: each one's path in the project and its current branch, a check on each chosen. **All** / **None** sit beside the count.

- **At first**, every repository is chosen when there are 8 or fewer, and none when there are more (a folder of dozens of clones isn't checked out whole by accident).
- **Remembered** per project on this device: the next new chat in that project starts from the last choice.
- **Not choosable:** a repository with no commit yet (there is nothing to branch from). It's listed, dimmed, with "No commits yet".
- **None chosen:** the chat can't start, and the composer says so.

The chip under the composer reads *device › project › N repositories* behind the worktree glyph. A **Local checkout** of the same project runs in its folder, as any plain folder does.

## Where the repositories are found

The project folder's own subfolders, and one level further inside a plain subfolder (`packages/ui` above). Never dot folders, `node_modules`, linked worktrees or submodules (each belongs to another repository), nor a symbolic link (it may lead out of the project). At most 64 are listed.

The host device looks, when asked (`ListProjectRepositories`): nothing is stored in the workspace registry, so a repository cloned into the folder shows up in the next new chat.

## What the host makes

When the chat's first run is drained on the host (`WorktreeSpec::repositories`):

1. **A folder is claimed** under the worktree location (Settings → General → Worktrees; `~/.zeron/worktrees` by default): `<location>/<project folder name>/<name>`, with a generated name that no chosen repository has a `zeron/<name>` branch for. The folder is claimed by creating it, so two chats starting at once never share one.
2. **The project's own entries are linked** into it: files and plain folders, so the agent reads the project as it is (`AGENTS.md`, shared scripts). A folder that holds a repository is created instead, and its other entries linked. Repositories not chosen are left out, never linked: the agent would change the originals. Editing a linked file changes the original for every chat. On Windows, a link that can't be created (no permission to create symbolic links) is skipped.
3. **Each chosen repository gets a worktree** at its own path, on the shared `zeron/<name>` branch, off its current commit. A run whose base ref names a branch a repository has starts that repository from it instead.
4. **All or nothing:** if any repository can't be checked out (a hook refuses, a lock), every worktree already made is removed along with its branch, then the folder, and the run fails naming the repository. The project's own folder and the originals behind the links are never touched.

The chat runs in the new folder, and its row records it (`cwd`) and the shared branch. The project's **setup Action** (*Run automatically on worktree creation*) runs once in the folder, with `ZERON_PROJECT_ROOT` set to the project and `ZERON_WORKTREE_PATH` set to the folder. A retried first run reuses the chat's worktree.

Once the chat has a title, every repository still on `zeron/<name>` is renamed to one branch named after it, the same name in each. A name already taken in any of them gets the folder's suffix in all of them. A repository whose branch was switched in the meantime is left alone. If one rename fails, those already renamed are put back.

`DeleteWorktree` on the folder removes each repository's worktree (deleting its `zeron/…` branch) and then the folder. Only the links are removed, never their originals. A folder holding no worktree is refused. As with single-repository worktrees, archiving or deleting a chat doesn't remove its worktree.

## The Git panel

A chat whose folder holds several repositories (a project worktree, or a local checkout of such a project) shows one of them in the panel's Git tabs, with a **repository chip** beside the branch chip. It starts on the repository last shown for that chat, otherwise the first one with changes, otherwise the first. Changes, History, staging, commit, push and branch switching all act on the repository shown. Choosing another in the chip switches the tab to it. The chip's list shows each repository's branch and how many files changed there. **Last turn** is recorded for each repository separately as a turn starts.

Plugins can find the same repositories with `zeron.repositories(folder)` in their service (`sessions.read`, and only within this device's chats' and projects' folders).

## Compatibility

- **Older hosts** (a device on a version before project worktrees) refuse `ListProjectRepositories`. The composer then offers no worktree for such a folder and runs in it, as before. A worktree spec naming repositories is only ever sent after the host has listed them (`capabilities::PROJECT_WORKTREES_V1`).
- **Wire and storage:** `WorktreeSpec::repositories` is additive, serde-defaulted and omitted when empty, so single-repository worktrees are sent exactly as before. No registry, session-document or SQLite schema changes.
- **Other clients:** iOS and Android show these chats like any other. They don't create project worktrees yet.
