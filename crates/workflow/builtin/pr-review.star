# zeron-workflow
# name: pr-review
# description: Independent reviewers examine the changes; a fresh agent then tries to disprove every finding
# when_to_use: When the user asks for a thorough, multi-agent review of the current branch or the uncommitted changes
# args:
#   base: {type: string, default: "main", description: "Branch to compare against"}
#   deep: {type: bool, default: false, description: "Add a fourth reviewer for tests and documentation"}
#   max_findings: {type: int, default: 12, description: "Confirm at most this many findings, most severe first"}

ANGLES = [
    ["security", "You are a security reviewer."],
    ["correctness", "You look for logic errors, broken edge cases and races."],
    ["maintainability", "You look for code that will be hard to change or easy to misuse."],
]
DEEP_ANGLES = [["tests and docs", "You check that behaviour changes are tested and documented."]]
RANK = {"high": 0, "medium": 1, "low": 2}

FINDING = schema.obj({
    "where": schema.str("file:line the problem is at"),
    "what": schema.str("what is wrong, in one or two sentences"),
    "evidence": schema.str("what you read or ran that shows it"),
    "severity": schema.enum(["high", "medium", "low"]),
})
REVIEW = schema.obj({"findings": schema.list(FINDING)})
VERDICT = schema.obj({
    "confirmed": schema.bool("true only if you reproduced or directly verified the problem"),
    "evidence": schema.str("what you did and saw"),
})
SUMMARY = schema.obj({"conclusion": schema.str("two or three sentences for the user")})

def confirm(finding):
    # A fresh agent per finding: it has not seen the reviewer's reasoning.
    verdict = agent("checker").ask(
        "Try to disprove this finding. Read the code it points at and run what you need.\n" + json.encode(finding),
        schema = VERDICT,
        read_only = True,
    ).result()
    if verdict.ok:
        status = "verified" if verdict.value["confirmed"] else "unconfirmed"
        return {"finding": finding, "status": status, "evidence": verdict.value["evidence"]}
    return {"finding": finding, "status": "unconfirmed", "evidence": verdict.error}

def main(args):
    base = args["base"]
    changed = git.changed_files(base = base)
    if not changed:
        return {
            "conclusion": "Nothing to review: no changes against " + base + ".",
            "findings": [],
            "verified": [],
            "not_covered": [],
        }

    phase("review")
    angles = ANGLES + (DEEP_ANGLES if args["deep"] else [])
    reviewers = [agent(a[0] + " reviewer", persona = a[1]) for a in angles]
    handles = [
        r.ask(
            "Review the changes against `" + base + "` for " + a[0] + " problems. Files changed:\n"
            + "\n".join(changed[:100])
            + "\nAsk what would fail. Report only real problems, each with evidence; return an empty list if there are none. Do not modify files.",
            schema = REVIEW,
            read_only = True,
        )
        for r, a in zip(reviewers, angles)
    ]
    found = []
    for review in results(handles):
        found += review["findings"]
    found = sorted(found, key = lambda f: RANK[f["severity"]])[:args["max_findings"]]
    report({"reviewed": len(found)})

    phase("confirm")
    checked = pmap(confirm, found)
    verified = [c for c in checked if c["status"] == "verified"]
    report({"verified": len(verified), "unconfirmed": len(checked) - len(verified)})

    phase("summarize")
    artifact.table(
        "findings", "Findings", ["where", "severity", "status", "what"],
        [[c["finding"]["where"], c["finding"]["severity"], c["status"], c["finding"]["what"]] for c in checked],
    )
    summary = agent("judge").ask(
        "Write the conclusion of this review for the user. Say what is verified and what is not.\n" + json.encode(checked),
        schema = SUMMARY,
        read_only = True,
    ).result()
    return {
        "conclusion": summary.value["conclusion"] if summary.ok else "No conclusion was produced: " + str(summary.error),
        "findings": [
            {
                "where": c["finding"]["where"],
                "what": c["finding"]["what"],
                "evidence": c["evidence"],
                "status": c["status"],
                "severity": c["finding"]["severity"],
            }
            for c in checked
        ],
        "verified": [str(len(verified)) + " of " + str(len(checked)) + " findings reproduced by an independent agent"],
        "not_covered": ["Only the " + str(len(changed)) + " changed file(s) against " + base + " were reviewed."],
    }
