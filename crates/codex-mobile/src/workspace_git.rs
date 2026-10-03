//! Embedded, deliberately scoped Git. Never launches git, hooks or credential helpers.
use git2::{Repository, StatusOptions};
use serde::Deserialize;
use serde_json::json;
use std::{
    ffi::{CStr, CString, c_char},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Deserialize)]
struct Request {
    root: PathBuf,
    args: Vec<String>,
    id: String,
    available_bytes: usize,
    available_entries: usize,
}
static ACTIVE: OnceLock<Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>> =
    OnceLock::new();
fn active() -> &'static Mutex<std::collections::HashMap<String, Arc<AtomicBool>>> {
    ACTIVE.get_or_init(Default::default)
}
fn error(s: &str) -> git2::Error {
    git2::Error::from_str(s)
}
fn child(root: &Path, path: &str) -> Result<PathBuf, git2::Error> {
    let path = Path::new(path.strip_prefix("/workspace/").unwrap_or(path));
    if path == Path::new(".") || path == Path::new("/workspace") {
        return Ok(root.to_owned());
    }
    let mut out = root.to_owned();
    for piece in path.components() {
        match piece {
            Component::Normal(name) if !name.to_string_lossy().eq_ignore_ascii_case(".git") => {
                out.push(name)
            }
            _ => return Err(error("Expected a workspace path without traversal or .git")),
        }
        if out
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(error("Symbolic links are unsupported"));
        }
    }
    Ok(out)
}
fn run(request: &Request, cancel: &AtomicBool) -> Result<String, git2::Error> {
    static SETUP: std::sync::Once = std::sync::Once::new();
    SETUP.call_once(|| unsafe {
        let _ = git2::opts::set_server_connect_timeout_in_milliseconds(5_000);
        let _ = git2::opts::set_server_timeout_in_milliseconds(10_000);
    });
    let mut args = request.args.as_slice();
    let root = request
        .root
        .canonicalize()
        .map_err(|_| error("Workspace missing"))?;
    let mut cwd = root.clone();
    while args.first().map(String::as_str) == Some("-C") && args.len() >= 3 {
        cwd = child(
            if args[1].starts_with("/workspace") {
                &root
            } else {
                &cwd
            },
            &args[1],
        )?;
        args = &args[2..];
    }
    let command = args.first().map(String::as_str).unwrap_or("help");
    let params = &args[usize::from(!args.is_empty())..];
    let help = "Native Git: init; status; diff [--cached]; add PATH...; commit -m MESSAGE; log; branch [NAME]; config user.name VALUE; config user.email VALUE; clone HTTPS_URL DEST. Use git -C /workspace/project COMMAND. Public HTTPS clone only; authentication, fetch/push, submodules, hooks and executables are unavailable.\n";
    if matches!(command, "help" | "--help") {
        return Ok(help.into());
    }
    if command == "init" && params.is_empty() {
        let repo = Repository::init(&cwd)?;
        repo.config()?.set_bool("core.symlinks", false)?;
        return Ok("Initialized repository\n".into());
    }
    if command == "clone" && params.len() == 2 {
        let parsed = url::Url::parse(&params[0]).map_err(|_| error("Invalid HTTPS URL"))?;
        if parsed.scheme() != "https"
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.host_str().is_none()
        {
            return Err(error(
                "Clone requires a public HTTPS URL without credentials",
            ));
        }
        let destination = child(
            if params[1].starts_with("/workspace") {
                &root
            } else {
                &cwd
            },
            &params[1],
        )?;
        if destination.exists() {
            return Err(error("Clone destination already exists"));
        }
        if !destination.parent().is_some_and(Path::is_dir) {
            return Err(error(
                "Create the destination parent directory before cloning",
            ));
        }
        let start = Instant::now();
        let mut callbacks = git2::RemoteCallbacks::new();
        callbacks.transfer_progress(|stats| {
            !cancel.load(Ordering::Relaxed)
                && start.elapsed() < Duration::from_secs(30)
                && stats.received_bytes() < 64 * 1024 * 1024
        });
        let mut fetch = git2::FetchOptions::new();
        fetch.remote_callbacks(callbacks);
        fetch.follow_redirects(git2::RemoteRedirect::None);
        // Clone without checkout: reject oversized trees and symlinks before any working files are created.
        let mut checkout = git2::build::CheckoutBuilder::new();
        checkout.dry_run();
        let result = (|| {
            let repo = git2::build::RepoBuilder::new()
                .fetch_options(fetch)
                .with_checkout(checkout)
                .clone(&params[0], &destination)?;
            let tree = repo.head()?.peel_to_tree()?;
            let mut bytes = 0usize;
            let mut count = 0usize;
            let mut invalid = false;
            tree.walk(git2::TreeWalkMode::PreOrder, |_, entry| {
                count += 1;
                if entry.filemode() == 0o120000
                    || entry.filemode() == 0o160000
                    || entry
                        .name()
                        .is_some_and(|name| name.eq_ignore_ascii_case(".git"))
                {
                    invalid = true;
                    return git2::TreeWalkResult::Abort;
                }
                if entry.kind() == Some(git2::ObjectType::Blob) {
                    match repo.find_blob(entry.id()) {
                        Ok(blob) => {
                            bytes += blob.size();
                            if blob.size() > 16 * 1024 * 1024 {
                                invalid = true;
                            }
                        }
                        Err(_) => invalid = true,
                    }
                }
                if invalid
                    || count + 1 > request.available_entries
                    || bytes > request.available_bytes
                    || cancel.load(Ordering::Relaxed)
                    || start.elapsed() > Duration::from_secs(30)
                {
                    invalid = true;
                    git2::TreeWalkResult::Abort
                } else {
                    git2::TreeWalkResult::Ok
                }
            })?;
            if invalid {
                return Err(error(
                    "Clone cancelled or exceeds limits (128 MB, 20,000 entries, 16 MB/file; no symlinks/submodules)",
                ));
            }
            repo.config()?.set_bool("core.symlinks", false)?;
            repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))?;
            Ok("Cloned public repository\n".into())
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(destination);
        }
        return result;
    }
    // Do not discover parent repositories, honor .git redirections or load external config.
    if !cwd
        .join(".git")
        .symlink_metadata()
        .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
    {
        return Err(error(
            "No repository here. Run git init, or git -C /workspace/project ...",
        ));
    }
    let repo = Repository::open_ext(
        &cwd,
        git2::RepositoryOpenFlags::NO_SEARCH,
        std::iter::empty::<&Path>(),
    )?;
    let output = match command {
        "status" if params.is_empty() || params == ["--short"] => {
            let mut options = StatusOptions::new();
            options.include_untracked(true).recurse_untracked_dirs(true);
            let mut text = String::new();
            for entry in repo.statuses(Some(&mut options))?.iter() {
                text.push_str(&format!(
                    "{:?} {}\n",
                    entry.status(),
                    entry.path().unwrap_or("<non-UTF8>")
                ));
                if text.len() > 240_000 {
                    text.push_str("[truncated]\n");
                    break;
                }
            }
            if text.is_empty() {
                "Working tree clean\n".into()
            } else {
                text
            }
        }
        "config"
            if params.len() == 2 && matches!(params[0].as_str(), "user.name" | "user.email") =>
        {
            repo.config()?.set_str(&params[0], &params[1])?;
            "Saved repository identity\n".into()
        }
        "add" if !params.is_empty() => {
            let mut paths = Vec::new();
            for name in params {
                let absolute = child(
                    if name.starts_with("/workspace") {
                        &root
                    } else {
                        &cwd
                    },
                    name,
                )?;
                let relative = absolute
                    .strip_prefix(&cwd)
                    .map_err(|_| error("Git paths must belong to this repository"))?;
                paths.push(if relative.as_os_str().is_empty() {
                    ".".into()
                } else {
                    relative.to_string_lossy().into_owned()
                });
            }
            let mut index = repo.index()?;
            index.add_all(&paths, git2::IndexAddOption::DEFAULT, None)?;
            index.update_all(&paths, None)?;
            index.write()?;
            "Staged files\n".into()
        }
        "commit" if params.len() == 2 && params[0] == "-m" => {
            let signature = repo.signature().map_err(|_| {
                error("Set git config user.name NAME and git config user.email EMAIL first")
            })?;
            let mut index = repo.index()?;
            let tree = repo.find_tree(index.write_tree()?)?;
            let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
            if parent.as_ref().is_some_and(|p| p.tree_id() == tree.id()) {
                return Err(error("Nothing staged to commit"));
            }
            let parents: Vec<_> = parent.iter().collect();
            let id = repo.commit(
                Some("HEAD"),
                &signature,
                &signature,
                &params[1],
                &tree,
                &parents,
            )?;
            format!("Committed {id}\n")
        }
        "log" if params.is_empty() => {
            let mut walk = repo.revwalk()?;
            walk.push_head()?;
            let mut text = String::new();
            for id in walk.take(30) {
                let c = repo.find_commit(id?)?;
                text.push_str(&format!("{} {}\n", c.id(), c.summary().unwrap_or("")));
            }
            text
        }
        "diff" if params.is_empty() || params == ["--cached"] => {
            let tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
            let diff = if params.is_empty() {
                repo.diff_index_to_workdir(None, None)?
            } else {
                repo.diff_tree_to_index(tree.as_ref(), None, None)?
            };
            let mut text = String::new();
            let mut truncated = false;
            diff.print(git2::DiffFormat::Patch, |_, _, line| {
                if text.len() >= 240_000 {
                    truncated = true;
                    return true;
                }
                if matches!(line.origin(), '+' | '-' | ' ') {
                    text.push(line.origin());
                }
                text.push_str(&String::from_utf8_lossy(line.content()));
                true
            })?;
            if truncated {
                text.push_str("\n[truncated]\n");
            }
            text
        }
        "branch" if params.len() <= 1 => {
            if let Some(name) = params.first() {
                repo.branch(name, &repo.head()?.peel_to_commit()?, false)?;
                format!("Created branch {name}\n")
            } else {
                let mut text = String::new();
                for branch in repo.branches(Some(git2::BranchType::Local))? {
                    let (b, _) = branch?;
                    text.push_str(&format!(
                        "{} {}\n",
                        if b.is_head() { "*" } else { " " },
                        b.name()?.unwrap_or("<non-UTF8>")
                    ));
                }
                text
            }
        }
        // Branch switching is omitted until staged checkout validation also covers historical trees.
        _ => return Err(error(help)),
    };
    Ok(output)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn zeron_git_run(input: *const c_char) -> *mut c_char {
    let result = std::panic::catch_unwind(|| -> Result<String, String> {
        if input.is_null() {
            return Err("Missing Git request".into());
        }
        let request: Request = serde_json::from_slice(unsafe { CStr::from_ptr(input) }.to_bytes())
            .map_err(|e| e.to_string())?;
        let cancel = Arc::new(AtomicBool::new(false));
        active()
            .lock()
            .unwrap()
            .insert(request.id.clone(), cancel.clone());
        let result = run(&request, &cancel)
            .map(|text| {
                if text.len() > 256_000 {
                    text.chars().take(240_000).collect::<String>() + "\n[truncated]\n"
                } else {
                    text
                }
            })
            .map_err(|e| e.message().to_owned());
        active().lock().unwrap().remove(&request.id);
        result
    })
    .unwrap_or_else(|_| Err("Git failed".into()));
    let output = match result {
        Ok(stdout) => json!({"stdout":stdout,"stderr":"","exitCode":0}),
        Err(stderr) => json!({"stdout":"","stderr":stderr,"exitCode":1}),
    };
    CString::new(output.to_string()).unwrap().into_raw()
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zeron_git_cancel(id: *const c_char) {
    if id.is_null() {
        return;
    }
    let id = unsafe { CStr::from_ptr(id) }.to_string_lossy();
    if let Some(cancel) = active().lock().unwrap().get(id.as_ref()) {
        cancel.store(true, Ordering::Relaxed);
    }
}
