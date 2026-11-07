//! A damaged `docs.sqlite3` is found at open, kept aside, and rebuilt with what
//! can still be read, rather than failing every read of every chat for good.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use zeron_sync::DocsStore;

const PAGE: u64 = 4096;

/// A store with a few chats' snapshots, ledger entries, and enough outbox rows
/// to span many pages, closed with everything in its main file.
fn store_with_data(dir: &Path) {
    let store = DocsStore::open(dir).unwrap();
    for i in 0..5 {
        store.save_snapshot(&format!("chat-{i}"), &[i as u8; 300]).unwrap();
        store.mark_processed(&format!("cmd-{i}")).unwrap();
    }
    for i in 0..400 {
        store
            .enqueue_chat_update("filler", &format!("batch-{i}"), &[7u8; 900])
            .unwrap();
    }
    store.flush_for_sleep();
}

fn pages(path: &Path) -> u64 {
    std::fs::metadata(path).unwrap().len() / PAGE
}

/// Overwrite page `n` (0-based) with garbage, as a torn restore would.
fn smash_page(path: &Path, n: u64) {
    let mut file = std::fs::OpenOptions::new().read(true).write(true).open(path).unwrap();
    file.seek(SeekFrom::Start(n * PAGE)).unwrap();
    let mut buf = vec![0u8; PAGE as usize];
    file.read_exact(&mut buf).unwrap();
    for (i, byte) in buf.iter_mut().enumerate() {
        *byte = byte.wrapping_mul(31).wrapping_add(i as u8) ^ 0x5a;
    }
    file.seek(SeekFrom::Start(n * PAGE)).unwrap();
    file.write_all(&buf).unwrap();
    file.sync_all().unwrap();
}

fn kept_copies(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().into_string().unwrap())
        .filter(|n| n.starts_with("docs-corrupt-") && n.ends_with(".sqlite3"))
        .collect();
    names.sort();
    names
}

#[test]
fn a_healthy_store_opens_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    store_with_data(dir.path());
    let store = DocsStore::open(dir.path()).unwrap();
    assert!(store.recovery().is_none());
    assert_eq!(store.load_snapshot("chat-3").unwrap(), Some(vec![3u8; 300]));
    assert!(kept_copies(dir.path()).is_empty());
}

#[test]
fn a_damaged_page_is_found_and_the_rest_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    store_with_data(dir.path());
    let path = dir.path().join("docs.sqlite3");
    // Inside the outbox, past the small tables at the front.
    let n = pages(&path);
    assert!(n > 20, "the store spans {n} pages");
    smash_page(&path, n - 5);

    let store = DocsStore::open(dir.path()).unwrap();
    let recovery = store.recovery().expect("the damage was found").clone();
    assert!(!recovery.reason.is_empty());
    assert!(recovery.kept_at.exists(), "the damaged file is kept for diagnosis");
    assert_eq!(kept_copies(dir.path()).len(), 1);
    assert!(recovery.salvaged_rows > 5, "{recovery:?}");
    // What lives in undamaged pages came across: every chat and the ledger.
    for i in 0..5 {
        assert_eq!(store.load_snapshot(&format!("chat-{i}")).unwrap(), Some(vec![i as u8; 300]));
        assert!(store.is_processed(&format!("cmd-{i}")).unwrap(), "cmd-{i}");
    }
    // And it's a working store again.
    store.save_snapshot("chat-new", b"after").unwrap();
    assert_eq!(store.load_snapshot("chat-new").unwrap(), Some(b"after".to_vec()));
    drop(store);
    let again = DocsStore::open(dir.path()).unwrap();
    assert!(again.recovery().is_none(), "the rebuilt store checks clean");
}

#[test]
fn a_file_that_isnt_a_database_any_more_starts_fresh() {
    let dir = tempfile::tempdir().unwrap();
    store_with_data(dir.path());
    let path = dir.path().join("docs.sqlite3");
    let len = std::fs::metadata(&path).unwrap().len();
    std::fs::write(&path, vec![0xa5u8; len as usize]).unwrap();

    let store = DocsStore::open(dir.path()).unwrap();
    let recovery = store.recovery().expect("recovered");
    assert_eq!(recovery.salvaged_rows, 0);
    assert_eq!(store.load_snapshot("chat-1").unwrap(), None, "back from the rooms instead");
    store.save_snapshot("chat-1", b"fresh").unwrap();
}

#[test]
fn a_marker_from_a_running_engine_asks_for_a_full_check_and_goes_once_clean() {
    let dir = tempfile::tempdir().unwrap();
    store_with_data(dir.path());
    let path = dir.path().join("docs.sqlite3");
    zeron_sync::request_store_check(&path);
    assert!(zeron_sync::store_check_marker(&path).exists());
    let store = DocsStore::open(dir.path()).unwrap();
    assert!(store.recovery().is_none());
    assert!(!zeron_sync::store_check_marker(&path).exists());
}

#[test]
fn only_the_newest_damaged_copies_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("docs.sqlite3");
    for round in 0..5 {
        store_with_data(dir.path());
        std::fs::write(&path, vec![0x11u8 + round; 8192]).unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        assert!(store.recovery().is_some(), "round {round}");
        drop(store);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(kept_copies(dir.path()).len(), 3);
}

#[test]
fn a_read_that_hits_damage_while_running_is_noticed() {
    let dir = tempfile::tempdir().unwrap();
    store_with_data(dir.path());
    let path = dir.path().join("docs.sqlite3");
    let store = DocsStore::open(dir.path()).unwrap();
    // Damaged under a store that's already open, as a bad restore would be.
    let n = pages(&path);
    for page in 2..n {
        smash_page(&path, page);
    }
    let mut failed = false;
    for i in 0..5 {
        failed |= store.load_snapshot(&format!("chat-{i}")).is_err();
    }
    failed |= store.pending_chat_updates("filler").is_err();
    assert!(failed, "the damage was read");
    assert!(zeron_sync::corruption_seen());
}

#[test]
fn a_sleep_leaves_the_database_file_whole_and_the_wal_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = DocsStore::open(dir.path()).unwrap();
    for i in 0..50 {
        store.save_snapshot(&format!("chat-{i}"), &[1u8; 2000]).unwrap();
    }
    let wal = dir.path().join("docs.sqlite3-wal");
    assert!(std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0) > 0, "writes are in the WAL");
    assert!(store.flush_for_sleep());
    assert_eq!(std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0), 0);
    // The file alone, as a snapshot would see it, is the whole store.
    let copy = tempfile::tempdir().unwrap();
    std::fs::copy(dir.path().join("docs.sqlite3"), copy.path().join("docs.sqlite3")).unwrap();
    let alone = DocsStore::open(copy.path()).unwrap();
    assert!(alone.recovery().is_none());
    assert_eq!(alone.load_snapshot("chat-49").unwrap(), Some(vec![1u8; 2000]));
}
