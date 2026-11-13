//! Plugins from someone else: installed from a package file
//! (`.zeron-plugin`), a link to one, a GitHub release carrying one, or a
//! git repository; updated from where each came from.
//!
//! A package is the shared signed package (`package.rs`, the format
//! templates use too) with the plugin's own signed `zeron-plugin.json` as
//! its manifest and the files that signature covers. A git repository is
//! read the same way: its folder packed, then verified like any package. So
//! every way in checks the signature over every file before anything is
//! written, and what's written is checked again before it takes the place
//! of what was there.
//!
//! Installing never decides for you, except for a plugin signed by your own
//! key (`mod.rs`): someone else's waits for the consent window, the app's
//! own. An update must be signed by the key the installed one was, or by
//! another live key the edge verifies to the same account (the author on a
//! new machine), which asks you again; one asking for more than you allowed
//! waits for you again.
//!
//! Where each came from is kept beside its files in `.source.json` (a dot
//! file, so never part of what's signed). An update found is kept in
//! `<plugins>/.updates/<id>.zeron-plugin`, with what it changes in
//! `<id>.json`, until it's installed or the plugin changes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::files::MAX_FILES;
use super::manifest::{self, MANIFEST_FILE, Manifest};
use super::package::{self, MAX_PACKAGE_BYTES, Words};
use super::scan;
use super::signing::{self, SigCheck};

pub const FORMAT: &str = "zeron-plugin:v1";
pub const EXTENSION: &str = "zeron-plugin";
pub const SOURCE_FILE: &str = ".source.json";
/// Where a plugin is put together before it takes its folder.
pub const INCOMING_DIR: &str = ".incoming";
/// Updates found, waiting to be installed.
pub const UPDATES_DIR: &str = ".updates";

const GIT_TIMEOUT: Duration = Duration::from_secs(120);
const HTTP_TIMEOUT: Duration = Duration::from_secs(120);

/// Why installing or updating a plugin failed, as the app shows it in your
/// language: each failure reads `"<reason>: <words>"` (the convention plugin
/// ops fail with, `agents::Failure::text`), the reason one of these, the
/// words English for whoever reads it raw (an agent, the CLI). The app's
/// screens/plugins/words.luau `failure` maps each to a sentence of its
/// locale; an error without one is shown as it reads.
pub mod reason {
    pub const NOT_A_PACKAGE: &str = "not_a_package";
    pub const UNSIGNED: &str = "unsigned";
    pub const TAMPERED: &str = "tampered";
    pub const BAD_SIGNATURE: &str = "bad_signature";
    pub const TOO_BIG: &str = "too_big";
    pub const BAD_FILE: &str = "bad_file";
    /// `git` isn't installed (or can't be run) on the device.
    pub const GIT_MISSING: &str = "git_missing";
    pub const GIT_TIMEOUT: &str = "git_timeout";
    /// Private, missing, or the branch is wrong: git doesn't say which.
    pub const REPO_UNREADABLE: &str = "repo_unreadable";
    pub const REF_NOT_FOUND: &str = "ref_not_found";
    pub const NO_MANIFEST: &str = "no_manifest";
    /// GitHub answered 403 or 429: too many requests from here for now.
    pub const RATE_LIMITED: &str = "rate_limited";
    pub const GITHUB_UNREACHABLE: &str = "github_unreachable";
    pub const RELEASE_NOT_FOUND: &str = "release_not_found";
    /// The release carries no `.zeron-plugin` (or `.zeron-template`).
    pub const NO_PACKAGE: &str = "no_package";
    pub const DOWNLOAD_FAILED: &str = "download_failed";
    pub const NOT_A_LINK: &str = "not_a_link";
    pub const HTTPS_ONLY: &str = "https_only";
    /// One by the same id is installed, signed by another key.
    pub const OTHER_SIGNER: &str = "other_signer";
    /// An update found is signed by another key than the installed one.
    pub const NEW_SIGNER: &str = "new_signer";
    /// One by the same id is installed and isn't signed.
    pub const INSTALLED_UNSIGNED: &str = "installed_unsigned";
    pub const NOT_NEWER: &str = "not_newer";
    pub const DIFFERENT_PLUGIN: &str = "different_plugin";
    pub const UP_TO_DATE: &str = "up_to_date";
    /// Reinstalling: where it came from isn't known, or is gone.
    pub const NO_SOURCE: &str = "no_source";
}

/// `"<reason>: <words>"`.
pub fn failure(reason: &str, words: impl std::fmt::Display) -> String {
    format!("{reason}: {words}")
}

/// The reason `error` carries, if it carries one of [`reason`]'s.
pub fn reason_of(error: &str) -> Option<&str> {
    let (code, _) = error.split_once(": ")?;
    (!code.is_empty() && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')).then_some(code)
}

pub const WORDS: Words = Words {
    not_a_package: "not_a_package: That isn't a Zeron plugin",
    unsigned: "unsigned: It isn't signed, so there's no telling who made it",
    tampered: "tampered: Its files don't match its signature: it was changed after it was signed",
    bad_signature: "bad_signature: Its signature doesn't check out",
    too_big: "too_big: It's too big for a plugin",
    bad_file: "bad_file: It has a file Zeron won't write ({rel})",
};

/// Where a plugin came from (`.source.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    /// `file`, `link`, `release` or `git`.
    pub kind: String,
    /// The link, the repository (`release`, `git`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The package file (`file`); the plugin's folder in the repository (`git`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The branch or tag (`git`); the release's tag, none for the latest (`release`).
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    /// The commit it was installed from (`git`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub installed_at: i64,
}

