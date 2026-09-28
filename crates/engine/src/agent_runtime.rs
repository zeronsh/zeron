//! The host-side pieces every chat run needs to orchestrate through the
//! `zeron` CLI: a `zeron` shim on the child's PATH (this same binary — the
//! CLI is one binary with the engine), the staged `zeron-cli` skill bundle,
//! and the skill listing for harnesses that can only read skills as text.
//!
//! [`prepare`] runs once at engine assembly, off the async hot path, and
//! never fails startup: every step degrades to a `warn!` so a read-only or
//! exotic data dir costs the agent its CLI, not the user their engine.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use zeron_proto::AgentSkill;

/// Per-engine injection material, produced once by [`prepare`] and stamped
/// onto every chat run's [`RunRequest::agent`].
#[derive(Debug)]
pub struct AgentRuntime {
    /// What `ZERON_CLI` advertises: the shim on Unix, the exe itself on Windows.
    pub cli_path: PathBuf,
    /// Directory prepended to the child's PATH (holds the shim on Unix).
    pub cli_dir: PathBuf,
    /// Root of the staged skill bundle (`None` when staging failed).
    pub skill_bundle: Option<PathBuf>,
    /// Skills inside the bundle, with absolute SKILL.md paths.
    pub skills: Vec<AgentSkill>,
}

/// Build the runtime for `data_dir`: install/refresh the `zeron` shim and
/// stage the skill bundle content-addressed under `runtime/skills/`.
pub fn prepare(data_dir: &Path) -> AgentRuntime {
    let (cli_path, cli_dir) = shim(data_dir);
    let skill_bundle = stage_skills(data_dir);
    let skills = skill_bundle
        .as_ref()
        .map(|bundle| {
            zeron_guide::skills()
                .iter()
                .map(|skill| AgentSkill {
                    name: skill.name.to_owned(),
                    description: skill.description.to_owned(),
                    path: bundle.join(skill.path).to_string_lossy().into_owned(),
                })
                .collect()
        })
        .unwrap_or_default();
    AgentRuntime {
        cli_path,
        cli_dir,
        skill_bundle,
        skills,
    }
}

/// The extra paragraph an agent-spawned child's instructions carry: its final
/// reply IS the report (the settle notification delivers it to the parent),
/// so `zeron chat tell` is reserved for blockers.
fn child_report_instructions(parent_chat_id: &str) -> String {
    format!(
        "You are a child chat spawned by @chat:{parent_chat_id}. When your work is done, end your turn with your final report as your reply: Zeron delivers it to your parent automatically. Do not use `zeron chat tell` to report progress or results to your parent; use it only when you are blocked and need a decision from your parent before you can continue."
    )
}

/// The `AgentContext` stamped on a chat run; `None` without a serving port —
/// a context that points at nothing would send the agent's `zeron` calls
/// into the void. `parent_chat_id` is set for agent-spawned children, whose
/// instructions then name the parent and say the final reply is the report.
pub fn context(
    runtime: &AgentRuntime,
    port: u16,
    device_id: &str,
    chat_id: &str,
    parent_chat_id: Option<&str>,
) -> zeron_proto::AgentContext {
    let env = [
        ("ZERON_CHAT_ID".to_owned(), chat_id.to_owned()),
        ("ZERON_DEVICE_ID".to_owned(), device_id.to_owned()),
        ("ZERON_IPC_PORT".to_owned(), port.to_string()),
        (
            "ZERON_CLI".to_owned(),
            runtime.cli_path.to_string_lossy().into_owned(),
        ),
    ]
    .into_iter()
    .collect();
    // The child paragraph rides between the base instructions and the skills
    // listing (appended later by `instructions_with_skills`).
    let instructions = match parent_chat_id.filter(|parent| !parent.is_empty()) {
        Some(parent) => format!(
            "{}\n\n{}",
            zeron_guide::instructions(),
            child_report_instructions(parent)
        ),
        None => zeron_guide::instructions().to_owned(),
    };
    zeron_proto::AgentContext {
        env,
        cli_dir: runtime.cli_dir.to_string_lossy().into_owned(),
        instructions,
        skill_bundle: runtime
            .skill_bundle
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        skills: runtime.skills.clone(),
    }
}

/// (cli_path, cli_dir) — a `zeron` symlink to this binary under
/// `{data_dir}/bin`, atomically replaced. On Windows there is no symlink,
/// so the exe's own directory is the PATH entry and the exe is the CLI.
/// Any failure degrades to pointing straight at the running exe.
fn shim(data_dir: &Path) -> (PathBuf, PathBuf) {
    match install_shim(data_dir) {
        Ok(pair) => pair,
        Err(error) => {
            tracing::warn!(%error, "could not stage the zeron CLI shim; agents get the bare exe");
            let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("zeron"));
            let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
            (exe, dir)
        }
    }
}

