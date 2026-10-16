//! Free-form launch arguments for an agent CLI (`ChatConfig::launch_args`,
//! `RunRequest::launch_args`).
//!
//! The arguments are passed to the agent process verbatim on every run of the
//! chat. Zeron itself drives each CLI over a protocol (stream-json on stdio,
//! JSON-RPC to `app-server`) and sets the flags that protocol depends on, so
//! each harness has a list of flags Zeron manages that callers may not
//! override. [`validate`] is the single gate: the MCP server runs it before
//! creating a chat, and the host and harness re-run it before spawning.

use crate::HarnessId;

/// Most arguments one chat may carry.
pub const MAX_ARGS: usize = 64;

/// Claude Code long flags Zeron sets itself, or that would break the
/// stream-json stdio protocol, session handling, or the title run.
const CLAUDE_RESERVED_LONG: &[&str] = &[
    "--print",
    "--input-format",
    "--output-format",
    "--verbose",
    "--include-partial-messages",
    "--replay-user-messages",
    "--thinking-display",
    "--permission-prompt-tool",
    "--permission-mode",
    "--dangerously-skip-permissions",
    "--allow-dangerously-skip-permissions",
    "--resume",
    "--continue",
    "--session-id",
    "--fork-session",
    "--model",
    "--effort",
    "--settings",
    "--strict-mcp-config",
    "--system-prompt",
    "--system-prompt-file",
    "--no-session-persistence",
    "--cloud",
    "--remote",
    "--teleport",
    "--sdk-url",
    "--help",
    "--version",
];

/// Short aliases of [`CLAUDE_RESERVED_LONG`] (`-p`, `-r`, `-c`, `-h`, `-v`).
const CLAUDE_RESERVED_SHORT: &[char] = &['p', 'r', 'c', 'h', 'v'];

/// Codex config keys (`-c key=value`) Zeron sets per thread/turn over the
/// app-server protocol.
const CODEX_RESERVED_CONFIG_KEYS: &[&str] = &[
    "model",
    "model_reasoning_effort",
    "sandbox_mode",
    "approval_policy",
    "service_tier",
];

/// Whether launch args are implemented for `harness`.
pub fn supported(harness: HarnessId) -> bool {
    matches!(harness, HarnessId::ClaudeCode | HarnessId::Codex)
}

/// Check `args` for `harness`. `Ok` for an empty list on any harness; the
/// error text is user-facing.
pub fn validate(harness: HarnessId, args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Ok(());
    }
    if !supported(harness) {
        return Err(format!(
            "launch args are not supported for {} yet",
            harness_name(harness)
        ));
    }
    if args.len() > MAX_ARGS {
        return Err(format!(
            "too many launch args ({}; at most {MAX_ARGS})",
            args.len()
        ));
    }
    for (index, arg) in args.iter().enumerate() {
        if arg.is_empty() {
            return Err(format!("launch arg {index} is empty"));
        }
        if arg.contains(['\0', '\n', '\r']) {
            return Err(format!(
                "launch arg {index} contains a NUL or newline character"
            ));
        }
    }
    match harness {
        HarnessId::ClaudeCode => validate_claude(args),
        HarnessId::Codex => validate_codex(args),
        _ => unreachable!("`supported` gates every other harness"),
    }
}

fn harness_name(harness: HarnessId) -> String {
    serde_json::to_value(harness)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{harness:?}"))
}

fn managed(flag: &str, harness: &str) -> String {
    format!("`{flag}` is managed by Zeron and cannot be set via {harness} launch args")
}

fn validate_claude(args: &[String]) -> Result<(), String> {
    for arg in args {
        if arg == "--" {
            return Err("`--` is not allowed in claude-code launch args".into());
        }
        if let Some(long) = arg.strip_prefix("--") {
            let name = long.split_once('=').map_or(long, |(name, _)| name);
            if let Some(flag) = CLAUDE_RESERVED_LONG.iter().find(|flag| flag[2..] == *name) {
                return Err(managed(flag, "claude-code"));
            }
        } else if let Some(short) = arg.strip_prefix('-')
            && let Some(first) = short.chars().next()
            && CLAUDE_RESERVED_SHORT.contains(&first)
        {
            // `-p`, `-p=x`, `-pfoo` and bundles such as `-pr` all start the
            // reserved flag.
            return Err(managed(&format!("-{first}"), "claude-code"));
        }
    }
    Ok(())
}