impl Source {
    pub fn load(dir: &Path) -> Option<Self> {
        std::fs::read(dir.join(SOURCE_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    }

    /// What the app is shown: never a local path.
    pub fn shown(&self) -> Value {
        let mut value = json!({ "kind": self.kind });
        if let Some(url) = &self.url {
            value["url"] = json!(url);
        }
        if let Some(r) = &self.git_ref {
            value["ref"] = json!(r);
        }
        value
    }
}

/// A plugin package, verified.
#[derive(Debug, Clone)]
pub struct Package {
    pub manifest: Manifest,
    pub key: String,
    pub files: BTreeMap<String, Vec<u8>>,
    /// The plugin's content hash, as a scan of it installed will say.
    pub hash: String,
}

impl Package {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub fn name(&self) -> &str {
        if self.manifest.name.is_empty() {
            &self.manifest.id
        } else {
            &self.manifest.name
        }
    }
}

fn keep_path(rel: &str) -> bool {
    package::plain_path(rel) && rel != MANIFEST_FILE
}

/// The plugin package `raw`, verified (signature over every file, a
/// manifest Zeron can run, an id it can be installed under).
pub fn parse(raw: Vec<u8>) -> Result<Package, String> {
    let verified = package::parse(raw, FORMAT, signing::PAYLOAD_PREFIX, keep_path, &WORDS)?;
    let manifest = Manifest::from_value(verified.manifest.clone())?;
    if let Some(problem) = manifest.problem() {
        return Err(problem.to_owned());
    }
    manifest::check_id(&manifest.id)?;
    if !verified.files.contains_key(&manifest.entry) {
        return Err(format!("Its entry file {} is missing", manifest.entry));
    }
    // What `scan::inspect` checks of a folder: the signature as a plugin's.
    let hashes = package::hashes(&verified.files);
    if signing::check(&manifest, &hashes) != SigCheck::Valid(verified.key.clone()) {
        return Err(WORDS.bad_signature.into());
    }
    let hash = signing::content_hash(&manifest, &hashes);
    Ok(Package {
        manifest,
        key: verified.key,
        files: verified.files,
        hash,
    })
}

/// The plugin folder `dir` as a package, if it's signed and its files are
/// what was signed. (An own plugin is re-signed before it's exported.)
pub fn pack_folder(dir: &Path) -> Result<Vec<u8>, String> {
    let inspected = scan::inspect(dir, false);
    if let Some(error) = inspected.error {
        return Err(error);
    }
    let manifest = inspected.manifest.as_ref().ok_or(WORDS.not_a_package)?;
    match &inspected.sig {
        SigCheck::Valid(_) => {}
        SigCheck::Unsigned => return Err(WORDS.unsigned.into()),
        SigCheck::FilesDiffer => return Err(WORDS.tampered.into()),
        SigCheck::BadSignature => return Err(WORDS.bad_signature.into()),
    }
    if inspected.files.len() > MAX_FILES {
        return Err(WORDS.too_big.into());
    }
    let mut files = BTreeMap::new();
    for rel in inspected.files.keys() {
        let path = zeron_plugin_guard::path::join(dir, rel)?;
        let bytes = std::fs::read(path).map_err(|_| format!("Couldn't read {rel}"))?;
        files.insert(rel.clone(), bytes);
    }
    package::pack(FORMAT, &manifest.raw, &files)
}

/// What an update changes in what a plugin asks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GrantChange {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

pub fn grant_change(before: &[String], after: &[String]) -> GrantChange {
    let before: BTreeSet<&String> = before.iter().collect();
    let after: BTreeSet<&String> = after.iter().collect();
    GrantChange {
        added: after.difference(&before).map(|s| (*s).clone()).collect(),
        removed: before.difference(&after).map(|s| (*s).clone()).collect(),
    }
}

/// Whether `candidate` is a later version than `installed`: semver when
/// both are, else dot-separated numbers (`2` > `1.9`, a missing part is 0).
/// Anything it can't order is not newer: an update must say it's later.
pub fn newer(candidate: &str, installed: &str) -> bool {
    let clean = |v: &str| v.trim().trim_start_matches(['v', 'V']).to_owned();
    let (a, b) = (clean(candidate), clean(installed));
    if let (Ok(a), Ok(b)) = (semver::Version::parse(&a), semver::Version::parse(&b)) {
        return a > b;
    }
    let numbers = |v: &str| v.split('.').map(|p| p.parse::<u64>().ok()).collect::<Option<Vec<u64>>>();
    let (Some(mut a), Some(mut b)) = (numbers(&a), numbers(&b)) else {
        return false;
    };
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    a > b
}

pub fn not_newer(candidate: &str, installed: &str) -> String {
    failure(reason::NOT_NEWER, format!("Version {candidate} isn't later than the {installed} installed, so it won't replace it"))
}

/// What was installed over, when it was an update.
#[derive(Debug, Clone)]
pub struct Replaced {
    /// The key the replaced one was signed by.
    pub key: String,
    pub version: String,
    pub permissions: Vec<String>,
    pub hash: String,
}

/// The installed plugin `id`'s key, version, permissions and hash, if it's
/// signed and its files are what was signed.
fn installed(dir: &Path) -> Result<(String, Replaced), String> {
    let inspected = scan::inspect(dir, true);
    let manifest = inspected.manifest.as_ref();
    let SigCheck::Valid(key) = inspected.sig.clone() else {
        return Err(failure(
            reason::INSTALLED_UNSIGNED,
            format!(
                "{} is installed already, and it isn't signed. Remove it first to install this one.",
                manifest.map_or(inspected.folder.as_str(), |m| m.name.as_str())
            ),
        ));
    };
    Ok((
        key.clone(),
        Replaced {
            key,
            version: manifest.map(|m| m.version.clone()).unwrap_or_default(),
            permissions: manifest.map(|m| m.permissions.clone()).unwrap_or_default(),
            hash: inspected.hash,
        },
    ))
}

/// Install `package` under `plugins_dir`, from `source`. Over an installed
/// plugin only when the same key signed both, or the installed one's key is
/// `rotated_from` (checked by the caller to be the same account's as the
/// package's); never over anything else. Returns what it replaced.
pub fn install(plugins_dir: &Path, package: &Package, source: &Source, rotated_from: Option<&str>) -> Result<Option<Replaced>, String> {
    let id = package.id().to_owned();
    let dir = plugins_dir.join(&id);
    let replaced = if std::fs::symlink_metadata(&dir).is_ok() {
        let (key, replaced) = installed(&dir)?;
        if key != package.key && rotated_from != Some(key.as_str()) {
            return Err(failure(
                reason::OTHER_SIGNER,
                format!("A plugin called {id} is installed already, signed by someone else. Remove it first to install this one."),
            ));
        }
        // Signed downgrades (or a different build under the same version)
        // are refused: an old version's holes stay closed.
        if replaced.hash != package.hash && !newer(&package.manifest.version, &replaced.version) {
            return Err(not_newer(&package.manifest.version, &replaced.version));
        }
        Some(replaced)
    } else {
        None
    };
    let incoming = plugins_dir.join(INCOMING_DIR);
    std::fs::create_dir_all(&incoming).map_err(|_| "Couldn't make room for it".to_string())?;
    let staged = incoming.join(format!("{id}-{}", crate::new_id()));
    let result = stage(&staged, package, source).and_then(|()| {
        // What's about to be installed is what was verified.
        let check = scan::inspect(&staged, false);
        if check.sig != SigCheck::Valid(package.key.clone()) || check.hash != package.hash {
            return Err("It changed while it was being installed".to_string());
        }
        if replaced.is_some() {
            let old = incoming.join(format!("{id}-old-{}", crate::new_id()));
            std::fs::rename(&dir, &old).map_err(|_| "Couldn't replace the installed one".to_string())?;
            if std::fs::rename(&staged, &dir).is_err() {
                let _ = std::fs::rename(&old, &dir);
                return Err("Couldn't replace the installed one".into());
            }
            let _ = std::fs::remove_dir_all(&old);
            Ok(())
        } else {
            std::fs::rename(&staged, &dir).map_err(|_| "Couldn't install it".to_string())
        }
    });
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staged);
    }
    result.map(|()| replaced)
}