#[cfg(unix)]
fn install_shim(data_dir: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    let bin = data_dir.join("bin");
    std::fs::create_dir_all(&bin)?;
    let exe = std::env::current_exe()?;
    let link = bin.join("zeron");
    // Skip the rewrite when the shim already resolves to this binary —
    // restarts of the same build would otherwise churn the link pointlessly.
    if std::fs::read_link(&link).ok().as_deref() == Some(exe.as_path()) {
        return Ok((link, bin));
    }
    let tmp = bin.join(format!(".zeron.tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(&exe, &tmp)?;
    std::fs::rename(&tmp, &link)?;
    Ok((link, bin))
}

#[cfg(windows)]
fn install_shim(data_dir: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    let _ = data_dir;
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    Ok((exe, dir))
}

/// Content hash of the compiled-in bundle: first 16 hex chars of sha256 over
/// every `path\0contents\0` in order. Stable across restarts, so the staged
/// dir is reused bit-for-bit.
fn bundle_hash() -> String {
    let mut hash = Sha256::new();
    for file in zeron_guide::bundle_files() {
        hash.update(file.path.as_bytes());
        hash.update([0]);
        hash.update(file.contents.as_bytes());
        hash.update([0]);
    }
    format!("{:x}", hash.finalize())[..16].to_owned()
}

/// Stage `zeron_guide::bundle_files()` into `runtime/skills/<hash>` — written
/// into a sibling tmp dir then renamed so a concurrent or crashed stage never
/// leaves a half-written bundle behind. Afterwards removes stale tmp dirs
/// and superseded hash dirs (best effort).
fn stage_skills(data_dir: &Path) -> Option<PathBuf> {
    let root = data_dir.join("runtime").join("skills");
    let hash = bundle_hash();
    let bundle = root.join(&hash);
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(&root)?;
        if !bundle.is_dir() {
            let tmp = root.join(format!(".tmp-{hash}-{}", std::process::id()));
            for file in zeron_guide::bundle_files() {
                let path = tmp.join(file.path);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, file.contents)?;
            }
            std::fs::rename(&tmp, &bundle)?;
        }
        // GC: every other entry — an older content hash or a tmp dir orphaned
        // by a crash — is dead weight.
        for entry in std::fs::read_dir(&root)?.flatten() {
            if entry.file_name().to_string_lossy() != hash.as_str() {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => Some(bundle),
        Err(error) => {
            tracing::warn!(%error, "could not stage the zeron skill bundle");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shim_is_created_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = prepare(dir.path());
        #[cfg(unix)]
        {
            let link = dir.path().join("bin").join("zeron");
            assert_eq!(runtime.cli_path, link);
            assert_eq!(
                std::fs::read_link(&link).unwrap(),
                std::env::current_exe().unwrap()
            );
        }
        let second = prepare(dir.path());
        assert_eq!(second.cli_path, runtime.cli_path);
        assert_eq!(second.cli_dir, runtime.cli_dir);
    }

    #[test]
    fn bundle_is_staged_with_every_compiled_in_file() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = prepare(dir.path());
        let bundle = runtime.skill_bundle.expect("bundle staged");
        for file in zeron_guide::bundle_files() {
            let staged = std::fs::read_to_string(bundle.join(file.path)).unwrap();
            assert_eq!(staged, file.contents, "{}", file.path);
        }
        assert_eq!(runtime.skills.len(), zeron_guide::skills().len());
        for skill in &runtime.skills {
            assert!(Path::new(&skill.path).is_absolute());
            assert!(Path::new(&skill.path).is_file());
        }
    }

    #[test]
    fn staging_is_stable_and_gcs_superseded_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let first = prepare(dir.path());
        let root = dir.path().join("runtime").join("skills");
        // A stale hash dir and an orphaned tmp dir both die on re-stage.
        std::fs::create_dir_all(root.join("deadbeefdeadbeef")).unwrap();
        std::fs::create_dir_all(root.join(".tmp-stale-1")).unwrap();
        let second = prepare(dir.path());
        assert_eq!(first.skill_bundle, second.skill_bundle);
        let entries: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn unreadable_data_dir_degrades_without_panicking() {
        // A FILE where runtime/skills must be a directory: staging fails,
        // the runtime still carries a usable CLI.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("runtime")).unwrap();
        std::fs::write(dir.path().join("runtime").join("skills"), "block").unwrap();
        let runtime = prepare(dir.path());
        assert!(runtime.skill_bundle.is_none());
        assert!(runtime.skills.is_empty());
        assert!(runtime.cli_path.is_absolute());
    }

    #[test]
    fn context_carries_the_chat_env_and_instructions() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = prepare(dir.path());
        let ctx = context(&runtime, 27702, "dev-1", "chat-9", None);
        assert_eq!(ctx.env["ZERON_CHAT_ID"], "chat-9");
        assert_eq!(ctx.env["ZERON_DEVICE_ID"], "dev-1");
        assert_eq!(ctx.env["ZERON_IPC_PORT"], "27702");
        assert_eq!(
            ctx.env["ZERON_CLI"],
            runtime.cli_path.to_string_lossy().as_ref()
        );
        assert_eq!(ctx.instructions, zeron_guide::instructions());
        assert!(ctx.prompt_prefix().starts_with("<system_instructions>"));
    }

    #[test]
    fn child_context_names_the_parent_between_the_base_and_skills_text() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = prepare(dir.path());
        let parent = "3f6b2a18-9c4d-4e5f-8a7b-1c2d3e4f5a6b";
        let ctx = context(&runtime, 27702, "dev-1", "chat-9", Some(parent));
        assert!(
            ctx.instructions.starts_with(zeron_guide::instructions()),
            "the base instructions stay verbatim at the front"
        );
        assert!(
            ctx.instructions
                .contains(&format!("You are a child chat spawned by @chat:{parent}."))
        );
        assert!(
            ctx.instructions.contains(
                "Do not use `zeron chat tell` to report progress or results to your parent"
            )
        );
        let with_skills = ctx.instructions_with_skills();
        let child_at = with_skills.find("You are a child chat spawned by").unwrap();
        let skills_at = with_skills.find("Skills available in this chat").unwrap();
        assert!(child_at < skills_at, "the skills paragraph comes last");
    }

    #[test]
    fn top_level_context_keeps_the_plain_instructions() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = prepare(dir.path());
        for none in [None, Some("")] {
            let ctx = context(&runtime, 27702, "dev-1", "chat-9", none);
            assert_eq!(ctx.instructions, zeron_guide::instructions());
        }
    }
}
