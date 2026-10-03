//! Compact-title-only removal of recognizable shell launch wrappers.
//! Expanded blocks retain the original invocation for auditing.

/// Show the script rather than its shell executable and launch options in a title.
/// Only unwrap one layer: a shell explicitly invoked by the script is meaningful.
pub fn display_command(command: &str) -> String {
    unwrap_command(command).unwrap_or_else(|| command.to_owned())
}

fn unwrap_command(command: &str) -> Option<String> {
    let mut input = command.trim();
    // PowerShell's call operator, commonly used with quoted executable paths.
    let call_operator = input.starts_with('&');
    if call_operator {
        input = input[1..].trim_start();
    }
    let (executable, mut rest) = literal_word(input)?;
    if executable.contains([';', '|', '&', '<', '>', '$', '`']) {
        return None;
    }
    // Recognize Windows paths even when viewing them on macOS/Linux, and vice versa.
    let name = executable.rsplit(['/', '\\']).next()?;
    let windows_name = name.to_ascii_lowercase();
    let powershell = matches!(
        windows_name.as_str(),
        "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe"
    );
    let cmd = matches!(windows_name.as_str(), "cmd" | "cmd.exe");
    let unix = matches!(name, "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish")
        || matches!(
            windows_name.as_str(),
            "sh.exe" | "bash.exe" | "zsh.exe" | "dash.exe" | "ksh.exe" | "fish.exe"
        );
    if call_operator && !powershell {
        return None;
    }
    let mut cmd_strip_quotes = false;
    loop {
        let (option, after) = literal_word(rest)?;
        if powershell {
            let option = option.to_ascii_lowercase();
            if matches!(option.as_str(), "-command" | "-c") {
                let script = windows_script(after.trim())?;
                if script.trim().is_empty() || script == "-" {
                    return None;
                }
                return Some(script.to_owned());
            }
            match option.as_str() {
                "-nologo" | "-noprofile" | "-noninteractive" | "-sta" | "-mta" => rest = after,
                "-executionpolicy" | "-windowstyle" | "-inputformat" | "-outputformat" => {
                    let (value, after_value) = literal_word(after)?;
                    if value.starts_with('-') || value.is_empty() {
                        return None;
                    }
                    rest = after_value;
                }
                // File/encoded/argument-bearing modes and unknown switches are not wrappers.
                _ => return None,
            }
        } else if cmd {
            let option = option.to_ascii_lowercase();
            if option == "/c" {
                let script = after.trim();
                if script.is_empty() || script == "\"" || script == "\"\"" {
                    return None;
                }
                // /s strips exactly the first and last quotes, including the
                // doubled outer quotes around a quoted executable + arguments.
                let script = if script.starts_with('"')
                    && script.ends_with('"')
                    && (cmd_strip_quotes || script.starts_with("\"\"") && script.ends_with("\"\""))
                {
                    &script[1..script.len() - 1]
                } else {
                    // Without /s, quotes enclosing an executable path can be
                    // significant. Keep them rather than guessing filesystem state.
                    script
                };
                if script.trim().is_empty() {
                    return None;
                }
                return Some(script.to_owned());
            }
            match option.as_str() {
                "/s" => {
                    cmd_strip_quotes = true;
                    rest = after;
                }
                "/d" | "/q" | "/a" | "/u" | "/e:on" | "/e:off" | "/v:on" | "/v:off" | "/f:on"
                | "/f:off" => rest = after,
                _ => return None,
            }
        } else if unix {
            if option.starts_with('-')
                && option.len() > 1
                && option[1..].chars().all(|c| matches!(c, 'l' | 'i' | 'c'))
                && option.chars().filter(|c| *c == 'c').count() <= 1
            {
                if option.contains('c') {
                    let (script, suffix) = posix_word(after)?;
                    // Extra arguments bind $0/$1; outer redirections/operators
                    // also belong to this invocation. Do not silently drop them.
                    if script.trim().is_empty() || !suffix.trim().is_empty() {
                        return None;
                    }
                    return Some(script);
                }
                rest = after;
            } else if matches!(
                option,
                "--login" | "--interactive" | "--noprofile" | "--norc"
            ) {
                rest = after;
            } else {
                return None;
            }
        } else {
            return None;
        }
    }
}

/// Executable/option words are literal, not shell-expanded. In particular, do
/// not treat backslashes in an unquoted Windows path as POSIX escapes.
fn literal_word(input: &str) -> Option<(&str, &str)> {
    let input = input.trim_start();
    let first = input.chars().next()?;
    if matches!(first, '\'' | '"') {
        let end = input[1..].find(first)? + 1;
        let rest = &input[end + 1..];
        if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            return None;
        }
        Some((&input[1..end], rest))
    } else {
        let end = input.find(char::is_whitespace).unwrap_or(input.len());
        let word = &input[..end];
        if word.contains(['\'', '"']) {
            return None;
        }
        Some((word, &input[end..]))
    }
}

/// Remove a single, complete Windows command-string enclosure. Preserve all
/// inner spelling (backslashes, PowerShell backticks and doubled quotes).
fn windows_script(script: &str) -> Option<&str> {
    let quote = *script.as_bytes().first()?;
    if !matches!(quote, b'\'' | b'"') {
        return Some(script);
    }
    let bytes = script.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        if matches!(bytes[i], b'\\' | b'`') && quote == b'"' {
            i += 2;
        } else if bytes[i] == quote {
            if i == bytes.len() - 1 {
                return Some(&script[1..i]);
            }
            if bytes[i + 1] == quote {
                i += 2;
            } else {
                return None;
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Decode one POSIX shell word without evaluating substitutions. This handles
/// shell_words-style joined commands, including the '\'' apostrophe sequence.
fn posix_word(input: &str) -> Option<(String, &str)> {
    let input = input.trim_start();
    let mut chars = input.char_indices().peekable();
    let mut quote = None;
    let mut word = String::new();
    while let Some((index, c)) = chars.next() {
        match (quote, c) {
            (None, c) if c.is_whitespace() => return Some((word, &input[index..])),
            (
                None,
                ';' | '|' | '&' | '<' | '>' | '(' | ')' | '*' | '?' | '[' | ']' | '~' | '#' | '{'
                | '}' | '!',
            ) => return None,
            // The script cannot be recovered statically if the outer shell
            // expands it (variables, command substitution, ANSI-C quoting).
            (None | Some('"'), '$' | '`') => return None,
            (None, '\'' | '"') => quote = Some(c),
            (Some(q), c) if q == c => quote = None,
            (None, '\\') => {
                let (_, escaped) = chars.next()?;
                if escaped != '\n' {
                    word.push(escaped);
                }
            }
            (Some('"'), '\\') => {
                let (_, escaped) = chars.next()?;
                if !matches!(escaped, '$' | '`' | '"' | '\\' | '\n') {
                    word.push('\\');
                }
                if escaped != '\n' {
                    word.push(escaped);
                }
            }
            _ => word.push(c),
        }
    }
    quote.is_none().then_some((word, ""))
}
