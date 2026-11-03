//! Recognize npm's `.cmd` shims so Windows can launch what they wrap directly.
//!
//! Every npm-installed CLI on Windows is a `name.cmd` written by npm's
//! cmd-shim, and a batch file only runs through `cmd.exe`, which brings its
//! own limits: no UNC working directory (a project on `\\wsl.localhost\…` or
//! a network share silently runs in `C:\Windows`), an 8191-character command
//! line in which every `%` we escape grows by seven, and no CR/LF in
//! arguments. The shims do one thing: run `node <script>` (preferring a
//! `node.exe` beside the shim) or run a native `.exe` target. Recognizing
//! exactly the layouts cmd-shim generates lets the launcher do the same
//! without `cmd.exe`; anything else — hand-written batch files, shims that
//! set variables, pnpm's NODE_PATH shims — keeps the `cmd.exe` route.
//!
//! The parser is a whitelist over cmd-shim's statements, current (npm 7+)
//! and older (npm 6 and earlier) alike: any line it does not know rejects
//! the whole file.

/// What a recognized shim runs, relative to the shim's own directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ShimTarget {
    /// A native executable, launched as-is with the caller's arguments.
    Exe(String),
    /// A script for `node`, launched as `node <args> <script> …`.
    Node { script: String, args: Vec<String> },
}

/// cmd-shim files are a few hundred bytes; anything larger is not one.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const MAX_SHIM_BYTES: u64 = 16 * 1024;

/// Statements cmd-shim writes around the launch line, lowercased, with the
/// optional leading `@` removed.
const STRUCTURE: &[&str] = &[
    "",
    "echo off",
    "goto start",
    ":start",
    ":find_dp0",
    "set dp0=%~dp0",
    "exit /b",
    "exit /b %errorlevel%",
    "setlocal",
    "endlocal",
    "call :find_dp0",
    ")",
    ") else (",
    "set pathext=%pathext:;.js;=;%",
    "if exist \"%dp0%\\node.exe\" (",
    "if exist \"%~dp0\\node.exe\" (",
    "set \"_prog=%dp0%\\node.exe\"",
    "set \"_prog=node\"",
];

/// The current shims' guard against cmd's "Terminate batch job?" prompt.
const LAUNCH_PREFIX: &str = "endlocal & goto #_undefined_# 2>nul || title %comspec% & ";

/// How the shim directory is spelled: the current `%dp0%`, the old `%~dp0`.
const DIR_PREFIXES: [&str; 2] = ["%dp0%\\", "%~dp0\\"];

/// Interpreters cmd-shim selects for a `#!/usr/bin/env node` script.
const NODE_PROGRAMS: [&str; 4] = [
    "\"%_prog%\"",
    "\"%dp0%\\node.exe\"",
    "\"%~dp0\\node.exe\"",
    "node",
];

pub(crate) fn parse(contents: &str) -> Option<ShimTarget> {
    let mut target: Option<ShimTarget> = None;
    for line in contents.lines() {
        let line = line.trim();
        let line = line.strip_prefix('@').unwrap_or(line).trim_start();
        let lower = line.to_ascii_lowercase();
        if STRUCTURE.contains(&lower.as_str()) {
            continue;
        }
        let launch = parse_launch(line)?;
        // The old layout launches twice (node.exe beside the shim, else
        // node on PATH); both must agree.
        if target.as_ref().is_some_and(|seen| *seen != launch) {
            return None;
        }
        target = Some(launch);
    }
    target
}

/// One launch line: `[guard] [program [args]] "<dir>\<target>" %*`.
fn parse_launch(line: &str) -> Option<ShimTarget> {
    let lower = line.to_ascii_lowercase();
    // ASCII lowercasing keeps byte offsets, so `lower` indexes `line`.
    let start = if lower.starts_with(LAUNCH_PREFIX) {
        LAUNCH_PREFIX.len()
    } else {
        0
    };
    let rest = line[start..].trim_end().strip_suffix("%*")?.trim_end();
    let inner = rest.strip_suffix('"')?;
    let open = inner.rfind('"')?;
    let quoted = &inner[open + 1..];
    let relative = DIR_PREFIXES.iter().find_map(|prefix| {
        quoted
            .get(..prefix.len())
            .filter(|head| head.eq_ignore_ascii_case(prefix))
            .map(|_| &quoted[prefix.len()..])
    })?;
    if relative.is_empty() || !safe(relative) {
        return None;
    }
    let head = rest[..open].trim();
    if head.is_empty() {
        let extension = relative.rsplit_once('.')?.1;
        return (extension.eq_ignore_ascii_case("exe") || extension.eq_ignore_ascii_case("com"))
            .then(|| ShimTarget::Exe(relative.to_owned()));
    }
    let program = NODE_PROGRAMS.iter().find(|program| {
        head.get(..program.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(program))
            && head[program.len()..]
                .chars()
                .next()
                .is_none_or(char::is_whitespace)
    })?;
    let args: Vec<String> = head[program.len()..]
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    args.iter().all(|arg| safe(arg)).then(|| ShimTarget::Node {
        script: relative.to_owned(),
        args,
    })
}

