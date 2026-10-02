# zeron-workflow
# name: fix-until-green
# description: Run a cargo gate, let one agent fix what fails (at most a few rounds), then have a fresh agent check the fix did not cheat
# when_to_use: When tests, clippy or the build are red and the user wants them fixed without supervision
# args:
#   check: {type: string, default: "test", description: "Cargo subcommand used as the gate (test, clippy, check, build)"}
#   extra: {type: json, default: ["--workspace"], description: "Extra cargo arguments, as a list"}
#   rounds: {type: int, default: 3, description: "Most fix attempts before giving up"}

FIX = schema.obj({"changed": schema.list(schema.str("file path")), "notes": schema.str()})
AUDIT = schema.obj({
    "clean": schema.bool("true only if the diff fixes the cause without weakening tests or silencing checks"),
    "concerns": schema.list(schema.str("one concern, citing the file and line")),
})

def gate(args):
    # Whether it passes is a command's exit code, not an agent's opinion.
    return run("cargo", [args["check"]] + args["extra"], timeout_s = 900)

def main(args):
    phase("fix")
    fixer = agent("fixer", persona = "You fix failing builds and tests at their cause. You never delete, skip or loosen a test or silence a lint to get a pass.")
    result = gate(args)
    rounds = 0
    for _ in range(args["rounds"]):
        if result.ok:
            break
        rounds += 1
        fixer.ask(
            "`cargo " + args["check"] + "` fails. Fix the cause in the source. Output (tail):\n" + result.stderr[-3000:] + result.stdout[-1000:],
            schema = FIX,
        ).result()
        result = gate(args)
    report({"green": result.ok, "fix_rounds": rounds})

    phase("review")
    audit = agent("auditor").ask(
        "Read `git diff` and look only for cheating: tests deleted, ignored or weakened, assertions loosened, lints silenced, errors swallowed. Do not modify files.",
        schema = AUDIT,
        read_only = True,
    ).result()
    return {
        "conclusion": ("The gate passes" if result.ok else "The gate is still failing") + " after " + str(rounds) + " fix round(s).",
        "green": result.ok,
        "clean": audit.value["clean"] if audit.ok else None,
        "concerns": audit.value["concerns"] if audit.ok else [],
        "verified": ["cargo " + args["check"] + " " + ("passed" if result.ok else "failed") + " on the last run"],
        "not_covered": ["The independent diff review is an agent's judgement, not a proof."],
    }
