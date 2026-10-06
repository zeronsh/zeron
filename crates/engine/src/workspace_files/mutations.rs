//! Structural mutations execute on the workspace host, serialized against saves.
use super::*;
use zeron_proto::{
    MoveWorkspaceEntryRequest, WorkspaceMutationOutcome, WorkspaceMutationRejection as Reason,
};

type MutationResult<T> = Result<T, (Reason, String)>;

impl WorkspaceFiles {
    pub async fn delete_entry(
        &self,
        request: zeron_proto::DeleteWorkspaceEntryRequest,
    ) -> Result<WorkspaceMutationOutcome, WorkspaceFilesError> {
        let workspace = self.resolve_target(&request.target).await?;
        let gate = self
            .mutation_gate(&workspace.checkout_id)
            .write_owned()
            .await;
        if request.expected_checkout_id.is_empty()
            || self.resolve_target(&request.target).await? != workspace
            || request.expected_checkout_id != workspace.checkout_id
        {
            return Ok(rejected(
                request.operation_id,
                Reason::WorkspaceChanged,
                "Workspace changed; refresh before deleting",
            ));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_on_drop = CancelOnDrop::new(cancel.clone());
        let owner = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _gate = gate;
            let result = match delete_blocking(&workspace, &request, &cancel) {
                Ok(()) => WorkspaceMutationOutcome::Applied {
                    operation_id: request.operation_id.clone(),
                    checkout_id: workspace.checkout_id.clone(),
                    change: WorkspaceFileChange {
                        operation_id: Some(request.operation_id),
                        kind: WorkspaceFileChangeKind::Removed,
                        path: request.path,
                        old_path: None,
                    },
                    entry: None,
                },
                Err((reason, message)) => rejected(request.operation_id, reason, &message),
            };
            owner.publish_mutation(&workspace.checkout_id, &result);
            result
        })
        .await
        .map_err(|e| WorkspaceFilesError::Io(format!("delete worker failed: {e}")))?;
        cancel_on_drop.disarm();
        Ok(result)
    }

    fn publish_mutation(&self, checkout: &str, result: &WorkspaceMutationOutcome) {
        if let Some(watch) = lock(&self.inner.watches).get(checkout).cloned() {
            match result {
                WorkspaceMutationOutcome::Applied { change, .. } => {
                    watch.publish(false, vec![change.clone()])
                }
                WorkspaceMutationOutcome::Rejected {
                    reason: Reason::PartialFailure,
                    ..
                } => watch.publish(true, vec![]),
                _ => {}
            }
        }
    }

    pub(super) fn mutation_gate(&self, checkout: &str) -> Arc<tokio::sync::RwLock<()>> {
        let mut gates = lock(&self.inner.mutation_gates);
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(checkout).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(tokio::sync::RwLock::new(()));
        gates.insert(checkout.into(), Arc::downgrade(&gate));
        gate
    }

    pub async fn move_entry(
        &self,
        request: MoveWorkspaceEntryRequest,
    ) -> Result<WorkspaceMutationOutcome, WorkspaceFilesError> {
        let workspace = self.resolve_target(&request.target).await?;
        let gate = self
            .mutation_gate(&workspace.checkout_id)
            .write_owned()
            .await;
        let current = self.resolve_target(&request.target).await?;
        if request.expected_checkout_id.is_empty()
            || current != workspace
            || request.expected_checkout_id != workspace.checkout_id
        {
            return Ok(rejected(
                request.operation_id,
                Reason::WorkspaceChanged,
                "Workspace changed; refresh before moving",
            ));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_on_drop = CancelOnDrop::new(cancel.clone());
        let owner = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _gate = gate;
            let result = match move_blocking(&workspace, &request, &cancel) {
                Ok(entry) => WorkspaceMutationOutcome::Applied {
                    operation_id: request.operation_id.clone(),
                    checkout_id: workspace.checkout_id.clone(),
                    change: WorkspaceFileChange {
                        operation_id: Some(request.operation_id),
                        kind: WorkspaceFileChangeKind::Renamed,
                        path: entry.path.clone(),
                        old_path: Some(request.source_path),
                    },
                    entry: Some(entry),
                },
                Err((reason, message)) => rejected(request.operation_id, reason, &message),
            };
            owner.publish_mutation(&workspace.checkout_id, &result);
            result
        })
        .await
        .map_err(|e| WorkspaceFilesError::Io(format!("move worker failed: {e}")))?;
        cancel_on_drop.disarm();
        Ok(result)
    }
}