/// Codex is spawned as `codex <args> app-server`, so only the global config
/// flags that app-server honors make sense: `-c/--config key=value`,
/// `--enable <feature>` and `--disable <feature>`. Anything else (a
/// subcommand, `--model`, `--sandbox`, …) would change what is launched or is
/// overridden by the protocol, and is rejected.
fn validate_codex(args: &[String]) -> Result<(), String> {
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let (flag, attached) = split_codex_flag(arg);
        let value = match flag {
            "-c" | "--config" | "--enable" | "--disable" => match attached {
                Some(value) => value.to_owned(),
                None => rest
                    .next()
                    .cloned()
                    .ok_or_else(|| format!("`{flag}` needs a value"))?,
            },
            _ => {
                return Err(format!(
                    "`{arg}` is not allowed in codex launch args \
                     (Zeron only accepts -c/--config, --enable and --disable)"
                ));
            }
        };
        if matches!(flag, "-c" | "--config") {
            let Some((key, _)) = value.split_once('=') else {
                return Err(format!("`{flag} {value}` must be key=value"));
            };
            let key = key.trim();
            if key.is_empty() {
                return Err(format!("`{flag} {value}` has an empty key"));
            }
            if CODEX_RESERVED_CONFIG_KEYS.contains(&key) {
                return Err(managed(key, "codex"));
            }
        } else if value.is_empty() || value.starts_with('-') {
            return Err(format!("`{flag}` needs a feature name, not `{value}`"));
        }
    }
    Ok(())
}

