#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import sys
import urllib.error
import urllib.request
from datetime import datetime, timedelta, timezone
from urllib.parse import urlencode

PR_WORKFLOW = "pr-ci.yml"
ACTIVE = ("queued", "in_progress", "waiting", "pending", "requested")
RETRYABLE = ("failure", "cancelled", "timed_out")


def to_cancel(runs: list[dict], protected: set[int], cancel_main: bool) -> list[dict]:
    chosen = []
    for run in runs:
        if run.get("status") not in ACTIVE or run["id"] in protected:
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
    return own[0] if own[0].get("conclusion") in RETRYABLE else None


def from_pull_request(run: dict, pr: dict) -> bool:
    head = pr["head"]
    run_repo = run.get("head_repository") or {}
    associated = run.get("pull_requests") or []
    return (run.get("event") == "pull_request" and run.get("head_branch") == head["ref"]
            and run_repo.get("id") == head["repo"]["id"]
            and (not associated or any(item["number"] == pr["number"] for item in associated)))


def request(method: str, path: str):
    url = os.environ.get("GITHUB_API_URL", "https://api.github.com") + path
    req = urllib.request.Request(url, method=method, headers={
        "Authorization": f"Bearer {os.environ['GITHUB_TOKEN']}",
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
    })
    with urllib.request.urlopen(req, timeout=30) as response:
        text = response.read()
        return json.loads(text) if text else None


def workflow_runs(repo: str, workflow: str) -> list[dict]:
    runs: list[dict] = []
    seen: set[int] = set()
    for status in ACTIVE:
        page = 1
        while True:
            batch = request("GET", f"/repos/{repo}/actions/workflows/{workflow}/runs"
                                   f"?status={status}&per_page=100&page={page}")["workflow_runs"]
            for run in batch:
                if run["id"] not in seen:
                    seen.add(run["id"])
                    runs.append(run)
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
    head = pr["head"]
    if not head.get("repo"):
        print(f"::error::#{number}'s source repository is unavailable.")
        return 1
    query = urlencode({"event": "pull_request", "head_sha": head["sha"], "branch": head["ref"], "per_page": 100})
    candidates = request("GET", f"/repos/{repo}/actions/workflows/{PR_WORKFLOW}/runs?{query}")["workflow_runs"]
    recent = [run for run in candidates if from_pull_request(run, pr) and run["head_sha"] == head["sha"]]
    if not recent:
        print(f"::error::#{number} has no Pull request CI run for its current head. Update the branch to start one.")
        return 1
    latest = max(recent, key=lambda run: run["id"])
    if latest.get("status") == "action_required" or latest.get("conclusion") == "action_required":
        print(f"::error::Review and approve #{number}'s Pull request CI run before using CI emergency.")
        return 1

    lines = [f"### CI emergency for #{number}", ""]
    rerun = to_rerun(recent, head["sha"])
    if rerun:
        created = datetime.fromisoformat(rerun["created_at"].replace("Z", "+00:00"))
        if datetime.now(timezone.utc) - created >= timedelta(days=30):
            print(f"::error::#{number}'s CI run is too old to re-run. Update the branch to start a new run.")
            return 1
        try:
            request("POST", f"/repos/{repo}/actions/runs/{rerun['id']}/rerun")
        except urllib.error.HTTPError as error:
            print(f"::error::Could not re-run #{number}'s CI run {rerun['id']} ({error.code}).")
            return 1
        lines.append(f"Re-ran #{number}'s CI run {rerun['id']}.")
    elif any(run.get("status") in ACTIVE for run in recent):
        lines.append(f"#{number}'s CI is active.")
    elif latest.get("conclusion") == "success":
        print(f"#{number}'s CI already passed.")
        return 0
    else:
        print(f"::error::#{number}'s CI concluded {latest.get('conclusion')}. Update the branch to start a new run.")
        return 1

    active = workflow_runs(repo, PR_WORKFLOW) + workflow_runs(repo, "ci.yml")
    protected = {run["id"] for run in active if from_pull_request(run, pr)}
    cancelled = []
    for run in to_cancel(active, protected, cancel_main):
        try:
            request("POST", f"/repos/{repo}/actions/runs/{run['id']}/cancel")
            cancelled.append(run)
        except OSError as error:  # already finished, or not cancellable
            print(f"::warning::could not cancel run {run['id']}: {error}")
    lines.append(f"Cancelled {len(cancelled)} other CI run(s):")
    lines += [f"- [{r.get('display_title', r['id'])}]({r.get('html_url', '')}) ({r.get('event')})" for r in cancelled]

    text = "\n".join(lines)
    print(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as f:
            f.write(text + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