fn delete_blocking(
    workspace: &ResolvedWorkspace,
    request: &zeron_proto::DeleteWorkspaceEntryRequest,
    cancel: &AtomicBool,
) -> MutationResult<()> {
    if request.operation_id.is_empty() || request.operation_id.len() > 128 {
        return Err((Reason::InvalidPath, "Invalid operation identity".into()));
    }
    let (relative, metadata) = source(
        &workspace.root,
        &request.path,
        &request.expected_source_revision,
        request.expected_kind,
    )?;
    if metadata.is_dir() && !request.recursive {
        return Err((
            Reason::InvalidDestination,
            "Deleting a folder requires recursive confirmation".into(),
        ));
    }
    if cancel.load(Ordering::Acquire) {
        return Err((Reason::Busy, "Delete cancelled before execution".into()));
    }
    let target = workspace.root.join(relative.as_path());
    if metadata.is_dir() {
        // std::fs::remove_dir_all does not follow symbolic links and uses platform
        // handle-relative traversal. A failed recursive delete may have removed children.
        std::fs::remove_dir_all(target).map_err(|e| {
            (
                Reason::PartialFailure,
                format!("Folder deletion did not complete; refresh to see remaining files: {e}"),
            )
        })
    } else {
        std::fs::remove_file(target).map_err(io_error)
    }
}

fn rejected(operation_id: String, reason: Reason, message: &str) -> WorkspaceMutationOutcome {
    WorkspaceMutationOutcome::Rejected {
        operation_id,
        reason,
        message: message.into(),
    }
}

fn domain_error(error: WorkspaceFilesError) -> (Reason, String) {
    let reason = match &error {
        WorkspaceFilesError::BadParams(_) => Reason::InvalidPath,
        WorkspaceFilesError::Authorization(_) => Reason::WorkspaceChanged,
        WorkspaceFilesError::NotFound(_) => Reason::SourceMissing,
        WorkspaceFilesError::Unsupported(_) => Reason::Unsupported,
        WorkspaceFilesError::Io(_) => Reason::InvalidDestination,
    };
    (reason, error.to_string())
}

fn io_error(error: std::io::Error) -> (Reason, String) {
    use std::io::ErrorKind;
    (
        match error.kind() {
            ErrorKind::AlreadyExists => Reason::DestinationExists,
            ErrorKind::NotFound => Reason::SourceMissing,
            ErrorKind::PermissionDenied => Reason::PermissionDenied,
            ErrorKind::Unsupported | ErrorKind::CrossesDevices => Reason::Unsupported,
            _ => Reason::InvalidDestination,
        },
        error.to_string(),
    )
}

/// Metadata revision is deliberately cheap; directory revisions are not recursive snapshots.
pub(super) fn revision(metadata: &std::fs::Metadata) -> String {
    let mut hash = Sha256::new();
    hash.update(format!(
        "{:?}:{:?}:{:?}:{}:{}",
        metadata.file_type(),
        metadata.modified().ok(),
        metadata.created().ok(),
        metadata.len(),
        metadata.permissions().readonly()
    ));
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hash.update(format!(
            ":{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec()
        ));
    }
    hex(&hash.finalize())
}

pub(super) fn is_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return metadata.file_attributes() & 0x400 != 0; // FILE_ATTRIBUTE_REPARSE_POINT, includes junctions.
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn source(
    root: &Path,
    path: &str,
    expected: &str,
    kind: WorkspaceEntryKind,
) -> MutationResult<(WorkspaceRelativePath, std::fs::Metadata)> {
    let relative = WorkspaceRelativePath::file(path).map_err(domain_error)?;
    // Require canonical wire spelling so identity/deduplication is unambiguous.
    if relative.wire_path() != path {
        return Err((
            Reason::InvalidPath,
            "Path must use canonical workspace-relative spelling".into(),
        ));
    }
    let parent = WorkspaceRelativePath(relative.as_path().parent().unwrap_or(Path::new("")).into());
    checked_directory(root, &parent).map_err(domain_error)?;
    let metadata = std::fs::symlink_metadata(root.join(relative.as_path())).map_err(io_error)?;
    if is_link(&metadata) || !(metadata.is_file() || metadata.is_dir()) {
        return Err((
            Reason::Unsupported,
            "Links and special files cannot be mutated".into(),
        ));
    }
    let actual_kind = if metadata.is_dir() {
        WorkspaceEntryKind::Directory
    } else {
        WorkspaceEntryKind::File
    };
    if actual_kind != kind || expected.is_empty() || revision(&metadata) != expected {
        return Err((
            Reason::SourceChanged,
            "Entry changed; refresh before trying again".into(),
        ));
    }
    Ok((relative, metadata))
}