fn stage(staged: &Path, package: &Package, source: &Source) -> Result<(), String> {
    let write_error = |_| "Couldn't write its files".to_string();
    std::fs::create_dir_all(staged).map_err(write_error)?;
    for (rel, bytes) in &package.files {
        // (Every path through the one join that keeps it inside, on every
        // system: `D:x.cmd` never leaves the staging folder.)
        let path = zeron_plugin_guard::path::join(staged, rel)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(write_error)?;
        }
        std::fs::write(&path, bytes).map_err(write_error)?;
    }
    std::fs::write(staged.join(MANIFEST_FILE), signing::pretty(&package.manifest.raw)).map_err(write_error)?;
    let source = serde_json::to_vec_pretty(source).unwrap_or_default();
    std::fs::write(staged.join(SOURCE_FILE), source).map_err(write_error)?;
    Ok(())
}

// ── Where a package comes from ──────────────────────────────────────────────

/// What a link names.
#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
    /// A package file to download.
    Link(String),
    /// A GitHub repository's release (`None`: the latest).
    Release { repo: String, tag: Option<String> },
    /// A git repository, at a branch or tag, the plugin in a folder of it.
    Git { url: String, git_ref: Option<String>, path: Option<String> },
}

fn plain_url(url: &str) -> bool {
    url.starts_with("https://") || (cfg!(test) && url.starts_with("file://"))
}

/// `link` as people paste it, made a full https link: `owner/repo` is
/// GitHub's, a host without a scheme (`github.com/owner/repo`) is https.
/// None when it isn't an https link at all.
pub fn https_link(link: &str) -> Option<String> {
    let link = link.trim();
    // `owner/repo`: GitHub.
    let shorthand = link.split('/').count() == 2
        && !link.contains(':')
        && link.split('/').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)));
    // A host without a scheme (`github.com/owner/repo`, as people paste
    // it): https.
    let bare_host = !link.contains("://")
        && link.split('/').next().is_some_and(|host| host.contains('.') && !host.starts_with('.') && !host.contains(['@', ':']))
        && link.split('/').count() > 1;
    let link = if shorthand {
        format!("https://github.com/{link}")
    } else if bare_host {
        format!("https://{link}")
    } else {
        link.to_owned()
    };
    plain_url(&link).then_some(link)
}

/// What `link` names, if it's something a plugin can come from: a package
/// link (`….zeron-plugin`), a GitHub release (`…/releases/latest`,
/// `…/releases/tag/<tag>`), a GitHub repository (`owner/repo`, its page,
/// `…/tree/<branch>/<folder>`), or any `https` git repository.
pub fn origin(link: &str, git_ref: Option<&str>, path: Option<&str>) -> Result<Origin, String> {
    let link = link.trim();
    let not_one = || failure(reason::NOT_A_LINK, "That link isn't a plugin package or a git repository");
    if link.is_empty() || link.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(not_one());
    }
    let clean = |s: Option<&str>| s.map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let (git_ref, path) = (clean(git_ref), clean(path));
    if let Some(path) = &path
        && !package::plain_path(path)
    {
        return Err("That folder isn't inside the repository".into());
    }
    if let Some(r) = &git_ref
        && (r.starts_with('-') || r.contains(['\\', ' ', ':', '\0']) || r.contains(".."))
    {
        return Err("That branch or tag can't be used".into());
    }
    let Some(link) = https_link(link) else {
        return Err(failure(reason::HTTPS_ONLY, "Plugins come from https links"));
    };
    let bare = link.split(['?', '#']).next().unwrap_or(&link).trim_end_matches('/');
    if bare.ends_with(&format!(".{EXTENSION}")) {
        return Ok(Origin::Link(link));
    }
    if let Some(rest) = bare.strip_prefix("https://github.com/") {
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(not_one());
        }
        let repo = format!("{}/{}", parts[0], parts[1].trim_end_matches(".git"));
        match &parts[2..] {
            [] => {}
            ["releases"] | ["releases", "latest"] => return Ok(Origin::Release { repo, tag: None }),
            ["releases", "tag", tag] => return Ok(Origin::Release { repo, tag: Some((*tag).to_owned()) }),
            ["tree", branch, folder @ ..] => {
                let folder = (!folder.is_empty()).then(|| folder.join("/"));
                if let Some(folder) = &folder
                    && !package::plain_path(folder)
                {
                    return Err(not_one());
                }
                return Ok(Origin::Git {
                    url: format!("https://github.com/{repo}.git"),
                    git_ref: git_ref.or(Some((*branch).to_owned())),
                    path: path.or(folder),
                });
            }
            _ => return Err(not_one()),
        }
        return Ok(Origin::Git { url: format!("https://github.com/{repo}.git"), git_ref, path });
    }
    Ok(Origin::Git { url: link, git_ref, path })
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .https_only(!cfg!(test))
        .user_agent("Zeron")
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// The package at `url`, read up to the largest a package may be.
pub async fn download(url: &str) -> Result<Vec<u8>, String> {
    let mut response = http()
        .get(url)
        .send()
        .await
        .map_err(|_| failure(reason::DOWNLOAD_FAILED, "Couldn't download it"))?;
    let status = response.status().as_u16();
    if status == 429 || (status == 403 && github_host(url)) {
        return Err(rate_limited());
    }
    if !response.status().is_success() {
        return Err(failure(reason::DOWNLOAD_FAILED, format!("Couldn't download it ({status})")));
    }
    if response.content_length().is_some_and(|n| n > MAX_PACKAGE_BYTES) {
        return Err(WORDS.too_big.into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| failure(reason::DOWNLOAD_FAILED, "Couldn't download it"))? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() as u64 > MAX_PACKAGE_BYTES {
            return Err(WORDS.too_big.into());
        }
    }
    Ok(bytes)
}

