use zeron_proto::{
    TodoItem, TodoStatus, ToolCall,
    view::{display_command, tool_call_text, tool_chip_content},
};

#[test]
fn command_display_hides_powershell_executable_from_screenshot() {
    let script =
        "Get-Content -LiteralPath 'src/main.rs'; Get-Content -LiteralPath 'tests/main.rs' -Tail 25";
    let command = format!(
        "\"C:\\WINDOWS\\System32\\WindowsPowerShell\\v1.0\\powershell.exe\" -Command \"{script}\""
    );
    let call = ToolCall::Exec {
        command: command.clone(),
    };
    assert_eq!(tool_chip_content(&call), ("Run", script.to_owned()));
    assert_eq!(tool_call_text(&call), command);
}

#[test]
fn expanded_commands_preserve_the_exact_invocation_for_auditing() {
    for command in [
        "./bash -c 'ls -la'",
        "/tmp/evil/sh -c 'ls -la'",
        "~/bin/zsh -c 'ls'",
        r"C:\Users\x\Downloads\bash.exe -c 'ls'",
        r#".\powershell.exe -Command "Get-ChildItem""#,
        "/bin/bash -lc 'git status'",
        "pwsh -NoProfile -Command \"Get-Date\"",
        "  bash -c 'echo one\n\techo two'  \n",
    ] {
        let call = ToolCall::Exec {
            command: command.into(),
        };
        assert_eq!(tool_call_text(&call), command, "{command:?}");
    }
}

#[test]
fn expanded_todo_invocations_preserve_all_status_markers() {
    let call = ToolCall::Todo {
        items: vec![
            TodoItem::new("finished", TodoStatus::Completed),
            TodoItem::new("working", TodoStatus::InProgress),
            TodoItem::new("next", TodoStatus::Pending),
        ],
    };
    assert_eq!(tool_call_text(&call), "[x] finished\n[~] working\n[ ] next");
}

fn assert_display(command: &str, script: &str) {
    assert_eq!(display_command(command), script, "{command:?}");
}

#[test]
fn command_display_recognizes_powershell_paths_options_and_case() {
    for executable in [
        "powershell",
        "powershell.exe",
        "pwsh",
        "pwsh.exe",
        "PoWeRsHeLl.ExE",
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        r#""C:\Program Files\PowerShell\7\pwsh.exe""#,
        "'/opt/microsoft/powershell/7/pwsh'",
        r#"& "C:\Program Files\PowerShell\7\pwsh.exe""#,
        r#"&'C:\Program Files\PowerShell\7\pwsh.exe'"#,
    ] {
        for options in [
            "",
            "-NoLogo -NoProfile -NonInteractive ",
            "-ExecutionPolicy Bypass -WindowStyle Hidden -InputFormat Text -OutputFormat Text ",
        ] {
            for flag in ["-Command", "-c", "-COMMAND"] {
                assert_display(
                    &format!(
                        "  {executable} {options}{flag} \"Get-Content 'file with spaces.rs'\"  "
                    ),
                    "Get-Content 'file with spaces.rs'",
                );
            }
        }
    }
}

#[test]
fn command_display_preserves_windows_script_syntax() {
    for script in [
        r"Get-Content 'C:\work\src\main.rs' | Select-Object -First 5; Write-Host $env:PATH",
        "Get-Content '路径/ação.rs'\n\tWrite-Output '😀'",
        "Write-Output `\"hello`\"",
        "Write-Output \"\"hello\"\"",
        r"Write-Output 'C:\work\'",
    ] {
        assert_display(&format!("pwsh -Command \"{script}\""), script);
        assert_display(&format!("powershell -Command {script}"), script);
    }
    assert_display("pwsh -c 'Get-ChildItem -Force'", "Get-ChildItem -Force");
    // Do not remove a command's own quotes when it is not wholly enclosed.
    assert_display(
        "pwsh -c & 'C:\\Program Files\\tool.exe' --flag",
        "& 'C:\\Program Files\\tool.exe' --flag",
    );
}

#[test]
fn command_display_recognizes_unix_shell_wrappers() {
    for executable in [
        "/bin/sh",
        "/usr/bin/bash",
        "/usr/bin/zsh",
        "dash",
        "ksh",
        "fish",
        "bash.exe",
        r#""C:\Program Files\Git\bin\bash.exe""#,
        "'/path with spaces/bash'",
    ] {
        for options in [
            "-c",
            "-lc",
            "-cl",
            "-lic",
            "-l -c",
            "--login -c",
            "--noprofile --norc -c",
        ] {
            assert_display(
                &format!("{executable} {options} 'cargo test && echo \"done\"'"),
                "cargo test && echo \"done\"",
            );
        }
    }
    assert_display("bash -c pwd", "pwd");
}

#[test]
fn command_display_decodes_only_posix_argument_quoting() {
    for (command, script) in [
        (
            r"bash -lc 'printf '\''%s\n'\'' '\''hello world'\'''",
            r"printf '%s\n' 'hello world'",
        ),
        (
            r#"bash -c "echo \"quoted\"; echo \$HOME; printf '%s\n'""#,
            r#"echo "quoted"; echo $HOME; printf '%s\n'"#,
        ),
        (r"bash -c echo\ hello", "echo hello"),
        (
            "zsh -lc 'echo 路径\n\tprintf \"😀\"'",
            "echo 路径\n\tprintf \"😀\"",
        ),
        (
            r"sh -c 'echo $HOME; echo $(pwd); echo `pwd`'",
            r"echo $HOME; echo $(pwd); echo `pwd`",
        ),
        ("sh -c 'echo '\"hello\"", "echo hello"),
    ] {
        assert_display(command, script);
    }
}