fn move_blocking(
    workspace: &ResolvedWorkspace,
    request: &MoveWorkspaceEntryRequest,
    cancel: &AtomicBool,
) -> MutationResult<WorkspaceEntry> {
    if request.operation_id.is_empty() || request.operation_id.len() > 128 {
        return Err((Reason::InvalidPath, "Invalid operation identity".into()));
    }
    let (source, metadata) = source(
        &workspace.root,
        &request.source_path,
        &request.expected_source_revision,
        request.expected_kind,
    )?;
    let destination =
        WorkspaceRelativePath::file(&request.destination_path).map_err(domain_error)?;
    if destination.wire_path() != request.destination_path {
        return Err((Reason::InvalidPath, "Invalid destination spelling".into()));
    }
    if destination == source
        || (metadata.is_dir() && destination.as_path().starts_with(source.as_path()))
    {
        return Err((
            Reason::InvalidDestination,
            "Cannot move an entry into itself".into(),
        ));
    }
    let parent = WorkspaceRelativePath(
        destination
            .as_path()
            .parent()
            .unwrap_or(Path::new(""))
            .into(),
    );
    checked_directory(&workspace.root, &parent).map_err(domain_error)?;
    if cancel.load(Ordering::Acquire) {
        return Err((Reason::Busy, "Move cancelled before execution".into()));
    }
    move_entry_no_replace(&workspace.root, source.as_path(), destination.as_path())?;
    let target = workspace.root.join(destination.as_path());
    let updated = std::fs::symlink_metadata(&target).unwrap_or(metadata);
    Ok(WorkspaceEntry {
        path: destination.wire_path(),
        name: target.file_name().unwrap().to_string_lossy().into_owned(),
        kind: request.expected_kind,
        size: updated.is_file().then_some(updated.len()),
        modified_at: updated.modified().ok().map(chrono::DateTime::from),
        ignored: false,
        read_only: !updated.is_file(),
        mutation_revision: Some(revision(&updated)),
    })
}

/// Case-insensitive filesystems may reject even a spelling-only rename under
/// NOREPLACE. Stage that single directory entry, never a second hard link.
fn move_entry_no_replace(root: &Path, source: &Path, destination: &Path) -> MutationResult<()> {
    match move_no_replace(root, source, destination) {
        Ok(()) => Ok(()),
        Err(error) => {
            #[cfg(unix)]
            if error.kind() == std::io::ErrorKind::AlreadyExists
                && is_case_alias(root, source, destination)
            {
                let temporary = source
                    .parent()
                    .unwrap_or(Path::new(""))
                    .join(format!(".zeron-save-{}.tmp", uuid::Uuid::new_v4()));
                move_no_replace(root, source, &temporary).map_err(io_error)?;
                if let Err(error) = move_no_replace(root, &temporary, destination) {
                    if let Err(rollback) = move_no_replace(root, &temporary, source) {
                        return Err((
                            Reason::PartialFailure,
                            format!(
                                "Rename could not complete ({error}) or restore its original name ({rollback}); recover the entry at {}",
                                temporary.display()
                            ),
                        ));
                    }
                    return Err(io_error(error));
                }
                return Ok(());
            }
            Err(io_error(error))
        }
    }
}

#[cfg(unix)]
fn is_case_alias(root: &Path, source: &Path, destination: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    if source.parent() != destination.parent() {
        return false;
    }
    let (Some(from), Some(to)) = (source.file_name(), destination.file_name()) else {
        return false;
    };
    if from == to || from.to_string_lossy().to_lowercase() != to.to_string_lossy().to_lowercase() {
        return false;
    }
    let (Ok(a), Ok(b)) = (
        std::fs::symlink_metadata(root.join(source)),
        std::fs::symlink_metadata(root.join(destination)),
    ) else {
        return false;
    };
    if a.dev() != b.dev() || a.ino() != b.ino() || is_link(&a) || is_link(&b) {
        return false;
    }
    // Distinct hard-link names are a real collision, not an alias in a
    // case-insensitive directory. A failed enumeration never authorizes a move.
    let Ok(entries) = std::fs::read_dir(root.join(source.parent().unwrap_or(Path::new("")))) else {
        return false;
    };
    for entry in entries {
        match entry {
            Ok(entry) if entry.file_name() == to => return false,
            Err(_) => return false,
            _ => {}
        }
    }
    true
}