/// The `.zeron-plugin` asset of `repo`'s release `tag` (the latest when none).
pub async fn release_asset(repo: &str, tag: Option<&str>) -> Result<String, String> {
    release_asset_of(repo, tag, EXTENSION).await
}

/// The asset of `repo`'s release `tag` (the latest when none) whose name
/// ends `.{extension}` (a plugin's, a template's).
pub async fn release_asset_of(repo: &str, tag: Option<&str>, extension: &str) -> Result<String, String> {
    let api = match tag {
        Some(tag) => format!("https://api.github.com/repos/{repo}/releases/tags/{tag}"),
        None => format!("https://api.github.com/repos/{repo}/releases/latest"),
    };
    let ask = |token: Option<&str>| {
        let mut request = http().get(&api).header("Accept", "application/vnd.github+json");
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        request.send()
    };
    let unreachable = |_| failure(reason::GITHUB_UNREACHABLE, "Couldn't reach GitHub");
    // As you, when you've signed in to GitHub here (GitHub allows a person
    // far more requests than a device asking anonymously); anonymously
    // again when that sign-in is turned away (expired, revoked).
    let token = github_token().await;
    let mut response = ask(token.as_deref()).await.map_err(unreachable)?;
    if token.is_some() && response.status().as_u16() == 401 {
        response = ask(None).await.map_err(unreachable)?;
    }
    match response.status().as_u16() {
        403 | 429 => return Err(rate_limited()),
        _ if !response.status().is_success() => {
            return Err(failure(reason::RELEASE_NOT_FOUND, "That release can't be found"));
        }
        _ => {}
    }
    let release: Value = response
        .json()
        .await
        .map_err(|_| failure(reason::RELEASE_NOT_FOUND, "Couldn't read that release"))?;
    release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(&format!(".{extension}"))))
        .and_then(|a| a["browser_download_url"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| failure(reason::NO_PACKAGE, format!("That release has no .{extension} file")))
}

/// GitHub's words for too many requests (a 403 or 429 from it).
fn rate_limited() -> String {
    failure(
        reason::RATE_LIMITED,
        "GitHub is turning away requests from this device for now (too many in a short while). Try again later, or sign in with GitHub's own tool (`gh auth login`) so Zeron asks as you",
    )
}

/// Whether `url` is GitHub's (where a release's files are served from too).
fn github_host(url: &str) -> bool {
    let host = url.trim_start_matches("https://").split(['/', '?', '#']).next().unwrap_or("");
    host == "github.com" || host.ends_with(".github.com") || host.ends_with(".githubusercontent.com")
}

/// How long a GitHub sign-in read from `gh` is used before it's read again.
const TOKEN_FOR: Duration = Duration::from_secs(10 * 60);

/// Your GitHub token on this device, if you have one: `GITHUB_TOKEN` or
/// `GH_TOKEN`, else what `gh auth token` gives (read at most every few
/// minutes). Only ever sent to api.github.com, to look up releases.
async fn github_token() -> Option<String> {
    if cfg!(test) {
        return None;
    }
    for name in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Some(token) = std::env::var(name).ok().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()) {
            return Some(token);
        }
    }
    static READ: std::sync::Mutex<Option<(std::time::Instant, Option<String>)>> = std::sync::Mutex::new(None);
    if let Some((at, token)) = READ.lock().ok().and_then(|r| r.clone())
        && at.elapsed() < TOKEN_FOR
    {
        return token;
    }
    let mut cmd = tokio::process::Command::new("gh");
    cmd.args(["auth", "token", "--hostname", "github.com"])
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.as_std_mut().creation_flags(0x08000000);
    }
    let token = tokio::time::timeout(Duration::from_secs(5), cmd.output())
        .await
        .ok()
        .and_then(Result::ok)
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty() && t.len() < 512 && !t.chars().any(char::is_whitespace));
    if let Ok(mut read) = READ.lock() {
        *read = Some((std::time::Instant::now(), token.clone()));
    }
    token
}