#[test]
fn command_display_recognizes_cmd_without_corrupting_paths_or_escapes() {
    for executable in [
        "cmd",
        "CMD.EXE",
        r"C:\Windows\System32\cmd.exe",
        r#""C:\Windows\System32\cmd.exe""#,
    ] {
        assert_display(
            &format!("{executable} /D /Q /V:OFF /E:ON /C dir /b && echo ready"),
            "dir /b && echo ready",
        );
        assert_display(
            &format!("{executable} /d /s /c \"dir /b && echo ready\""),
            "dir /b && echo ready",
        );
    }
    assert_display(
        r#"cmd /d /s /c ""C:\Program Files\tool.exe" "hello world"""#,
        r#""C:\Program Files\tool.exe" "hello world""#,
    );
    assert_display(
        r#"cmd /c ""C:\Program Files\tool.exe" "hello world"""#,
        r#""C:\Program Files\tool.exe" "hello world""#,
    );
    assert_display(
        r#"cmd /c "C:\Program Files\tool.exe""#,
        r#""C:\Program Files\tool.exe""#,
    );
    assert_display(
        r"cmd /c echo ^& ^| %PATH% !VALUE!",
        r"echo ^& ^| %PATH% !VALUE!",
    );
}

#[test]
fn command_display_preserves_unrecognized_incomplete_and_argument_bearing_invocations() {
    for command in [
        "",
        " \n\t ",
        "cargo test",
        "Get-Content 'main.rs'",
        "set -e\ncargo test",
        "echo powershell -Command hello",
        "python -c 'print(1)'",
        "node -e 'console.log(1)'",
        "notbash -c 'pwd'",
        "bash-wrapper -c 'pwd'",
        "./Bash -c 'pwd'",
        "powershell",
        "pwsh -NoProfile",
        "powershell -Command",
        "powershell -Command -",
        "powershell -Command \"-\"",
        "pwsh -c \"\"",
        "pwsh -c \" \n \"",
        "pwsh -Command \"unterminated",
        "pwsh -c 'unterminated",
        "pwsh -c \"echo one\" \"two\"",
        "pwsh -File script.ps1 -Command value",
        "powershell -EncodedCommand ZQBjAGgAbwA=",
        "pwsh -CommandWithArgs 'echo $args' one two",
        "pwsh -NoExit -Command 'pwd'",
        "pwsh -Unknown -Command 'pwd'",
        "pwsh -ExecutionPolicy -Command 'pwd'",
        "bash",
        "bash -lc",
        "bash -c ''",
        "bash -c ' \n '",
        "bash -c 'unterminated",
        "bash -c 'echo x' > out.txt",
        "bash -c 'echo x' && echo y",
        "bash -c 'echo $1' zero one",
        "bash -c echo hello",
        "bash -c $'echo\\nhello'",
        "bash -c \"$SCRIPT\"",
        "bash -c \"echo $(pwd)\"",
        "bash -c `cat script`",
        "bash -c pwd\\",
        "bash -o pipefail -c 'pwd'",
        "bash -cc 'pwd'",
        "bash -c *.sh",
        "bash -c ~/script",
        "bash -c {one,two}",
        "bash -c #comment",
        "bash script.sh -c 'pwd'",
        "env FOO=bar bash -c 'pwd'",
        "cmd",
        "cmd /c",
        "cmd /s /c \"\"",
        "cmd /k dir",
        "cmd /unknown /c dir",
        "& bash -c pwd",
        "echo;/bin/bash -c pwd",
    ] {
        assert_display(command, command);
    }
}

#[test]
fn command_display_unwraps_only_the_outer_shell() {
    assert_display(
        "bash -lc 'pwsh -Command Get-Date'",
        "pwsh -Command Get-Date",
    );
    assert_display("powershell -Command \"cmd /c dir\"", "cmd /c dir");
}

#[test]
fn command_display_handles_a_long_script_without_truncation() {
    let script = "Get-Content '路径.rs';\n".repeat(2_000);
    assert_display(&format!("powershell -Command \"{script}\""), &script);
}

#[test]
fn command_display_preserves_empty_or_incomplete_cmd_quotes() {
    for command in ["cmd /s /c \"", "cmd /s /c \" \n \"", "cmd /c \"\""] {
        assert_display(command, command);
    }
}

#[test]
fn command_display_does_not_panic_on_partial_streamed_invocations() {
    for command in [
        r#""C:\路径\powershell.exe" -NoProfile -Command "Write-Output '😀'""#,
        r#"cmd /d /s /c ""C:\Program Files\工具.exe" "😀"""#,
        r"/usr/bin/zsh -lc 'printf '\''%s\n'\'' '\''😀'\'''",
    ] {
        // Providers can deliver partial command strings while a tool starts.
        for end in command
            .char_indices()
            .map(|(i, _)| i)
            .chain([command.len()])
        {
            let _ = display_command(&command[..end]);
        }
    }
}