/// `--flag=value` and attached short values (`-ckey=value`, `-c=key=value`)
/// split into the flag and its attached value.
fn split_codex_flag(arg: &str) -> (&str, Option<&str>) {
    if arg.starts_with("--") {
        return match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value)),
            None => (arg, None),
        };
    }
    if let Some(value) = arg.strip_prefix("-c")
        && !value.is_empty()
    {
        return ("-c", Some(value.strip_prefix('=').unwrap_or(value)));
    }
    (arg, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    fn claude(list: &[&str]) -> Result<(), String> {
        validate(HarnessId::ClaudeCode, &args(list))
    }

    fn codex(list: &[&str]) -> Result<(), String> {
        validate(HarnessId::Codex, &args(list))
    }

    #[test]
    fn claude_accepts_the_documented_extras() {
        for list in [
            &["--chrome"][..],
            &["--no-chrome"],
            &["--add-dir", "../lib"],
            &["--agent", "reviewer"],
            &["--agents", "{\"a\":{}}"],
            &["--append-system-prompt", "be brief"],
            &["--mcp-config", "x.json", "--plugin-dir", "p"],
            &[
                "--allowedTools",
                "Bash",
                "Edit",
                "--disallowedTools",
                "Write",
            ],
            &["--fallback-model", "sonnet", "--betas", "x"],
            &["--add-dir=../lib"],
        ] {
            assert_eq!(claude(list), Ok(()), "{list:?}");
        }
    }

    #[test]
    fn empty_args_are_always_valid() {
        assert_eq!(validate(HarnessId::Mock, &[]), Ok(()));
        assert_eq!(validate(HarnessId::ClaudeCode, &[]), Ok(()));
    }

    #[test]
    fn claude_rejects_reserved_flags_naming_them() {
        for flag in CLAUDE_RESERVED_LONG {
            let err = claude(&[flag]).unwrap_err();
            assert!(err.contains(flag), "{err}");
            assert!(err.contains("managed by Zeron"), "{err}");
        }
    }

    #[test]
    fn claude_rejects_flag_equals_value_and_short_aliases() {
        let err = claude(&["--chrome", "--model=opus"]).unwrap_err();
        assert!(err.contains("`--model`"), "{err}");
        let err = claude(&["--permission-mode=plan"]).unwrap_err();
        assert!(err.contains("`--permission-mode`"), "{err}");
        for short in ["-p", "-r", "-c", "-h", "-v", "-pr", "-p=x", "-rabc"] {
            let err = claude(&[short]).unwrap_err();
            assert!(err.contains("managed by Zeron"), "{short}: {err}");
        }
        let err = claude(&["-p"]).unwrap_err();
        assert!(err.contains("`-p`"), "{err}");
        assert!(claude(&["--"]).is_err());
    }

    #[test]
    fn claude_prefix_lookalikes_are_not_reserved() {
        // `--model` is reserved, `--model-foo` and `--fallback-model` are not.
        assert_eq!(claude(&["--fallback-model", "x"]), Ok(()));
        assert_eq!(claude(&["--printer"]), Ok(()));
        assert_eq!(claude(&["--add-dir", "-"]), Ok(()));
    }

    #[test]
    fn rejects_limits_and_bad_characters() {
        assert!(claude(&[""]).unwrap_err().contains("empty"));
        assert!(claude(&["--chrome\0"]).is_err());
        assert!(claude(&["a\nb"]).is_err());
        assert!(claude(&["a\rb"]).is_err());
        let many = vec!["--chrome".to_owned(); MAX_ARGS];
        assert_eq!(validate(HarnessId::ClaudeCode, &many), Ok(()));
        let too_many = vec!["--chrome".to_owned(); MAX_ARGS + 1];
        assert!(
            validate(HarnessId::ClaudeCode, &too_many)
                .unwrap_err()
                .contains("too many")
        );
    }

    #[test]
    fn unsupported_harnesses_say_so() {
        for harness in [
            HarnessId::Cursor,
            HarnessId::Devin,
            HarnessId::Grok,
            HarnessId::Hermes,
            HarnessId::Pi,
            HarnessId::Opencode,
            HarnessId::Antigravity,
            HarnessId::Mock,
        ] {
            assert!(!supported(harness));
            let err = validate(harness, &args(&["--chrome"])).unwrap_err();
            assert!(err.contains("not supported for"), "{err}");
        }
        assert_eq!(
            validate(HarnessId::Cursor, &args(&["--x"])).unwrap_err(),
            "launch args are not supported for cursor yet"
        );
        assert!(supported(HarnessId::ClaudeCode));
        assert!(supported(HarnessId::Codex));
    }

    #[test]
    fn codex_accepts_global_config_flags() {
        for list in [
            &["-c", "model_provider=\"x\""][..],
            &["--config", "features.foo=true"],
            &["--config=shell_environment_policy.inherit=all"],
            &["-cfoo=1"],
            &["-c=foo=1"],
            &["--enable", "web_search", "--disable", "other"],
            &["--enable=web_search"],
        ] {
            assert_eq!(codex(list), Ok(()), "{list:?}");
        }
    }

    #[test]
    fn codex_rejects_subcommands_other_flags_and_managed_keys() {
        for list in [
            &["exec"][..],
            &["--model", "x"],
            &["-m", "x"],
            &["--sandbox", "read-only"],
            &["-p", "profile"],
            &["--listen", "ws://0.0.0.0:1"],
            &["-c"],
            &["-c", "novalue"],
            &["--enable"],
            &["--enable", "--disable"],
        ] {
            assert!(codex(list).is_err(), "{list:?}");
        }
        for key in CODEX_RESERVED_CONFIG_KEYS {
            let err = codex(&["-c", &format!("{key}=\"x\"")]).unwrap_err();
            assert!(
                err.contains(key) && err.contains("managed by Zeron"),
                "{err}"
            );
        }
        let err = codex(&["--config=model=\"x\""]).unwrap_err();
        assert!(err.contains("`model`"), "{err}");
    }
}