async fn git(args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new("git");
    if !cfg!(test) {
        cmd.args(["-c", "protocol.file.allow=never", "-c", "protocol.ext.allow=never"]);
    }
    cmd.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .env("GCM_INTERACTIVE", "never")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.as_std_mut().creation_flags(0x08000000);
    }
    let output = tokio::time::timeout(GIT_TIMEOUT, cmd.output())
        .await
        .map_err(|_| failure(reason::GIT_TIMEOUT, "The repository took too long to answer"))?
        .map_err(|_| {
            failure(
                reason::GIT_MISSING,
                "git isn't available on this device: install git, or install the plugin from a .zeron-plugin file or a GitHub release instead",
            )
        })?;
    if !output.status.success() {
        return Err(failure(reason::REPO_UNREADABLE, "Couldn't read that repository (is it public, and is the branch right?)"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The commit `git_ref` (or the default branch) is at in `url`, without
/// cloning it.
pub async fn git_head(url: &str, git_ref: Option<&str>) -> Result<String, String> {
    let out = git(&["ls-remote", "--", url, git_ref.unwrap_or("HEAD")], None).await?;
    out.split_whitespace()
        .next()
        .filter(|sha| sha.len() >= 40)
        .map(str::to_owned)
        .ok_or_else(|| failure(reason::REF_NOT_FOUND, "That branch or tag can't be found"))
}

/// `url` at `git_ref`, cloned into `scratch`: the plugin in it (at `path`)
/// packed, and the commit.
pub async fn from_git(url: &str, git_ref: Option<&str>, path: Option<&str>, scratch: &Path) -> Result<(Vec<u8>, String), String> {
    let clone = scratch.join(format!("git-{}", crate::new_id()));
    let dest = clone.display().to_string();
    let mut args = vec!["clone", "--depth", "1", "--single-branch", "--no-tags"];
    if let Some(r) = git_ref {
        args.extend(["--branch", r]);
    }
    args.extend(["--", url, &dest]);
    let result = async {
        git(&args, None).await?;
        let commit = git(&["rev-parse", "HEAD"], Some(&clone)).await?;
        let folder = plugin_folder(&clone, path)?;
        let raw = tokio::task::spawn_blocking(move || pack_folder(&folder))
            .await
            .map_err(|_| "Couldn't read it".to_string())??;
        Ok((raw, commit))
    }
    .await;
    let _ = std::fs::remove_dir_all(&clone);
    result
}

/// The plugin's folder at `path` in the repository cloned to `clone`, which
/// must stay inside it however its links point (a folder linked out of the
/// clone would pack files from this computer), with its own
/// `zeron-plugin.json` (not a link).
fn plugin_folder(clone: &Path, path: Option<&str>) -> Result<PathBuf, String> {
    let missing = || match path {
        Some(path) => failure(reason::NO_MANIFEST, format!("There's no zeron-plugin.json in {path}")),
        None => failure(reason::NO_MANIFEST, "There's no zeron-plugin.json at the top of that repository"),
    };
    let root = std::fs::canonicalize(clone).map_err(|_| "Couldn't read that repository".to_string())?;
    let folder = match path {
        Some(path) => std::fs::canonicalize(clone.join(path)).map_err(|_| missing())?,
        None => root.clone(),
    };
    if !folder.starts_with(&root) {
        return Err(format!("{} isn't inside that repository", path.unwrap_or_default()));
    }
    if !std::fs::symlink_metadata(folder.join(MANIFEST_FILE)).is_ok_and(|m| m.is_file()) {
        return Err(missing());
    }
    Ok(folder)
}

/// The package `origin` names, and where it came from (its `Source`,
/// without `version`/`installed_at`, which installing fills).
pub async fn fetch(origin: &Origin, scratch: &Path) -> Result<(Package, Source), String> {
    let (raw, source) = match origin {
        Origin::Link(url) => (
            download(url).await?,
            Source { kind: "link".into(), url: Some(url.clone()), path: None, git_ref: None, commit: None, version: String::new(), installed_at: 0 },
        ),
        Origin::Release { repo, tag } => {
            let asset = release_asset(repo, tag.as_deref()).await?;
            (
                download(&asset).await?,
                Source {
                    kind: "release".into(),
                    url: Some(format!("https://github.com/{repo}")),
                    path: None,
                    git_ref: tag.clone(),
                    commit: None,
                    version: String::new(),
                    installed_at: 0,
                },
            )
        }
        Origin::Git { url, git_ref, path } => {
            let (raw, commit) = from_git(url, git_ref.as_deref(), path.as_deref(), scratch).await?;
            (
                raw,
                Source { kind: "git".into(), url: Some(url.clone()), path: path.clone(), git_ref: git_ref.clone(), commit: Some(commit), version: String::new(), installed_at: 0 },
            )
        }
    };
    let package = tokio::task::spawn_blocking(move || parse(raw))
        .await
        .map_err(|_| "Couldn't read it".to_string())??;
    Ok((package, source))
}

/// `source` fetched again as it is now, whatever changed: what reinstalling
/// a plugin installs.
pub async fn refetch(source: &Source, scratch: &Path) -> Result<(Package, Source), String> {
    let gone = || failure(reason::NO_SOURCE, "Where it came from isn't known, so it can't be installed again from there");
    let origin = match source.kind.as_str() {
        "file" => {
            let path = source.path.clone().map(PathBuf::from).filter(|p| p.is_file()).ok_or_else(|| {
                failure(reason::NO_SOURCE, "The file it was installed from isn't there any more")
            })?;
            let package = tokio::task::spawn_blocking(move || package::read(&path, &WORDS).and_then(parse))
                .await
                .map_err(|_| "Couldn't read it".to_string())??;
            return Ok((package, source.clone()));
        }
        "link" => Origin::Link(source.url.clone().ok_or_else(gone)?),
        "release" => {
            let url = source.url.as_deref().ok_or_else(gone)?;
            Origin::Release { repo: url.trim_start_matches("https://github.com/").to_owned(), tag: source.git_ref.clone() }
        }
        "git" => Origin::Git { url: source.url.clone().ok_or_else(gone)?, git_ref: source.git_ref.clone(), path: source.path.clone() },
        _ => return Err(gone()),
    };
    fetch(&origin, scratch).await
}

/// Where `source` is now, if it may have changed: `None` when it surely
/// hasn't (a git branch still at the same commit; a package file gone).
pub async fn fetch_again(source: &Source, scratch: &Path) -> Result<Option<(Package, Source)>, String> {
    match source.kind.as_str() {
        "file" => {
            let Some(path) = source.path.clone() else { return Ok(None) };
            let path = PathBuf::from(path);
            if !path.is_file() {
                return Ok(None);
            }
            let package = tokio::task::spawn_blocking(move || package::read(&path, &WORDS).and_then(parse))
                .await
                .map_err(|_| "Couldn't read it".to_string())??;
            Ok(Some((package, source.clone())))
        }
        "link" => {
            let url = source.url.clone().ok_or("It has no link")?;
            fetch(&Origin::Link(url), scratch).await.map(Some)
        }
        "release" => {
            let url = source.url.as_deref().ok_or("It has no repository")?;
            let repo = url.trim_start_matches("https://github.com/").to_owned();
            fetch(&Origin::Release { repo, tag: source.git_ref.clone() }, scratch).await.map(Some)
        }
        "git" => {
            let url = source.url.clone().ok_or("It has no repository")?;
            let head = git_head(&url, source.git_ref.as_deref()).await?;
            if source.commit.as_deref() == Some(head.as_str()) {
                return Ok(None);
            }
            fetch(&Origin::Git { url, git_ref: source.git_ref.clone(), path: source.path.clone() }, scratch)
                .await
                .map(Some)
        }
        _ => Ok(None),
    }
}

// ── Updates found ───────────────────────────────────────────────────────────

/// An update found for an installed plugin (`.updates/<id>.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Found {
    /// The installed plugin's hash this was found against (a plugin that
    /// changed since has to be checked again).
    pub from: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hash: String,
    #[serde(default)]
    pub added: Vec<String>,
    #[serde(default)]
    pub removed: Vec<String>,
    /// Why it can't be installed (signed by someone else, tampered).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// Where it came from this time (a git commit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    /// What it grows past what you allowed (`capability::Diff`), when it
    /// grows anything: installing it waits for you.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest_diff: Option<Value>,
    /// Signed by another of its author's keys (a new machine): installing it
    /// asks you again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new_key: bool,
}

impl Found {
    /// What the app is shown.
    pub fn shown(&self) -> Value {
        let mut value = json!({ "version": self.version, "added": self.added, "removed": self.removed });
        if let Some(refused) = &self.refused {
            value["refused"] = json!(refused);
        }
        if let Some(diff) = &self.digest_diff {
            value["digest_diff"] = diff.clone();
        }
        if self.new_key {
            value["new_key"] = json!(true);
        }
        value
    }
}

pub fn updates_dir(plugins_dir: &Path) -> PathBuf {
    plugins_dir.join(UPDATES_DIR)
}

pub fn found_path(plugins_dir: &Path, id: &str) -> PathBuf {
    updates_dir(plugins_dir).join(format!("{id}.json"))
}

pub fn found_package_path(plugins_dir: &Path, id: &str) -> PathBuf {
    updates_dir(plugins_dir).join(format!("{id}.{EXTENSION}"))
}

/// The update found for `id`, if it's still for the plugin as it is (`hash`).
pub fn found(plugins_dir: &Path, id: &str, hash: &str) -> Option<Found> {
    let found: Found = std::fs::read(found_path(plugins_dir, id))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())?;
    (found.from == hash).then_some(found)
}

pub fn forget_found(plugins_dir: &Path, id: &str) {
    let _ = std::fs::remove_file(found_path(plugins_dir, id));
    let _ = std::fs::remove_file(found_package_path(plugins_dir, id));
}

/// Keep `package` as `id`'s update, found against the plugin at `from`.
pub fn keep_found(plugins_dir: &Path, id: &str, found: &Found, package: Option<&Package>) -> Result<(), String> {
    let dir = updates_dir(plugins_dir);
    std::fs::create_dir_all(&dir).map_err(|_| "Couldn't keep the update".to_string())?;
    match package {
        Some(package) => {
            let raw = package::pack(FORMAT, &package.manifest.raw, &package.files)?;
            std::fs::write(found_package_path(plugins_dir, id), raw).map_err(|_| "Couldn't keep the update".to_string())?;
        }
        None => {
            let _ = std::fs::remove_file(found_package_path(plugins_dir, id));
        }
    }
    let bytes = serde_json::to_vec_pretty(found).unwrap_or_default();
    std::fs::write(found_path(plugins_dir, id), bytes).map_err(|_| "Couldn't keep the update".to_string())
}

/// The plugin installed, as a check compares a new version with it.
pub struct Installed<'a> {
    pub id: &'a str,
    pub key: &'a str,
    pub version: &'a str,
    pub permissions: &'a [String],
    pub hash: &'a str,
    /// The capability record you allowed it (its own, when you haven't).
    pub allowed: &'a Value,
}

