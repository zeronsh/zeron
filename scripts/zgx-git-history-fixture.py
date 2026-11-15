#!/usr/bin/env python3
"""Write git_scroll_cost input from real commits, optionally stressing topology."""

import argparse
import json
import subprocess
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--count", type=int, default=100)
    parser.add_argument("--rev", default="HEAD")
    parser.add_argument("--all", action="store_true", help="Read all refs instead of --rev")
    parser.add_argument("--shape", choices=("real", "linear", "pending"), default="real")
    parser.add_argument("--extra-parents", type=int, default=8, help="Synthetic pending parents per commit")
    parser.add_argument("--github", action="store_true", help="Include mocked GitHub author/PR replies")
    args = parser.parse_args()
    if args.count < 1 or args.extra_parents < 0:
        parser.error("count must be positive and extra-parents nonnegative")

    cmd = ["git", "log", "--topo-order", f"-{args.count}", "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%s"]
    cmd += ["--all"] if args.all else [args.rev]
    cmd += ["--"]
    commits = []
    for line in subprocess.check_output(cmd).decode().splitlines():
        sha, parents, author, email, at, refs, subject = line.split("\x1f", 6)
        commits.append(dict(sha=sha, parents=parents.split(), author=author, email=email,
                            at=int(at), refs=refs.split(", ") if refs else [], subject=subject))
    if not commits:
        parser.error("the selected revision has no commits")
    if args.shape != "real":
        for i, commit in enumerate(commits):
            commit["parents"] = [commits[i + 1]["sha"]] if i + 1 < len(commits) else []
            if args.shape == "pending":
                commit["parents"] += [f"{30000 + i * args.extra_parents + j:040x}" for j in range(args.extra_parents)]

    world = dict(
        status=dict(branch="main", oid=commits[0]["sha"], conflicts=0, changes=0,
                    tree="t1", index="i1", device="devbox", remote="origin",
                    remote_url="git@github.com:zeronsh/zeron.git" if args.github else "", who="Ada"),
        changes={scope: dict(scope=scope, a="a", b="b", files=[]) for scope in ("working", "branch", "turn", "commit")},
        history=dict(more=False, commits=commits), diff=dict(text=""), fail={},
    )
    if args.github:
        world["github"] = dict(viewer="ada", repository="zeronsh/zeron", branches={}, commits={
            c["sha"]: dict(
                author=dict(user=dict(login=c["author"], avatarUrl="https://avatars.githubusercontent.com/u/1")),
                associatedPullRequests=dict(nodes=[dict(number=i + 1, title=c["subject"],
                    url=f"https://github.com/zeronsh/zeron/pull/{i + 1}", state="MERGED", merged=True)]),
            ) for i, c in enumerate(commits)
        })
    args.output.write_text(json.dumps(world) + "\n")


if __name__ == "__main__":
    main()
