# zeron-workflow
# name: repo-audit
# description: Read-only audit of a set of files by parallel auditors, every finding confirmed by a fresh agent, delivered as a report
# when_to_use: When the user wants a broad read-only audit (security, dead code, risky patterns) of part of the repository
# args:
#   glob: {type: string, default: "src/**", description: "Files to audit, as a glob"}
#   focus: {type: string, default: "bugs and risky patterns", description: "What the auditors look for"}
#   auditors: {type: int, default: 4, description: "How many auditors share the files"}

FINDING = schema.obj({
    "where": schema.str("file:line"),
    "what": schema.str("the problem, in one or two sentences"),
    "evidence": schema.str("the code you read that shows it"),
})
AUDIT = schema.obj({"findings": schema.list(FINDING)})
VERDICT = schema.obj({
    "confirmed": schema.bool("true only if the code really has the problem"),
    "evidence": schema.str("what you checked"),
})

def confirm(finding):
    verdict = agent("checker").ask(
        "Try to show this finding is wrong. Read the code it points at.\n" + json.encode(finding),
        schema = VERDICT,
        read_only = True,
    ).result()
    ok = verdict.ok and verdict.value["confirmed"]
    return {
        "finding": finding,
        "status": "verified" if ok else "unconfirmed",
        "evidence": verdict.value["evidence"] if verdict.ok else verdict.error,
    }

def main(args):
    files_found = files.glob(args["glob"])
    if not files_found:
        return {"conclusion": "No files match " + args["glob"] + ".", "findings": [], "not_covered": []}
    n = max(1, min(args["auditors"], len(files_found)))
    size = (len(files_found) + n - 1) // n

    phase("audit")
    handles = []
    for i in range(n):
        part = files_found[i * size:(i + 1) * size]
        handles.append(agent("auditor " + str(i + 1)).ask(
            "Audit these files for " + args["focus"] + ". Read them; report only real problems with the code as evidence. Do not modify anything.\n" + "\n".join(part),
            schema = AUDIT,
            read_only = True,
        ))
    found = []
    for audit in results(handles):
        found += audit["findings"]
    report({"candidates": len(found)})

    phase("confirm")
    checked = pmap(confirm, found)
    verified = [c for c in checked if c["status"] == "verified"]
    report({"verified": len(verified), "unconfirmed": len(checked) - len(verified)})

    phase("report")
    text = "# Audit: " + args["focus"] + "\n\n" + str(len(files_found)) + " file(s) under `" + args["glob"] + "`; "
    text += str(len(verified)) + " of " + str(len(checked)) + " findings confirmed.\n\n"
    for c in checked:
        text += "- **" + c["status"] + "** `" + c["finding"]["where"] + "`: " + c["finding"]["what"] + "\n"
    artifact.markdown("report", "Audit report", text)
    judge = agent("editor").ask(
        "Write a two or three sentence conclusion for the user from this audit. Separate what is verified from what is not.\n" + json.encode(checked),
        schema = schema.obj({"conclusion": schema.str()}),
        read_only = True,
    ).result()
    return {
        "conclusion": judge.value["conclusion"] if judge.ok else "No conclusion was produced: " + str(judge.error),
        "findings": [
            {"where": c["finding"]["where"], "what": c["finding"]["what"], "evidence": c["evidence"], "status": c["status"]}
            for c in checked
        ],
        "not_covered": ["Only files matching " + args["glob"] + " were read."],
    }