/// What a check of `installed` found in `fetched`: nothing new, or an
/// update (refused when someone else signed it, or it isn't later).
/// `same_account`: `fetched`'s key, though not the installed one's, is a
/// live key of the same account (checked by the caller).
pub fn compare(installed: &Installed<'_>, fetched: &Package, source: Source, same_account: bool) -> Option<Found> {
    let Installed { id, key, version, permissions, hash, allowed } = *installed;
    if fetched.hash == hash {
        return None;
    }
    let change = grant_change(permissions, &fetched.manifest.permissions);
    let refused = if fetched.id() != id {
        Some(failure(reason::DIFFERENT_PLUGIN, format!("The new version is a different plugin ({})", fetched.id())))
    } else if fetched.key != key && !same_account {
        Some(failure(reason::NEW_SIGNER, "The new version is signed by someone else, so it won't be installed"))
    } else if !newer(&fetched.manifest.version, version) {
        Some(not_newer(&fetched.manifest.version, version))
    } else {
        None
    };
    let diff = super::capability::diff(allowed, &super::capability::record(&fetched.manifest));
    Some(Found {
        from: hash.to_owned(),
        version: fetched.manifest.version.clone(),
        hash: fetched.hash.clone(),
        added: change.added,
        removed: change.removed,
        refused,
        source: Some(source),
        digest_diff: diff.grew().then(|| diff.to_value()),
        new_key: fetched.key != key,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::plugins::signing::SigningKey;

    pub fn signed_package(key: &SigningKey, id: &str, version: &str, permissions: &[&str], body: &str) -> Vec<u8> {
        let mut files = BTreeMap::new();
        files.insert("init.luau".to_string(), body.as_bytes().to_vec());
        files.insert("ui/badge.luau".to_string(), b"return {}".to_vec());
        let manifest = json!({
            "id": id, "name": id, "version": version, "entry": "init.luau", "permissions": permissions,
        });
        let manifest = key.sign_manifest(&manifest, &package::hashes(&files));
        package::pack(FORMAT, &manifest, &files).unwrap()
    }

    #[test]
    fn a_plugin_package_installs_what_was_signed_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let (key, _) = SigningKey::generate().unwrap();
        let raw = signed_package(&key, "focus", "1.0.0", &["ui.add"], "return 1");
        let package = parse(raw.clone()).unwrap();
        assert_eq!((package.id(), package.key.as_str()), ("focus", key.public().as_str()));
        let source = Source { kind: "file".into(), url: None, path: Some("/x.zeron-plugin".into()), git_ref: None, commit: None, version: "1.0.0".into(), installed_at: 1 };
        assert!(install(dir.path(), &package, &source, None).unwrap().is_none());
        let installed = scan::inspect(&dir.path().join("focus"), true);
        assert_eq!(installed.sig, SigCheck::Valid(key.public()));
        assert_eq!(installed.hash, package.hash);
        assert_eq!(Source::load(&dir.path().join("focus")), Some(source.clone()));
        // Nothing left half-way.
        assert_eq!(std::fs::read_dir(dir.path().join(INCOMING_DIR)).unwrap().count(), 0);
        // Packed back from its folder, it's the same plugin.
        let again = parse(pack_folder(&dir.path().join("focus")).unwrap()).unwrap();
        assert_eq!(again.hash, package.hash);

        // An update by the same key replaces it; by another key, refused.
        let newer = parse(signed_package(&key, "focus", "1.1.0", &["ui.add", "links.open"], "return 2")).unwrap();
        let replaced = install(dir.path(), &newer, &source, None).unwrap().unwrap();
        assert_eq!((replaced.version.as_str(), replaced.hash.as_str()), ("1.0.0", package.hash.as_str()));
        assert_eq!(grant_change(&replaced.permissions, &newer.manifest.permissions), GrantChange { added: vec!["links.open".into()], removed: vec![] });
        let (eve, _) = SigningKey::generate().unwrap();
        let theirs = parse(signed_package(&eve, "focus", "9.0.0", &[], "return 3")).unwrap();
        assert!(install(dir.path(), &theirs, &source, None).unwrap_err().contains("signed by someone else"));
        assert_eq!(scan::inspect(&dir.path().join("focus"), true).hash, newer.hash);

        // A package carrying its own zeron-plugin.json as a file, refused.
        let mut files = BTreeMap::new();
        files.insert("init.luau".to_string(), b"return 1".to_vec());
        files.insert(MANIFEST_FILE.to_string(), b"{}".to_vec());
        let manifest = key.sign_manifest(&json!({ "id": "sneaky", "name": "S", "version": "1", "entry": "init.luau" }), &package::hashes(&files));
        assert!(parse(package::pack(FORMAT, &manifest, &files).unwrap()).unwrap_err().starts_with("bad_file: It has a file Zeron won't write"));
        // Zeron's own name, refused.
        assert!(parse(signed_package(&key, "zeron", "1", &[], "return 1")).is_err());
    }

    #[test]
    fn links_name_where_a_plugin_comes_from() {
        let git = |url: &str, r: Option<&str>, p: Option<&str>| Origin::Git { url: url.into(), git_ref: r.map(Into::into), path: p.map(Into::into) };
        assert_eq!(origin("ada/focus", None, None).unwrap(), git("https://github.com/ada/focus.git", None, None));
        assert_eq!(origin("https://github.com/ada/focus", Some("v2"), None).unwrap(), git("https://github.com/ada/focus.git", Some("v2"), None));
        assert_eq!(origin("github.com/ada/focus", None, None).unwrap(), git("https://github.com/ada/focus.git", None, None));
        assert_eq!(
            origin("example.com/x/focus.zeron-plugin", None, None).unwrap(),
            Origin::Link("https://example.com/x/focus.zeron-plugin".into())
        );
        assert_eq!(
            origin("https://github.com/ada/tools/tree/main/plugins/focus", None, None).unwrap(),
            git("https://github.com/ada/tools.git", Some("main"), Some("plugins/focus"))
        );
        assert_eq!(origin("https://github.com/ada/focus/releases/latest", None, None).unwrap(), Origin::Release { repo: "ada/focus".into(), tag: None });
        assert_eq!(
            origin("https://github.com/ada/focus/releases/tag/v1.2", None, None).unwrap(),
            Origin::Release { repo: "ada/focus".into(), tag: Some("v1.2".into()) }
        );
        assert_eq!(
            origin("https://example.com/focus.zeron-plugin", None, None).unwrap(),
            Origin::Link("https://example.com/focus.zeron-plugin".into())
        );
        assert_eq!(origin("https://git.example.com/focus.git", None, None).unwrap(), git("https://git.example.com/focus.git", None, None));
        for bad in ["http://example.com/x.zeron-plugin", "ssh://git@github.com/a/b", "ext::sh -c x", "", "/etc/passwd", "a b/c", "git@github.com:a/b"] {
            assert!(origin(bad, None, None).is_err(), "{bad}");
        }
        assert!(origin("ada/focus", Some("--upload-pack=x"), None).is_err());
        assert!(origin("ada/focus", None, Some("../up")).is_err());
    }

    #[test]
    fn a_check_finds_an_update_only_when_its_signer_is_the_same() {
        let (key, _) = SigningKey::generate().unwrap();
        let (eve, _) = SigningKey::generate().unwrap();
        let installed = parse(signed_package(&key, "focus", "1.0.0", &["ui.add"], "return 1")).unwrap();
        let source = Source { kind: "link".into(), url: Some("https://e/x.zeron-plugin".into()), path: None, git_ref: None, commit: None, version: "1.0.0".into(), installed_at: 0 };
        let perms = installed.manifest.permissions.clone();
        let record = crate::plugins::capability::record(&installed.manifest);
        let key_public = key.public();
        let was = Installed { id: "focus", key: &key_public, version: "1.0.0", permissions: &perms, hash: &installed.hash, allowed: &record };
        assert_eq!(compare(&was, &installed, source.clone(), false), None);
        let later = parse(signed_package(&key, "focus", "1.1.0", &["github"], "return 2")).unwrap();
        let found = compare(&was, &later, source.clone(), false).unwrap();
        assert_eq!((found.version.as_str(), found.added.clone(), found.removed.clone(), found.refused.clone()), ("1.1.0", vec!["github".to_string()], vec!["ui.add".to_string()], None));
        assert_eq!(found.digest_diff.unwrap()["permissions_added"], json!(["github"]));
        let theirs = parse(signed_package(&eve, "focus", "2.0.0", &[], "return 3")).unwrap();
        assert!(compare(&was, &theirs, source, false).unwrap().refused.is_some());
    }

    /// R25(d): a git install's `path` was joined to the clone as written,
    /// so a folder in the repository linked out of it packed this
    /// computer's files.
    #[cfg(unix)]
    #[test]
    fn a_git_plugin_folder_stays_inside_the_clone() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join(MANIFEST_FILE), b"{}").unwrap();
        let clone = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(clone.path().join("plugins/focus")).unwrap();
        std::fs::write(clone.path().join("plugins/focus").join(MANIFEST_FILE), b"{}").unwrap();
        std::os::unix::fs::symlink(outside.path(), clone.path().join("plugins/out")).unwrap();
        std::fs::create_dir_all(clone.path().join("plugins/linked")).unwrap();
        std::os::unix::fs::symlink(outside.path().join(MANIFEST_FILE), clone.path().join("plugins/linked").join(MANIFEST_FILE)).unwrap();

        let inside = plugin_folder(clone.path(), Some("plugins/focus")).unwrap();
        assert!(inside.ends_with("plugins/focus"));
        assert!(plugin_folder(clone.path(), Some("plugins/out")).unwrap_err().contains("isn't inside"));
        assert!(plugin_folder(clone.path(), Some("plugins/linked")).unwrap_err().contains("There's no zeron-plugin.json"));
        assert!(plugin_folder(clone.path(), Some("plugins/none")).unwrap_err().contains("There's no zeron-plugin.json"));
        assert!(plugin_folder(clone.path(), None).unwrap_err().contains("at the top"));
    }

    /// Install failures carry a reason the app says in your language; GitHub
    /// turning requests away reads as that, never as "can't be found".
    #[tokio::test]
    async fn failures_carry_their_reason() {
        assert_eq!(reason_of(WORDS.tampered), Some(reason::TAMPERED));
        assert_eq!(reason_of(&not_newer("1", "2")), Some(reason::NOT_NEWER));
        assert_eq!(reason_of("Its files don't match: x"), None, "English with a colon isn't a reason");
        assert_eq!(reason_of(&origin("ftp://x", None, None).unwrap_err()), Some(reason::HTTPS_ONLY));
        assert!(github_host("https://github.com/a/b/releases/download/v1/x.zeron-plugin"));
        assert!(github_host("https://objects.githubusercontent.com/x"));
        assert!(!github_host("https://github.com.evil.example/x") && !github_host("https://example.com/github.com"));

        // A server answering each request with `status`.
        async fn answering(status: u16) -> String {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let at = listener.local_addr().unwrap();
            tokio::spawn(async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    let mut buf = [0u8; 2048];
                    let _ = socket.read(&mut buf).await;
                    let reply = format!("HTTP/1.1 {status} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
                    let _ = socket.write_all(reply.as_bytes()).await;
                }
            });
            format!("http://{at}/x.zeron-plugin")
        }
        let limited = download(&answering(429).await).await.unwrap_err();
        assert_eq!(reason_of(&limited), Some(reason::RATE_LIMITED), "{limited}");
        // (A 403 from anywhere but GitHub is that server's no.)
        let forbidden = download(&answering(403).await).await.unwrap_err();
        assert_eq!(reason_of(&forbidden), Some(reason::DOWNLOAD_FAILED), "{forbidden}");
    }

    /// R23: an update was any package with another hash under the same key,
    /// so the author's key (or whoever holds it) could roll a plugin back to
    /// an older, vulnerable version.
    #[test]
    fn a_signed_downgrade_is_refused() {
        assert!(newer("1.1.0", "1.0.0") && newer("2", "1.9") && newer("v1.10", "1.9") && newer("1.0.0", "1.0.0-beta"));
        assert!(!newer("1.0.0", "1.0.0") && !newer("0.9", "1.0") && !newer("1", "1.0.0") && !newer("tomorrow", "1"));
        let dir = tempfile::tempdir().unwrap();
        let (key, _) = SigningKey::generate().unwrap();
        let source = Source { kind: "file".into(), url: None, path: None, git_ref: None, commit: None, version: String::new(), installed_at: 0 };
        let v2 = parse(signed_package(&key, "focus", "2.0.0", &[], "return 2")).unwrap();
        install(dir.path(), &v2, &source, None).unwrap();
        let v1 = parse(signed_package(&key, "focus", "1.0.0", &[], "return 1")).unwrap();
        assert!(install(dir.path(), &v1, &source, None).unwrap_err().contains("isn't later"));
        let same_version = parse(signed_package(&key, "focus", "2.0.0", &[], "return 666")).unwrap();
        assert!(install(dir.path(), &same_version, &source, None).unwrap_err().contains("isn't later"));
        // The same package again is no change.
        assert!(install(dir.path(), &v2, &source, None).is_ok());
        let perms = Vec::new();
        let record = crate::plugins::capability::record(&v2.manifest);
        let key_public = key.public();
        let was = Installed { id: "focus", key: &key_public, version: "2.0.0", permissions: &perms, hash: &v2.hash, allowed: &record };
        assert!(compare(&was, &v1, source, false).unwrap().refused.unwrap().contains("isn't later"));
        assert_eq!(scan::inspect(&dir.path().join("focus"), true).hash, v2.hash);
    }
}
