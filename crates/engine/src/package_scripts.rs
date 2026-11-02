//! Explicit npm script discovery and execution in a host-validated checkout.
use std::io::Read;
use std::path::Path;

use zeron_proto::{PackageScript, ProjectAction, ProjectActionIcon};

use crate::EngineError;

const MAX_PACKAGE_BYTES: u64 = 1024 * 1024;

pub fn read_package_scripts(checkout: &Path) -> Result<Vec<PackageScript>, EngineError> {
    let file = match crate::project_actions::open_project_file(&checkout.join("package.json")) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_PACKAGE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PACKAGE_BYTES {
        return Err(EngineError::Other("package.json exceeds 1 MiB".into()));
    }
    let package: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|err| EngineError::Other(format!("Invalid package.json: {err}")))?;
    let Some(scripts) = package
        .get("scripts")
        .and_then(serde_json::Value::as_object)
    else {
        return Ok(Vec::new());
    };
    let mut scripts: Vec<_> = scripts
        .iter()
        .filter_map(|(name, value)| {
            let command = value.as_str()?;
            if name.is_empty() || name.chars().any(char::is_control) || command.trim().is_empty() {
                return None;
            }
            Some(PackageScript {
                name: name.clone(),
                command: command.to_owned(),
            })
        })
        .collect();
    scripts.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(scripts)
}

/// Resolve the name again at click time; never accept a shell command from the viewer.
pub fn package_script_action(checkout: &Path, name: &str) -> Result<ProjectAction, EngineError> {
    let script = read_package_scripts(checkout)?
        .into_iter()
        .find(|script| script.name == name)
        .ok_or_else(|| EngineError::Other("Package script no longer exists".into()))?;
    #[cfg(not(windows))]
    let argument = format!("'{}'", script.name.replace('\'', "'\"'\"'"));
    #[cfg(windows)]
    let argument = {
        // Both cmd and PowerShell accept this common, literal argument subset.
        if !script
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_:./@".contains(c))
        {
            return Err(EngineError::Other(
                "This script name cannot be run safely in a Windows shell".into(),
            ));
        }
        format!("\"{}\"", script.name)
    };
    Ok(ProjectAction {
        id: format!("npm:{}", script.name),
        name: format!("npm run {}", script.name),
        command: format!("npm run -- {argument}"),
        icon: ProjectActionIcon::Play,
        run_on_worktree_create: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_only_runnable_scripts_from_the_exact_checkout() {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("worktree");
        std::fs::create_dir(&checkout).unwrap();
        std::fs::write(
            root.path().join("package.json"),
            r#"{"scripts":{"root":"echo root"}}"#,
        )
        .unwrap();
        assert!(read_package_scripts(&checkout).unwrap().is_empty());
        std::fs::write(checkout.join("package.json"), r#"{"scripts":{"test":"echo test","dev":"vite","bad":42,"empty":"  ","":"no","line\nfeed":"no"}}"#).unwrap();
        let scripts = read_package_scripts(&checkout).unwrap();
        assert_eq!(
            scripts
                .iter()
                .map(|script| script.name.as_str())
                .collect::<Vec<_>>(),
            ["dev", "test"]
        );
        assert_eq!(scripts[0].command, "vite");
        let action = package_script_action(&checkout, "dev").unwrap();
        assert!(action.command.starts_with("npm run -- "));
        assert!(!action.run_on_worktree_create);
        std::fs::write(checkout.join("package.json"), "{}").unwrap();
        assert!(package_script_action(&checkout, "dev").is_err());
    }

    #[test]
    fn malformed_and_oversized_packages_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        for contents in [
            b"invalid".to_vec(),
            vec![b' '; MAX_PACKAGE_BYTES as usize + 1],
        ] {
            std::fs::write(root.path().join("package.json"), contents).unwrap();
            assert!(read_package_scripts(root.path()).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn script_names_are_literal_shell_arguments() {
        let root = tempfile::tempdir().unwrap();
        let name = "dev'$(touch injected); echo oops";
        std::fs::write(
            root.path().join("package.json"),
            serde_json::json!({"scripts": {name: "echo ok"}}).to_string(),
        )
        .unwrap();
        let action = package_script_action(root.path(), name).unwrap();
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "npm() {{ printf '%s\\n' \"$@\"; }}; {}",
                action.command
            ))
            .current_dir(root.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("run\n--\n{name}\n")
        );
        assert!(!root.path().join("injected").exists());
    }
}
