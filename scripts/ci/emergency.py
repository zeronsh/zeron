#!/usr/bin/env python3
"""CI emergency: give one pull request every runner, now.

Run by .github/workflows/ci-emergency.yml (manual; needs write access). It:

  1. adds the `ci:lock` label to the pull request (removing it first if it was
     there, which restarts its CI_LOCK_HOURS), so admission keeps a full run's
     runners for it until it is merged, closed or unlabelled
     (scripts/ci/admit.py);
  2. cancels every other pull request's CI run that is queued, waiting for
     admission or running (main and merge queue runs too, with CANCEL_MAIN);
  3. re-runs the pull request's latest CI run if none is in progress and the
     last one did not pass.

Cancelled pull requests need a push or a re-run afterwards.
"""

from __future__ import annotations

import json
import os
import sys
import urllib.request
from urllib.parse import quote

LOCK_LABEL = "ci:lock"
ACTIVE = ("queued", "in_progress", "waiting", "pending", "requested")


def to_cancel(runs: list[dict], head_sha: str, cancel_main: bool) -> list[dict]:
    """The active CI runs to cancel: every one not testing the pull request's head."""
    chosen = []
    for run in runs:
        if run.get("status") not in ACTIVE or run.get("head_sha") == head_sha:
            continue
        if run.get("event") == "pull_request" or cancel_main:
            chosen.append(run)
    return chosen


def to_rerun(runs: list[dict], head_sha: str) -> dict | None:
    """The pull request's latest CI run, if none is active and it did not pass."""
    own = sorted((r for r in runs if r.get("head_sha") == head_sha and r.get("event") == "pull_request"),
                 key=lambda r: r["id"], reverse=True)
    if not own or any(r.get("status") in ACTIVE for r in own):
        return None
    return own[0] if own[0].get("conclusion") != "success" else None


def request(method: str, path: str, body: dict | None = None):
    url = os.environ.get("GITHUB_API_URL", "https://api.github.com") + path
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method, headers={
        "Authorization": f"Bearer {os.environ['GITHUB_TOKEN']}",
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
    })
    with urllib.request.urlopen(req, timeout=30) as response:
        text = response.read()
        return json.loads(text) if text else None


def workflow_runs(repo: str, workflow: str) -> list[dict]:
    runs: list[dict] = []
    for status in ACTIVE:
        page = 1
        while True:
            batch = request("GET", f"/repos/{repo}/actions/workflows/{workflow}/runs"
                                   f"?status={status}&per_page=100&page={page}")["workflow_runs"]
            runs.extend(batch)
            if len(batch) < 100:
                break
            page += 1
    return runs


def main() -> int:
    repo = os.environ["GITHUB_REPOSITORY"]
    number = int(os.environ["PR"])
    cancel_main = os.environ.get("CANCEL_MAIN", "false") == "true"
    pr = request("GET", f"/repos/{repo}/pulls/{number}")
    if pr["state"] != "open":
        print(f"::error::#{number} is {pr['state']}")
        return 1
    head = pr["head"]["sha"]

    if any(label["name"] == LOCK_LABEL for label in pr.get("labels", [])):
        request("DELETE", f"/repos/{repo}/issues/{number}/labels/{quote(LOCK_LABEL)}")
    request("POST", f"/repos/{repo}/issues/{number}/labels", {"labels": [LOCK_LABEL]})
    lines = [f"### CI emergency for #{number}", "", f"Locked: `{LOCK_LABEL}` added to #{number}."]

    active = workflow_runs(repo, "ci.yml")
    cancelled = []
    for run in to_cancel(active, head, cancel_main):
        try:
            request("POST", f"/repos/{repo}/actions/runs/{run['id']}/cancel")
            cancelled.append(run)
        except OSError as error:  # already finished, or not cancellable
            print(f"::warning::could not cancel run {run['id']}: {error}")
    lines.append(f"Cancelled {len(cancelled)} other CI run(s):")
    lines += [f"- [{r.get('display_title', r['id'])}]({r.get('html_url', '')}) ({r.get('event')})" for r in cancelled]

    recent = request("GET", f"/repos/{repo}/actions/workflows/ci.yml/runs?head_sha={head}&per_page=20")["workflow_runs"]
    rerun = to_rerun(active + recent, head)
    if rerun:
        request("POST", f"/repos/{repo}/actions/runs/{rerun['id']}/rerun")
        lines.append(f"Re-ran #{number}'s CI run {rerun['id']}.")
    else:
        lines.append(f"#{number}'s CI is running or passed; nothing to re-run.")
    lines += ["", f"Remove `{LOCK_LABEL}` (or merge/close #{number}) to release the runners."]

    text = "\n".join(lines)
    print(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as f:
            f.write(text + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