/// Open every ancestor without following links, and anchor the native rename to those handles.
#[cfg(unix)]
fn open_parent(root: &Path, relative: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    };
    let mut directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root)?;
    for component in relative.parent().unwrap_or(Path::new("")).components() {
        let Component::Normal(name) = component else {
            return Err(std::io::ErrorKind::InvalidInput.into());
        };
        let name = std::ffi::CString::new(name.as_bytes())?;
        // SAFETY: a valid directory descriptor and NUL-terminated single path component.
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: openat returned a newly owned descriptor.
        directory = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    Ok(directory)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn move_no_replace(root: &Path, source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};
    let from = open_parent(root, source)?;
    let to = open_parent(root, destination)?;
    let source = std::ffi::CString::new(
        source
            .file_name()
            .ok_or(std::io::ErrorKind::InvalidInput)?
            .as_bytes(),
    )?;
    let destination = std::ffi::CString::new(
        destination
            .file_name()
            .ok_or(std::io::ErrorKind::InvalidInput)?
            .as_bytes(),
    )?;
    // SAFETY: live parent handles and NUL-terminated basename buffers; replacement is disabled.
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            from.as_raw_fd(),
            source.as_ptr(),
            to.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            from.as_raw_fd(),
            source.as_ptr(),
            to.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn move_no_replace(root: &Path, source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let from: Vec<u16> = root
        .join(source)
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let to: Vec<u16> = root
        .join(destination)
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: NUL-terminated paths; neither REPLACE_EXISTING nor COPY_ALLOWED is enabled.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn move_no_replace(_: &Path, _: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(root: &Path, path: &str, to: &str) -> MoveWorkspaceEntryRequest {
        let metadata = std::fs::symlink_metadata(root.join(path)).unwrap();
        MoveWorkspaceEntryRequest {
            target: WorkspaceTarget {
                chat_id: Some("chat".into()),
                space_id: None,
                checkout_path: None,
            },
            operation_id: "op".into(),
            expected_checkout_id: "checkout".into(),
            source_path: path.into(),
            destination_path: to.into(),
            expected_source_revision: revision(&metadata),
            expected_kind: if metadata.is_dir() {
                WorkspaceEntryKind::Directory
            } else {
                WorkspaceEntryKind::File
            },
        }
    }
    #[test]
    fn simultaneous_moves_never_replace_the_winner() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("one"), "one").unwrap();
        std::fs::write(root.join("two"), "two").unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let results = std::thread::scope(|scope| {
            let a = barrier.clone();
            let first = scope.spawn(move || {
                a.wait();
                move_entry_no_replace(root, Path::new("one"), Path::new("winner"))
            });
            let b = barrier.clone();
            let second = scope.spawn(move || {
                b.wait();
                move_entry_no_replace(root, Path::new("two"), Path::new("winner"))
            });
            (first.join().unwrap(), second.join().unwrap())
        });
        assert_ne!(results.0.is_ok(), results.1.is_ok());
        let winner = std::fs::read_to_string(root.join("winner")).unwrap();
        let loser = if winner == "one" { "two" } else { "one" };
        assert_eq!(std::fs::read_to_string(root.join(loser)).unwrap(), loser);
    }
    #[test]
    fn case_only_rename_preserves_contents() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("File.txt"), "keep").unwrap();
        move_entry_no_replace(dir.path(), Path::new("File.txt"), Path::new("file.txt")).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("file.txt")).unwrap(),
            "keep"
        );
        let names = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, vec![std::ffi::OsString::from("file.txt")]);
    }
    #[cfg(unix)]
    #[test]
    fn case_similar_hard_links_are_still_collisions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("A"), "keep").unwrap();
        // Only filesystems supporting distinct case names can create this fixture.
        if std::fs::hard_link(dir.path().join("A"), dir.path().join("a")).is_err() {
            return;
        }
        assert_eq!(
            move_entry_no_replace(dir.path(), Path::new("A"), Path::new("a"))
                .unwrap_err()
                .0,
            Reason::DestinationExists
        );
    }
    #[test]
    fn delete_requires_current_revision_and_explicit_recursion() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("folder")).unwrap();
        std::fs::write(root.join("folder/file"), "keep").unwrap();
        let ws = ResolvedWorkspace {
            checkout_id: "checkout".into(),
            root: root.into(),
        };
        let mut req = zeron_proto::DeleteWorkspaceEntryRequest {
            target: request(root, "folder", "unused").target,
            operation_id: "delete".into(),
            expected_checkout_id: "checkout".into(),
            path: "folder".into(),
            expected_source_revision: revision(&std::fs::metadata(root.join("folder")).unwrap()),
            expected_kind: WorkspaceEntryKind::Directory,
            recursive: false,
        };
        assert!(delete_blocking(&ws, &req, &AtomicBool::new(false)).is_err());
        assert!(root.join("folder/file").exists());
        req.recursive = true;
        delete_blocking(&ws, &req, &AtomicBool::new(false)).unwrap();
        assert!(!root.join("folder").exists());
        assert_eq!(
            delete_blocking(&ws, &req, &AtomicBool::new(false))
                .unwrap_err()
                .0,
            Reason::SourceMissing
        );
    }
    #[cfg(unix)]
    #[test]
    fn deleting_directory_does_not_follow_links_to_external_contents() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("folder")).unwrap();
        std::fs::write(outside.path().join("keep"), "keep").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("folder/link")).unwrap();
        let ws = ResolvedWorkspace {
            checkout_id: "checkout".into(),
            root: dir.path().into(),
        };
        let req = zeron_proto::DeleteWorkspaceEntryRequest {
            target: request(dir.path(), "folder", "unused").target,
            operation_id: "delete".into(),
            expected_checkout_id: "checkout".into(),
            path: "folder".into(),
            expected_source_revision: revision(
                &std::fs::metadata(dir.path().join("folder")).unwrap(),
            ),
            expected_kind: WorkspaceEntryKind::Directory,
            recursive: true,
        };
        delete_blocking(&ws, &req, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            std::fs::read_to_string(outside.path().join("keep")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn moves_subtrees_without_replacement_or_prefix_confusion() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("a/child")).unwrap();
        std::fs::create_dir(root.join("ab")).unwrap();
        std::fs::write(root.join("a/child/á.txt"), "keep").unwrap();
        let workspace = ResolvedWorkspace {
            checkout_id: "checkout".into(),
            root: root.into(),
        };
        let cancel = AtomicBool::new(false);
        assert_eq!(
            move_blocking(&workspace, &request(root, "a", "a/child/new"), &cancel)
                .unwrap_err()
                .0,
            Reason::InvalidDestination
        );
        assert_eq!(
            move_blocking(&workspace, &request(root, "a", "ab"), &cancel)
                .unwrap_err()
                .0,
            Reason::DestinationExists
        );
        move_blocking(&workspace, &request(root, "a", "ab/a"), &cancel).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("ab/a/child/á.txt")).unwrap(),
            "keep"
        );
    }
    #[test]
    fn stale_sources_invalid_paths_and_cancel_do_not_mutate() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("a"), "old").unwrap();
        let workspace = ResolvedWorkspace {
            checkout_id: "checkout".into(),
            root: root.into(),
        };
        let req = request(root, "a", "b");
        std::fs::write(root.join("a"), "changed").unwrap();
        assert_eq!(
            move_blocking(&workspace, &req, &AtomicBool::new(false))
                .unwrap_err()
                .0,
            Reason::SourceChanged
        );
        for to in ["../b", ".git/file", "", "/tmp/outside"] {
            assert!(
                move_blocking(&workspace, &request(root, "a", to), &AtomicBool::new(false))
                    .is_err()
            );
        }
        assert_eq!(
            move_blocking(&workspace, &request(root, "a", "b"), &AtomicBool::new(true))
                .unwrap_err()
                .0,
            Reason::Busy
        );
        assert!(root.join("a").exists());
        assert!(!root.join("b").exists());
    }
    #[cfg(unix)]
    #[test]
    fn links_cannot_be_overwritten_or_traversed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("a"), "keep").unwrap();
        std::os::unix::fs::symlink(outside.path().join("missing"), root.join("b")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("link")).unwrap();
        let ws = ResolvedWorkspace {
            checkout_id: "checkout".into(),
            root: root.into(),
        };
        assert_eq!(
            move_blocking(&ws, &request(root, "a", "b"), &AtomicBool::new(false))
                .unwrap_err()
                .0,
            Reason::DestinationExists
        );
        assert!(
            move_blocking(&ws, &request(root, "a", "link/a"), &AtomicBool::new(false)).is_err()
        );
        assert!(root.join("a").exists());
        assert!(!outside.path().join("a").exists());
    }
}