/// Text cmd.exe would take literally: no expansion, quoting or operators.
fn safe(text: &str) -> bool {
    !text.chars().any(|c| {
        matches!(c, '%' | '"' | '!' | '^' | '&' | '|' | '<' | '>' | '(' | ')') || c.is_control()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Written by npm 11's cmd-shim (7.x) for `#!/usr/bin/env node` scripts.
    const NODE_SCRIPT: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@anthropic-ai\\claude-code\\cli.js\" %*\r\n";

    // …for a target without a shebang (a native binary).
    const NATIVE_EXE: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\"%dp0%\\node_modules\\@scope\\cli\\bin\\native.exe\"   %*\r\n";

    // …for `#!/usr/bin/env node --max-old-space-size=4096`.
    const NODE_WITH_ARGS: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\" --max-old-space-size=4096 \"%dp0%\\node_modules\\@scope\\cli\\args.js\" %*\r\n";

    // …for `#!/usr/bin/env -S NODE_OPTIONS=--x node`: sets a variable.
    const NODE_WITH_VARIABLES: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n@SET NODE_OPTIONS=--x\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@scope\\cli\\vars.js\" %*\r\n";

    // …for `#!/usr/local/bin/node`: an interpreter other than node on PATH.
    const OTHER_INTERPRETER: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\/usr/local/bin/node.exe\" (\r\n  SET \"_prog=%dp0%\\/usr/local/bin/node.exe\"\r\n) ELSE (\r\n  SET \"_prog=/usr/local/bin/node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@scope\\cli\\direct.js\" %*\r\n";

    // cmd-shim 3.x (npm 6): the launch line before the subroutine footer.
    const NPM6_NODE_SCRIPT: &str = "@ECHO off\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\n\"%_prog%\"  \"%dp0%\\node_modules\\@openai\\codex\\bin\\codex.js\" %*\r\nENDLOCAL\r\nEXIT /b %errorlevel%\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n";

    // cmd-shim 2.x (npm 5 and yarn 1's global bin): two launch lines.
    const NPM5_NODE_SCRIPT: &str = "@IF EXIST \"%~dp0\\node.exe\" (\r\n  \"%~dp0\\node.exe\"  \"%~dp0\\..\\opencode-ai\\bin\\opencode\" %*\r\n) ELSE (\r\n  @SETLOCAL\r\n  @SET PATHEXT=%PATHEXT:;.JS;=;%\r\n  node  \"%~dp0\\..\\opencode-ai\\bin\\opencode\" %*\r\n)";

    #[test]
    fn npm_node_script_shims_name_their_script() {
        assert_eq!(
            parse(NODE_SCRIPT),
            Some(ShimTarget::Node {
                script: r"node_modules\@anthropic-ai\claude-code\cli.js".into(),
                args: Vec::new(),
            })
        );
        assert_eq!(
            parse(NODE_WITH_ARGS),
            Some(ShimTarget::Node {
                script: r"node_modules\@scope\cli\args.js".into(),
                args: vec!["--max-old-space-size=4096".into()],
            })
        );
    }

    #[test]
    fn npm_native_shims_name_their_executable() {
        assert_eq!(
            parse(NATIVE_EXE),
            Some(ShimTarget::Exe(
                r"node_modules\@scope\cli\bin\native.exe".into()
            ))
        );
    }

    #[test]
    fn older_npm_layouts_are_recognized() {
        assert_eq!(
            parse(NPM6_NODE_SCRIPT),
            Some(ShimTarget::Node {
                script: r"node_modules\@openai\codex\bin\codex.js".into(),
                args: Vec::new(),
            })
        );
        assert_eq!(
            parse(NPM5_NODE_SCRIPT),
            Some(ShimTarget::Node {
                script: r"..\opencode-ai\bin\opencode".into(),
                args: Vec::new(),
            })
        );
    }

    #[test]
    fn shims_that_do_more_keep_the_cmd_route() {
        // Variables, a different interpreter, or anything hand-written.
        assert_eq!(parse(NODE_WITH_VARIABLES), None);
        assert_eq!(parse(OTHER_INTERPRETER), None);
        assert_eq!(parse("@echo off\r\necho codex-cli 1.0.0\r\n"), None);
        assert_eq!(parse(""), None);
        // A native target must be an executable image, not a script.
        assert_eq!(parse(&NATIVE_EXE.replace("native.exe", "native.ps1")), None);
        // Expansion or operators anywhere in the launch line.
        for (from, to) in [
            ("cli.js", "%USERPROFILE%.js"),
            ("cli.js", "a&b.js"),
            ("cli.js", "a!b.js"),
            ("\"%_prog%\"  ", "\"%_prog%\" --x=%PATH% "),
            ("\"%_prog%\"  ", "\"%_prog%\" a&calc "),
            ("\"%_prog%\"  ", "\"%_prog%\"x "),
        ] {
            assert_eq!(parse(&NODE_SCRIPT.replace(from, to)), None, "{to}");
        }
        // Two launches that disagree.
        let disagreeing = NPM5_NODE_SCRIPT.replacen("opencode\" %*", "other\" %*", 1);
        assert_eq!(parse(&disagreeing), None);
    }

    #[test]
    fn line_endings_and_case_do_not_matter() {
        let unix = NODE_SCRIPT.replace("\r\n", "\n").to_uppercase();
        assert_eq!(
            parse(&unix),
            Some(ShimTarget::Node {
                script: r"NODE_MODULES\@ANTHROPIC-AI\CLAUDE-CODE\CLI.JS".into(),
                args: Vec::new(),
            })
        );
    }
}
