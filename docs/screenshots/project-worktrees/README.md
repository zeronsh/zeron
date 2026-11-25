# Project worktrees

A project folder holding several repositories, checked out whole: the composer's Work in panel lists its repositories in place of branches when a worktree is picked, and the Git tab shows one repository of the chat's folder at a time ([docs/reference/project-worktrees.md](../../reference/project-worktrees.md)).

Rendered offscreen from `ui/chat_fixture.luau` (`fixture.acme_project`, `fixture.clones_project`) with `src/audit_shots.rs`, and the Git tab from the Git plugin with its service played (`git_shows_one_of_a_folder_s_repositories_and_switches_between_them`'s set-up), on Linux under Xvfb at 2×.

- `panel-*.png`: Acme, its repositories each on its branch; one with no commit can't be chosen.
- `chosen-*.png`: one let go; the chip counts those chosen.
- `none-dark.png`, `blocked-dark.png`: none chosen, and the composer saying why it can't start.
- `many-*.png`: a folder of a dozen clones starts with none chosen; the list scrolls past six.
- `git-panel-dark.png`: the Git tab on a project worktree, its repository chip open.
