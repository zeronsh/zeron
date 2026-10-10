//! Workspace search benchmark: the `SearchFiles` (`@`),
//! `SearchWorkspaceFiles` (file tree) and `SearchWorkspaceContent` (cmd+K)
//! paths against one folder.
//!
//! ```text
//! BENCH_ROOT=/path/to/checkout cargo run --release -p zeron-engine \
//!     --example workspace_search_bench
//! ```
//!
//! `BENCH_QUERIES` (comma separated) overrides the default queries and
//! `BENCH_RUNS` the number of timed runs per query (default 10, after one
//! warm-up). RSS comes from `/proc/self/status`, so it prints only on Linux.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use zeron_engine::Repos;

const DEFAULT_QUERIES: &[&str] = &[
    "composer",
    "repos",
    "cmdpal",
    "Cargo.toml",
    "workspace_files",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::var_os("BENCH_ROOT")
        .map(PathBuf::from)
        .ok_or("set BENCH_ROOT to the folder to search")?;
    let root = std::fs::canonicalize(root)?;
    let queries: Vec<String> = match std::env::var("BENCH_QUERIES") {
        Ok(list) => list
            .split(',')
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(str::to_owned)
            .collect(),
        Err(_) => DEFAULT_QUERIES.iter().map(|q| (*q).to_owned()).collect(),
    };
    let runs: usize = std::env::var("BENCH_RUNS")
        .ok()
        .and_then(|runs| runs.parse().ok())
        .unwrap_or(10)
        .max(1);
    let runtime = tokio::runtime::Runtime::new()?;
    let data = tempfile::tempdir()?;
    let repos = Repos::with_worktrees_root(data.path(), "bench", data.path().join("worktrees"));

    println!("root: {}", root.display());
    print_rss("baseline");

    let search = repos.workspace_search();
    let started = Instant::now();
    search.warm(&root)?;
    if !search.wait_until_indexed(&root, Duration::from_secs(600)) {
        return Err("index did not finish within 10 minutes".into());
    }
    println!(
        "index ready in {:?}",
        started.elapsed()
    );
    print_rss("after index");

    println!();
    println!(
        "{:<38} {:>10} {:>10} {:>10} {:>8}",
        "query", "min", "median", "max", "results"
    );
    for query in &queries {
        let (timings, results) = measure(runs, || {
            runtime
                .block_on(repos.search_files(root.clone(), query.clone(), Vec::new()))
                .map(|matches| matches.len())
        })?;
        report(&format!("SearchFiles {query}"), &timings, results);
    }
    for query in &queries {
        let (timings, results) = measure(runs, || tree_search(&repos, &root, query, false))?;
        report(&format!("SearchWorkspaceFiles {query}"), &timings, results);
    }
    for query in &queries {
        let (timings, results) = measure(runs, || tree_search(&repos, &root, query, true))?;
        report(&format!("…including ignored {query}"), &timings, results);
    }
    for query in &queries {
        let (timings, results) = measure(runs, || {
            search
                .search_content(&root, query, 100, 3)
                .map(|found| found.matches.len())
        })?;
        report(
            &format!("SearchWorkspaceContent {query}"),
            &timings,
            results,
        );
    }
    print_rss("after all queries");
    search.release_all();
    // Release runs off-thread; give it and the allocator a moment.
    std::thread::sleep(Duration::from_secs(2));
    println!("live indexes after release: {}", search.live());
    print_rss("after evicting the index");
    Ok(())
}

fn tree_search(
    repos: &Repos,
    root: &Path,
    query: &str,
    include_ignored: bool,
) -> Result<usize, Box<dyn std::error::Error>> {
    Ok(zeron_engine::workspace_files::bench_search_root(
        repos.workspace_search(),
        root,
        query,
        include_ignored,
        zeron_engine::workspace_files::MAX_SEARCH_RESULTS,
    )?
    .len())
}

/// One warm-up, then `runs` timed calls.
fn measure<E: Into<Box<dyn std::error::Error>>>(
    runs: usize,
    mut search: impl FnMut() -> Result<usize, E>,
) -> Result<(Vec<Duration>, usize), Box<dyn std::error::Error>> {
    search().map_err(Into::into)?;
    let mut timings = Vec::with_capacity(runs);
    let mut results = 0;
    for _ in 0..runs {
        let started = Instant::now();
        results = search().map_err(Into::into)?;
        timings.push(started.elapsed());
    }
    timings.sort();
    Ok((timings, results))
}

fn report(label: &str, timings: &[Duration], results: usize) {
    println!(
        "{label:<38} {:>10} {:>10} {:>10} {results:>8}",
        format_duration(timings[0]),
        format_duration(timings[timings.len() / 2]),
        format_duration(timings[timings.len() - 1]),
    );
}

fn format_duration(duration: Duration) -> String {
    format!("{:.2}ms", duration.as_secs_f64() * 1000.0)
}

fn print_rss(label: &str) {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return;
    };
    if let Some(line) = status.lines().find(|line| line.starts_with("VmRSS:")) {
        println!("RSS {label}: {}", line.trim_start_matches("VmRSS:").trim());
    }
}
