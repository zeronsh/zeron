//! `docs/plugin-package-vectors.json`: the cases the edge's TypeScript must
//! agree with, every expected value made by this crate's own code:
//! canonical JSON, content hashes, signatures, packages, path rules,
//! manifests, limits, strict versions and their order, "newer", ranges,
//! the store's listing rules, changelog notes, deploy keys and signed
//! requests, and what a plugin asks for as its consent sheet rates it
//! (the tables behind the ratings, as data the edge imports).
//!
//! Inputs come from two places: the cases written below, and `corpus.json`
//! (inputs only: the cases the edge lane's interim vectors carried). Nothing
//! expected is ever read from the corpus.
//!
//! JSON inputs are carried as text (`input`, `manifest`), so how text is
//! parsed is part of what's held: a lone surrogate or `1e400` is refused,
//! `1.0` stays a float and is written `1.0`, `1e16` is written `1e+16`.
//!
//! The file is checked in. `vectors_are_current` builds it again and fails
//! when it differs: a change to any of these rules shows up as a change to
//! the file, which the edge's tests read. `UPDATE_VECTORS=1 cargo test -p
//! zeron-engine vectors_are_current` writes it. Every key and signature
//! comes from a fixed seed, so the file is the same on every run.

use std::collections::BTreeMap;
use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};

use super::capability;
use super::install::{self, FORMAT};
use super::listing::{self, CATEGORIES};
use super::manifest::{self, Manifest};
use super::package::{self, MAX_PACKAGE_BYTES};
use super::signing::{self, FileHashes, PAYLOAD_PREFIX, SigCheck, SigningKey, TEMPLATE_PAYLOAD_PREFIX};
use super::store::deploy_key::{self, DeployKey};
use super::version::{self, Range, Version};

const CORPUS: &str = include_str!("corpus.json");

fn corpus() -> Value {
    serde_json::from_str(CORPUS).expect("corpus.json is JSON")
}

/// The key every signed case is signed with, and another for "someone else".
fn author() -> SigningKey {
    SigningKey::from_seed(&[1; 32]).unwrap()
}

fn other() -> SigningKey {
    SigningKey::from_seed(&[2; 32]).unwrap()
}

fn files(list: &[(&str, &[u8])]) -> BTreeMap<String, Vec<u8>> {
    list.iter().map(|(k, v)| ((*k).to_owned(), v.to_vec())).collect()
}

/// Runs shorter than this are written out.
const LONG_RUN: usize = 256;

/// `text` as a case carries it: as written, or, when it holds a long run of
/// one character (a size case's filler), `{parts: [before, {repeat, count},
/// after]}`, which expands to exactly `text`.
fn compact(text: &str) -> Value {
    let mut best: Option<(usize, usize, char, usize)> = None; // (start, end, char, count)
    let mut run: Option<(usize, char, usize)> = None;
    for (at, c) in text.char_indices() {
        run = match run {
            Some((start, prev, n)) if prev == c => Some((start, c, n + 1)),
            _ => Some((at, c, 1)),
        };
        let (start, c, n) = run.unwrap();
        if n >= LONG_RUN && best.is_none_or(|b| n > b.3) {
            best = Some((start, at + c.len_utf8(), c, n));
        }
    }
    match best {
        None => json!(text),
        Some((start, end, c, n)) => json!({ "parts": [&text[..start], { "repeat": c.to_string(), "count": n }, &text[end..]] }),
    }
}

/// Each file's bytes: base64, or (text with filler) `{parts}` of its text.
fn files_b64(files: &BTreeMap<String, Vec<u8>>) -> Value {
    Value::Object(
        files
            .iter()
            .map(|(k, v)| {
                let value = match std::str::from_utf8(v).map(compact) {
                    Ok(parts @ Value::Object(_)) => parts,
                    _ => json!(B64.encode(v)),
                };
                (k.clone(), value)
            })
            .collect(),
    )
}

/// `value` as text, as a case carries it.
fn text(value: &Value) -> String {
    serde_json::to_string(value).unwrap()
}

fn sig_word(check: &SigCheck) -> &'static str {
    match check {
        SigCheck::Valid(_) => "valid",
        SigCheck::Unsigned => "unsigned",
        SigCheck::FilesDiffer => "files_differ",
        SigCheck::BadSignature => "bad_signature",
    }
}

/// A plugin whose settings carry fractions: nothing on any path refuses one.
fn fractional() -> Value {
    json!({
        "id": "dimmer", "name": "Dimmer", "version": "1.0.0", "entry": "init.luau",
        "settings": [{ "key": "opacity", "type": "number", "default": 0.5, "min": 0.1, "max": 1.0, "step": 0.05 }],
    })
}

// ── Canonical JSON ──────────────────────────────────────────────────────────

fn canonical_cases() -> Value {
    let own: &[(&str, &str)] = &[
        ("keys sorted by UTF-8 bytes, not UTF-16 units", "{\"\u{e000}\":1,\"\u{1f600}\":2,\"Z\":3,\"a\":4,\"é\":5}"),
        ("strings escaped as JSON.stringify does", "{\"s\":\"quote \\\" backslash \\\\ slash \\/ newline \\n tab \\t nul \\u0000 unit sep \\u001f del \u{7f} line sep \u{2028} é 😀\"}"),
        ("fractional settings", &text(&fractional())),
        ("a float written 1.0 stays one", "{\"f\":1.0,\"i\":1}"),
        ("1e16 is written 1e+16", "1e16"),
        ("1e15 is written in full", "1e15"),
        ("1e-5 is written in full", "1e-5"),
        ("1e-6 is written 1e-6", "1e-6"),
        ("a big integer stays exact", "18446744073709551615"),
        ("an integer past u64 becomes a float", "18446744073709551616"),
        ("a half", "0.5"),
        ("a quarter", "0.25"),
        ("a lone surrogate is refused", "\"\\udc00\""),
        ("a number too big for a double is refused", "1e400"),
        ("a trailing comma is refused", "[1,]"),
        ("two documents are refused", "{} {}"),
    ];
    let mut cases: Vec<(String, String)> = own.iter().map(|(n, i)| ((*n).to_owned(), (*i).to_owned())).collect();
    // Nesting: serde_json refuses a document nested deeper than 128.
    for depth in DEPTHS {
        cases.push((format!("an array nested {depth} deep in an object"), nested_member(depth)));
        cases.push((format!("arrays nested {depth} deep"), format!("{}{}", "[".repeat(depth), "]".repeat(depth))));
    }
    for c in corpus()["canonical_json"].as_array().unwrap() {
        cases.push((c["name"].as_str().unwrap().to_owned(), c["input"].as_str().unwrap().to_owned()));
    }
    Value::Array(
        cases
            .into_iter()
            .map(|(name, input)| match serde_json::from_str::<Value>(&input) {
                Ok(value) => json!({ "name": name, "input": input, "canonical": String::from_utf8(signing::canonical_json(&value)).unwrap() }),
                Err(_) => json!({ "name": name, "input": input, "error": true }),
            })
            .collect(),
    )
}

/// Depths around serde_json's nesting limit: a document nests at most 127
/// containers deep, its outermost counted (so `{"x":` and 127 arrays is
/// already one too many).
const DEPTHS: [usize; 5] = [125, 126, 127, 128, 129];

/// `{"x":` + `depth` arrays + `}`.
fn nested_member(depth: usize) -> String {
    format!("{{\"x\":{}{}}}", "[".repeat(depth), "]".repeat(depth))
}

fn nested_value(depth: usize) -> Value {
    (0..depth).fold(json!([]), |inner, _| json!([inner])).as_array().unwrap()[0].clone()
}

// ── Content hashes ──────────────────────────────────────────────────────────

fn content_hash_cases() -> Value {
    let base = files(&[("init.luau", b"return 1"), ("ui/badge.luau", b"return {}")]);
    let manifest = json!({ "id": "focus", "name": "Focus", "version": "1.0.0", "entry": "init.luau", "permissions": ["ui.add"] });
    let signed = author().sign_manifest(&manifest, &package::hashes(&base));
    let mut cases: Vec<(String, String, FileHashes)> = vec![
        ("unsigned".into(), text(&manifest), package::hashes(&base)),
        ("signed: the signature isn't part of it".into(), text(&signed), package::hashes(&base)),
        ("a file changed".into(), text(&manifest), package::hashes(&files(&[("init.luau", b"return 2"), ("ui/badge.luau", b"return {}")]))),
        ("unknown fields are part of it".into(), { let mut m = manifest.clone(); m["x-future"] = json!({ "k": [1] }); text(&m) }, package::hashes(&base)),
        ("fractional settings".into(), text(&fractional()), package::hashes(&base)),
    ];
    for c in corpus()["content_hash"].as_array().unwrap() {
        let files: FileHashes = serde_json::from_value(c["files"].clone()).unwrap();
        cases.push((c["name"].as_str().unwrap().to_owned(), c["manifest"].as_str().unwrap().to_owned(), files));
    }
    Value::Array(
        cases
            .into_iter()
            .map(|(name, manifest, files)| {
                let parsed = Manifest::from_value(serde_json::from_str(&manifest).unwrap()).unwrap();
                json!({ "name": name, "manifest": manifest, "files": files, "hash": signing::content_hash(&parsed, &files) })
            })
            .collect(),
    )
}

// ── Signatures ──────────────────────────────────────────────────────────────

fn signature_cases() -> Value {
    let key = author();
    let base = files(&[("init.luau", b"return 1")]);
    let h = package::hashes(&base);
    let manifest = json!({ "id": "focus", "name": "Focus", "version": "1.0.0", "entry": "init.luau" });
    let signed = key.sign_manifest(&manifest, &h);
    let mut cases: Vec<(String, String, String, FileHashes)> = Vec::new();
    let mut push = |name: &str, prefix: &str, manifest: &Value, files: &FileHashes| cases.push((name.into(), prefix.into(), text(manifest), files.clone()));
    push("valid", PAYLOAD_PREFIX, &signed, &h);
    push("unsigned", PAYLOAD_PREFIX, &manifest, &h);
    push("an extra file", PAYLOAD_PREFIX, &signed, &package::hashes(&files(&[("init.luau", b"return 1"), ("evil.luau", b"x")])));
    push("a changed file", PAYLOAD_PREFIX, &signed, &package::hashes(&files(&[("init.luau", b"return 2")])));
    push("a missing file", PAYLOAD_PREFIX, &signed, &FileHashes::new());
    push("a template's signature isn't a plugin's", PAYLOAD_PREFIX, &key.sign_manifest_for(TEMPLATE_PAYLOAD_PREFIX, &manifest, &h), &h);
    push("a plugin's signature isn't a template's", TEMPLATE_PAYLOAD_PREFIX, &signed, &h);
    push("fractional settings, signed", PAYLOAD_PREFIX, &key.sign_manifest(&fractional(), &h), &h);
    let mut edited = signed.clone();
    edited["version"] = json!("2.0.0");
    push("a manifest field changed after signing", PAYLOAD_PREFIX, &edited, &h);
    let mut forged = signed.clone();
    forged["signature"]["sig"] = json!(key.sign(b"something else"));
    push("a signature over something else", PAYLOAD_PREFIX, &forged, &h);
    let mut swapped = signed.clone();
    swapped["signature"]["key"] = json!(other().public());
    push("another key named", PAYLOAD_PREFIX, &swapped, &h);
    let mut junk = signed.clone();
    junk["signature"] = json!("nope");
    push("a signature that isn't an object", PAYLOAD_PREFIX, &junk, &h);
    let mut padded = signed.clone();
    padded["signature"]["sig"] = json!(format!("{}==", signed["signature"]["sig"].as_str().unwrap()));
    push("base64url with padding isn't accepted", PAYLOAD_PREFIX, &padded, &h);
    for c in corpus()["signatures"].as_array().unwrap() {
        let files: FileHashes = serde_json::from_value(c["files"].clone()).unwrap();
        cases.push((c["name"].as_str().unwrap().into(), c["prefix"].as_str().unwrap().into(), c["manifest"].as_str().unwrap().into(), files));
    }
    Value::Array(
        cases
            .into_iter()
            .map(|(name, prefix, manifest, files)| {
                let value: Value = serde_json::from_str(&manifest).unwrap();
                let result = signing::check_raw(&prefix, &value, &files);
                let mut case = json!({ "name": name, "prefix": prefix, "manifest": manifest, "files": files, "result": sig_word(&result) });
                // The bytes the signature covers, for the key it names.
                if let Some(key) = value["signature"]["key"].as_str().filter(|_| value.is_object()) {
                    case["key"] = json!(key);
                    case["payload"] = json!(String::from_utf8(signing::payload_for(&prefix, &value, key, &files)).unwrap());
                }
                case
            })
            .collect(),
    )
}

// ── Manifests ───────────────────────────────────────────────────────────────

fn manifest_cases() -> Value {
    let mut cases: Vec<(String, String)> = vec![
        ("fractional settings".into(), text(&fractional())),
        ("listing fields of the wrong shape never break it".into(), text(&json!({ "id": "focus", "name": "F", "version": "1", "entry": "init.luau", "categories": "x", "keywords": 7, "engines": [] }))),
        // (A site id is the store's alone: as a manifest's id it's fine.)
        ("an id the store site keeps is a fine id here".into(), text(&json!({ "id": "all", "name": "All", "version": "1.0.0", "entry": "init.luau" }))),
    ];
    for c in corpus()["manifests"].as_array().unwrap() {
        cases.push((c["name"].as_str().unwrap().into(), c["manifest"].as_str().unwrap().into()));
    }
    Value::Array(
        cases
            .into_iter()
            .map(|(name, text)| {
                let m = Manifest::from_value(serde_json::from_str(&text).unwrap()).unwrap();
                json!({ "name": name, "manifest": text, "problem": m.problem(), "id_error": manifest::check_id(&m.id).err() })
            })
            .collect(),
    )
}

// ── Packages ────────────────────────────────────────────────────────────────

/// A package's raw bytes: `files` signed by `key` (none: unsigned).
fn packed(key: Option<&SigningKey>, manifest: &Value, files: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let manifest = match key {
        Some(key) => key.sign_manifest(manifest, &package::hashes(files)),
        None => manifest.clone(),
    };
    package::pack(FORMAT, &manifest, files).unwrap()
}

/// What a device makes of package `raw`: `{ok: true, id, version, key,
/// hash, sha256, files}`, or `{ok: false, error}`. `error` is the refusal's
/// word (`install::reason`), the first in this order: the package's own
/// checks (`not_a_package`, `too_big`, `bad_file`, the signature's), then
/// `paths_collide` (two paths one file on a folding disk, or the manifest's
/// name), then the manifest's: `bad_manifest` (one Zeron won't run),
/// `bad_id`, `missing_entry`.
fn package_result(raw: Vec<u8>) -> Value {
    let sha256 = signing::sha256_hex(&raw);
    match install::parse(raw.clone()) {
        Ok(p) => json!({
            "ok": true, "id": p.id(), "version": p.manifest.version, "key": p.key, "hash": p.hash, "sha256": sha256,
            "files": p.files.len(), "paths": p.files.keys().collect::<Vec<_>>(),
        }),
        Err(error) => {
            let word = install::reason_of(&error).map(str::to_owned).unwrap_or_else(|| {
                let value: Value = serde_json::from_slice(&raw).unwrap();
                let m = Manifest::from_value(value["manifest"].clone()).unwrap();
                if m.problem() == Some(error.as_str()) {
                    "bad_manifest".into()
                } else if manifest::check_id(&m.id).err().as_deref() == Some(error.as_str()) {
                    "bad_id".into()
                } else if error.starts_with("Its entry file") {
                    "missing_entry".into()
                } else {
                    panic!("a refusal without a word: {error}")
                }
            });
            json!({ "ok": false, "error": word })
        }
    }
}

fn package_cases() -> Value {
    let key = author();
    let manifest = json!({ "id": "focus", "name": "Focus", "version": "1.0.0", "entry": "init.luau", "permissions": ["ui.add"] });
    let good = files(&[("init.luau", b"return 1"), ("ui/badge.luau", b"return {}")]);
    let valid = packed(Some(&key), &manifest, &good);
    let edit = |change: &dyn Fn(&mut Value)| {
        let mut value: Value = serde_json::from_slice(&valid).unwrap();
        change(&mut value);
        serde_json::to_vec(&value).unwrap()
    };
    let mut cases: Vec<(String, Vec<u8>)> = vec![
        ("valid".into(), valid.clone()),
        ("not JSON".into(), b"PK\x03\x04".to_vec()),
        ("another format".into(), edit(&|v| v["format"] = json!("zeron-template:v1"))),
        ("files isn't an object".into(), edit(&|v| v["files"] = json!([]))),
        ("unsigned".into(), packed(None, &manifest, &good)),
        ("a file changed after signing".into(), edit(&|v| v["files"]["init.luau"] = json!(B64.encode(b"return 2")))),
        ("a file added after signing".into(), edit(&|v| v["files"]["extra.luau"] = json!(B64.encode(b"x")))),
        ("a manifest field changed after signing".into(), edit(&|v| v["manifest"]["permissions"] = json!(["ui.add", "network"]))),
        ("a file that isn't base64".into(), edit(&|v| v["files"]["init.luau"] = json!("@@@"))),
        ("fractional settings".into(), packed(Some(&key), &fractional(), &good)),
    ];
    for (name, bad) in [
        ("a path out of the folder", "../evil.luau"),
        ("an absolute path", "/etc/x"),
        ("a dot folder", ".git/config"),
        ("a drive-relative path", "C:x.cmd"),
        ("a Windows device name", "ui/NUL.luau"),
        ("a trailing dot", "init.luau."),
        ("a backslash", "ui\\x.luau"),
        ("its own manifest as a file", "zeron-plugin.json"),
    ] {
        let mut with = good.clone();
        with.insert(bad.into(), b"x".to_vec());
        cases.push((name.into(), packed(Some(&key), &manifest, &with)));
    }
    // Paths one disk or another makes one file (macOS's and Windows' fold
    // names: listing.rs `fold_path`): refused as read, on every disk alike.
    for (name, paths) in [
        ("two paths that differ only in case", &["ui/Badge.luau", "ui/badge.luau"][..]),
        ("one name in Unicode's two spellings (NFC, NFD)", &["caf\u{e9}.luau", "cafe\u{301}.luau"]),
        ("a file and a folder that fold alike", &["ui", "UI/x.luau"]),
        ("its manifest's name in capitals", &["Zeron-Plugin.json"]),
        ("its manifest's name with a long s", &["zeron-plugin.j\u{17f}on"]),
        ("its manifest's name as a folder", &["ZERON-PLUGIN.JSON/x.luau"]),
    ] {
        let mut with = good.clone();
        for rel in paths {
            with.insert((*rel).into(), b"x".to_vec());
        }
        cases.push((name.into(), packed(Some(&key), &manifest, &with)));
    }
    let mut no_entry = good.clone();
    no_entry.remove("init.luau");
    cases.push(("its entry file is missing".into(), packed(Some(&key), &manifest, &no_entry)));
    for (name, field, value) in [
        ("a permission Zeron doesn't know", "permissions", json!(["root"])),
        ("an id Zeron keeps", "id", json!("zeron")),
        ("an id that isn't one", "id", json!("Focus")),
        ("unknown fields are kept", "x-future", json!({ "unknown": "fields are kept and signed" })),
    ] {
        let mut m = manifest.clone();
        m[field] = value;
        cases.push((name.into(), packed(Some(&key), &m, &good)));
    }
    // Nesting around serde_json's limit, in each of three places of a
    // validly signed package: an unknown top-level member, inside its
    // manifest (signed with it), inside a `files` value.
    let valid_text = String::from_utf8(valid.clone()).unwrap();
    for depth in DEPTHS {
        let nest = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        assert!(valid_text.starts_with("{\"files\":{"));
        cases.push((format!("an unknown member nested {depth} deep"), format!("{{\"x\":{nest},{}", &valid_text[1..]).into_bytes()));
        let mut deep = manifest.clone();
        deep["x"] = nested_value(depth);
        cases.push((format!("a manifest member nested {depth} deep"), packed(Some(&key), &deep, &good)));
        cases.push((format!("a files value nested {depth} deep"), valid_text.replacen("{\"files\":{", &format!("{{\"files\":{{\"zz\":{nest},"), 1).into_bytes()));
    }
    let mut out: Vec<Value> = Vec::new();
    for c in corpus()["packages"].as_array().unwrap() {
        cases.push((c["name"].as_str().unwrap().into(), B64.decode(c["package_b64"].as_str().unwrap()).unwrap()));
    }
    for (name, raw) in cases {
        out.push(json!({ "name": name, "package_b64": B64.encode(&raw), "result": package_result(raw) }));
    }
    // 2,001 copies of one 6,200-character file key (JSON keeps the last of
    // a duplicate key): written as parts, a segment repeated.
    let long_key = "a".repeat(6200);
    let segment = format!("\"{long_key}\":\"eA==\",");
    let tail = format!("\"{long_key}\":\"eA==\"}},\"format\":\"{FORMAT}\",\"manifest\":{}}}", text(&key.sign_manifest(&manifest, &package::hashes(&good))));
    let head = "{\"files\":{";
    let raw = format!("{head}{}{tail}", segment.repeat(2000)).into_bytes();
    out.push(json!({
        "name": "2,001 copies of a 6,200-character file key",
        "package_parts": [head, { "repeat": segment, "count": 2000 }, tail],
        "result": package_result(raw),
    }));
    Value::Array(out)
}

fn path_cases() -> Value {
    let own = [
        "init.luau", "ui/badge.luau", "shots/board.png", "a-b_c.d/e", "README.md", "", "/etc/x", "../x", "a/../b", "a/./b", ".git/config", "a/.hidden",
        "a//b", "a/", "a\\b", "D:x.cmd", "C:../x", "CON", "con.txt", "com1.luau", "LPT9", "ui/NUL.luau", "AUX", "x.", "a. ", "a :b", "x:y",
    ];
    let mut paths: Vec<String> = own.iter().map(|p| (*p).to_owned()).collect();
    for p in corpus()["paths"].as_array().unwrap() {
        let p = p.as_str().unwrap().to_owned();
        if !paths.contains(&p) {
            paths.push(p);
        }
    }
    Value::Array(paths.iter().map(|p| json!({ "path": p, "ok": package::plain_path(p) })).collect())
}

// ── Versions ────────────────────────────────────────────────────────────────

fn semver_cases() -> Value {
    let own = [
        "0.0.0", "1.2.3", "1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-0.3.7", "1.0.0-x.7.z.92", "1.0.0-x-y-z.--", "1.0.0+20130313144700",
        "1.0.0-beta+exp.sha.5114f85", "1.0.0+21AF26D3----117B344092BD", "1.0.0+001", "9007199254740991.9007199254740991.9007199254740991",
        "", "1", "1.2", "1.2.3.4", "v1.2.3", " 1.2.3", "1.2.3 ", "01.2.3", "1.02.3", "1.2.03", "1.2.3-", "1.2.3+", "1.2.3-01",
        "1.2.3-a..b", "1.2.3-a_b", "1.2.3+a..b", "1.2.3-é", "9007199254740992.0.0", "1.0.0-9007199254740992", "-1.0.0", "1.0.0++a", "1.0.0-+a",
    ];
    let mut inputs: Vec<String> = own.iter().map(|s| (*s).to_owned()).collect();
    inputs.push(format!("1.0.0-{}", "a".repeat(version::MAX_LEN - 6)));
    inputs.push(format!("1.0.0-{}", "a".repeat(version::MAX_LEN - 5)));
    let corpus = corpus();
    for i in corpus["semver"]["parse"].as_array().unwrap() {
        let i = i.as_str().unwrap().to_owned();
        if !inputs.contains(&i) {
            inputs.push(i);
        }
    }
    let parse: Vec<Value> = inputs
        .iter()
        .map(|input| match Version::parse(input) {
            Ok(v) => json!({
                "input": input, "ok": true, "major": v.major, "minor": v.minor, "patch": v.patch,
                "pre": v.pre.iter().map(ToString::to_string).collect::<Vec<_>>().join("."), "build": v.build.join("."),
            }),
            Err(_) => json!({ "input": input, "ok": false }),
        })
        .collect();
    let mut order: Vec<String> = [
        "0.0.1", "0.1.0", "1.0.0-0", "1.0.0-1", "1.0.0-2", "1.0.0-10", "1.0.0-A", "1.0.0-a", "1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta",
        "1.0.0-beta", "1.0.0-beta.2", "1.0.0-beta.11", "1.0.0-rc.1", "1.0.0", "1.0.1", "1.1.0", "2.0.0", "10.0.0",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    for pair in order.windows(2) {
        assert!(Version::parse(&pair[0]).unwrap() < Version::parse(&pair[1]).unwrap(), "{pair:?}");
    }
    // The corpus's own order, its strict versions only, as a second list.
    let corpus_order: Vec<String> = corpus["semver"]["order"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str().filter(|v| Version::parse(v).is_ok()).map(str::to_owned))
        .collect();
    for pair in corpus_order.windows(2) {
        assert!(Version::parse(&pair[0]).unwrap() < Version::parse(&pair[1]).unwrap(), "{pair:?}");
    }
    let mut pairs: Vec<(String, String)> = [
        ("1.0.0+a", "1.0.0+b"),
        ("1.0.0", "1.0.0+b"),
        ("1.0.0-rc.1+build.1", "1.0.0-rc.1+build.2"),
        ("1.0.0-a.1", "1.0.0-a.1.0"),
        ("1.0.0-1", "1.0.0-a"),
        ("2.0.0", "10.0.0"),
        ("1.0.0-rc.1", "1.0.0"),
    ]
    .iter()
    .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
    .collect();
    for p in corpus["semver"]["compare"].as_array().unwrap() {
        pairs.push((p[0].as_str().unwrap().to_owned(), p[1].as_str().unwrap().to_owned()));
    }
    let compare: Vec<Value> = pairs
        .iter()
        .filter_map(|(a, b)| {
            let (x, y) = (Version::parse(a).ok()?, Version::parse(b).ok()?);
            Some(json!({ "a": a, "b": b, "cmp": x.cmp(&y) as i8 }))
        })
        .collect();
    order.dedup();
    json!({ "parse": parse, "order": order, "corpus_order": corpus_order, "compare": compare, "max_len": version::MAX_LEN, "max_number": version::MAX_NUMBER })
}

/// The store's "newer": by precedence, build metadata ignored, so a version
/// differing only in its build is the same version (`version_exists`).
fn newer_cases() -> Value {
    let pairs = [
        ("1.0.1", "1.0.0"),
        ("1.0.0", "1.0.0"),
        ("1.0.0+b", "1.0.0+a"),
        ("1.0.0+a", "1.0.0"),
        ("1.0.0", "1.0.0-rc.1"),
        ("1.0.0-rc.2", "1.0.0-rc.1"),
        ("1.0.0-rc.1", "1.0.0"),
        ("0.9.9", "1.0.0"),
        ("1.10.0", "1.9.0"),
    ];
    Value::Array(
        pairs
            .iter()
            .map(|(candidate, installed)| {
                let newer = version::newer(&Version::parse(candidate).unwrap(), &Version::parse(installed).unwrap());
                json!({ "candidate": candidate, "installed": installed, "newer": newer })
            })
            .collect(),
    )
}

fn range_cases() -> Value {
    let cases: &[(&str, &[&str])] = &[
        (">=0.4.0", &["0.4.0", "0.3.9", "0.4.0-beta.1", "1.0.0"]),
        (">= 0.4.0 <0.6.0", &["0.5.9", "0.6.0", "0.6.0-0", "0.4.0"]),
        (">0.4.0", &["0.4.0", "0.4.1-0"]),
        ("<=1.0.0", &["1.0.0", "1.0.0+b", "1.0.1-0"]),
        ("<1.0.0", &["0.9.9", "1.0.0-rc.1", "1.0.0"]),
        ("^1.2.3", &["1.2.3", "1.9.0", "2.0.0-alpha", "2.0.0", "1.2.2"]),
        ("^0.2.3", &["0.2.9", "0.3.0", "0.3.0-0", "0.2.2"]),
        ("^0.0.3", &["0.0.3", "0.0.4", "0.0.4-0"]),
        ("^1.2.3-beta.2", &["1.2.3-beta.2", "1.2.3-beta.1", "1.2.3", "1.3.0-alpha"]),
        ("~1.2.3", &["1.2.9", "1.3.0", "1.3.0-rc.1", "1.2.3-rc.1"]),
        ("~0.0.3", &["0.0.9", "0.1.0"]),
        ("1.2.3", &["1.2.3", "1.2.3+build", "1.2.4"]),
        ("=1.2.3", &["1.2.3"]),
        ("*", &["0.0.1-alpha", "99.0.0"]),
        ("<0.3.0 || >=0.5.0", &["0.4.0", "0.5.1", "0.2.0"]),
        ("<0.3.0 || *", &["9.9.9"]),
        ("  >=1.0.0   <2.0.0  ", &["1.5.0"]),
        (">=1.0.0 <1.0.0", &["1.0.0"]),
        // Amendment 32: by precedence, with no npm-style pre-release
        // exclusion: a beta of a Zeron the range accepts runs the plugin.
        (">=1.0.0", &["2.0.0-beta", "1.0.1-rc.1"]),
        ("^0.4.0", &["0.4.5-beta.2"]),
    ];
    let mut out = Vec::new();
    for (range, versions) in cases {
        let parsed = Range::parse(range).unwrap();
        for v in *versions {
            out.push(json!({ "range": range, "version": v, "matches": parsed.matches(&Version::parse(v).unwrap()) }));
        }
    }
    let long = format!(">=1.0.0{}", " ".repeat(version::MAX_RANGE_LEN));
    for bad in ["", " ", "1.2", ">=", ">= ", "1.x", "1.2.3 - 2.0.0", "||", ">=1.0.0 ||", "^v1.0.0", ">=1.0.0\t<2.0.0", ">> 1.0.0", "* 1.0.0", ">=*", "latest", "~>1.0.0", &long] {
        assert!(Range::parse(bad).is_err(), "{bad:?}");
        out.push(json!({ "range": bad, "error": true }));
    }
    Value::Array(out)
}

// ── The store's listing rules ───────────────────────────────────────────────

fn listing_cases() -> Value {
    let (base, files) = listing::tests::listed();
    let mut cases: Vec<(String, Value, BTreeMap<String, Vec<u8>>)> = vec![
        ("meets every rule".into(), base.clone(), files.clone()),
        ("fractional settings meet them too".into(), { let mut m = base.clone(); m["settings"] = fractional()["settings"].clone(); m }, files.clone()),
    ];
    let changes: &[(&str, &str, Value)] = &[
        ("no category", "categories", Value::Null),
        ("an empty category list", "categories", json!([])),
        ("a category not on the list", "categories", json!(["games"])),
        ("three categories", "categories", json!(["git", "chat", "agents"])),
        ("a category twice", "categories", json!(["git", "git"])),
        ("a keyword in capitals", "keywords", json!(["Focus"])),
        ("nine keywords", "keywords", json!(["a", "b", "c", "d", "e", "f", "g", "h", "i"])),
        ("keywords with spaces and dashes", "keywords", json!(["code review", "git-flow"])),
        ("a keyword with two spaces", "keywords", json!(["code  review"])),
        ("an http homepage", "homepage", json!("http://example.com")),
        ("a repository with a space", "repository", json!("https://exa mple.com")),
        ("a homepage of 2048 bytes", "homepage", json!(format!("https://example.com/{}", "a".repeat(2048 - 20)))),
        ("a homepage of 2049 bytes", "homepage", json!(format!("https://example.com/{}", "a".repeat(2049 - 20)))),
        ("an empty license", "license", json!("")),
        ("a screenshot that isn't there", "screenshots", json!(["shots/missing.png"])),
        ("a screenshot that isn't a picture", "screenshots", json!(["init.luau"])),
        ("a screenshot outside the folder", "screenshots", json!(["../x.png"])),
        ("a partial engines range", "engines", json!({ "zeron": "0.4" })),
        ("engines as text", "engines", json!(">=0.4.0")),
        ("an icon that isn't an SVG", "icon", json!("README.md")),
        ("no icon", "icon", Value::Null),
        ("no description", "description", Value::Null),
        ("a description of two lines", "description", json!("a\nb")),
        ("a description of 300 characters", "description", json!("d".repeat(300))),
        ("a description of 301 characters", "description", json!("d".repeat(301))),
        ("a name of 65 characters", "name", json!("n".repeat(65))),
        ("a loose version", "version", json!("1.2")),
        ("an id starting zeron", "id", json!("zeron-tools")),
        ("a built-in plugin's id", "id", json!("github")),
        ("an id the site's pages use: all", "id", json!("all")),
        ("an id the site's pages use: publishing", "id", json!("publishing")),
        ("an id the site's pages use: featured", "id", json!("featured")),
        ("an id the site's pages use: feed", "id", json!("feed")),
        ("an id the site's pages use: admin", "id", json!("admin")),
        ("an id with a site word in it is anyone's", "id", json!("feed-reader")),
        ("an id a site word is in is anyone's", "id", json!("allergies")),
        ("a permission Zeron doesn't know", "permissions", json!(["root"])),
        ("a name with a direction override", "name", json!("Focus\u{202e}draoB")),
        ("a name with a zero-width space", "name", json!("Focus\u{200b}Board")),
        ("a description with a direction isolate", "description", json!("A board\u{2066}.")),
        ("a homepage without its path's slash (kept with it)", "homepage", json!("https://example.com")),
        ("a homepage with a capital host and a dot segment (kept parsed)", "homepage", json!("https://Example.com/x/../y")),
        ("a homepage written with a backslash", "homepage", json!("https:\\evil.com")),
        ("a homepage with a user and password", "homepage", json!("https://user:pw@example.com/")),
        ("a homepage with a space", "homepage", json!("https://exa mple.com/")),
        ("a repository with a query and a fragment", "repository", json!("https://github.com/a/b?x=1#y")),
    ];
    for (name, key, value) in changes {
        let mut m = base.clone();
        if value.is_null() {
            m.as_object_mut().unwrap().remove(*key);
        } else {
            m[*key] = value.clone();
        }
        cases.push(((*name).into(), m, files.clone()));
    }
    let with_file = |path: &str, bytes: Vec<u8>| {
        let mut f = files.clone();
        f.insert(path.into(), bytes);
        f
    };
    let mut no_readme = files.clone();
    no_readme.remove(listing::README);
    cases.push(("no README.md".into(), base.clone(), no_readme));
    cases.push(("a WebP screenshot".into(), { let mut m = base.clone(); m["screenshots"] = json!(["shots/a.webp"]); m }, with_file("shots/a.webp", b"RIFF\0\0\0\0WEBPVP8 ".to_vec())));
    // Each size limit, one byte under and one over (written as `parts`).
    for (name, bytes) in [("README.md at 512 KB", listing::MAX_README_BYTES), ("README.md one byte over 512 KB", listing::MAX_README_BYTES + 1)] {
        cases.push((name.into(), base.clone(), with_file(listing::README, vec![b'x'; bytes])));
    }
    let older = "## 1.2.0\n\n- Boards.\n\n## 0.1.0\n\n";
    for (name, bytes) in [("CHANGELOG.md at 512 KB", listing::MAX_CHANGELOG_BYTES), ("CHANGELOG.md one byte over 512 KB", listing::MAX_CHANGELOG_BYTES + 1)] {
        cases.push((name.into(), base.clone(), with_file(listing::CHANGELOG, format!("{older}{}", "x".repeat(bytes - older.len())).into_bytes())));
    }
    for (name, bytes) in [("notes at 64 KB", listing::MAX_NOTES_BYTES), ("notes one byte over 64 KB", listing::MAX_NOTES_BYTES + 1)] {
        cases.push((name.into(), base.clone(), with_file(listing::CHANGELOG, format!("## 1.2.0\n\n{}\n", "n".repeat(bytes)).into_bytes())));
    }
    let mut empty = base.clone();
    empty["x-notes"] = json!("");
    let around = signing::canonical_json(&empty).len();
    for (name, bytes) in [("a manifest at 256 KB", listing::MAX_MANIFEST_BYTES), ("a manifest one byte over 256 KB", listing::MAX_MANIFEST_BYTES + 1)] {
        let mut m = base.clone();
        m["x-notes"] = json!("x".repeat(bytes - around));
        assert_eq!(signing::canonical_json(&m).len(), bytes);
        cases.push((name.into(), m, files.clone()));
    }
    let svg = |bytes: usize| {
        let open = "<svg xmlns=\"http://www.w3.org/2000/svg\"><!--";
        let close = "--></svg>";
        format!("{open}{}{close}", "x".repeat(bytes - open.len() - close.len())).into_bytes()
    };
    for (name, bytes) in [("an icon at 32 KB", listing::MAX_ICON_BYTES), ("an icon one byte over 32 KB", listing::MAX_ICON_BYTES + 1)] {
        cases.push((name.into(), base.clone(), with_file("icon.svg", svg(bytes))));
    }
    for (name, field, value) in [
        ("a name of 64 characters", "name", json!("n".repeat(listing::MAX_NAME_LEN))),
        ("a keyword of 32 characters", "keywords", json!(["k".repeat(listing::MAX_KEYWORD_LEN)])),
        ("a keyword of 33 characters", "keywords", json!(["k".repeat(listing::MAX_KEYWORD_LEN + 1)])),
        ("a license of 64 characters", "license", json!("L".repeat(listing::MAX_LICENSE_LEN))),
        ("a license of 65 characters", "license", json!("L".repeat(listing::MAX_LICENSE_LEN + 1))),
    ] {
        let mut m = base.clone();
        m[field] = value;
        cases.push((name.into(), m, files.clone()));
    }
    // The store's mark is its alone (amendment 45): a name or description
    // that claims it is refused, read as NFKC then lowercase, words whole.
    for (name, field, value) in [
        ("the mark: a name saying Verified", "name", "Verified Focus"),
        ("the mark: a name saying OFFICIAL", "name", "OFFICIAL Focus"),
        ("the mark: a name with a check mark", "name", "Focus \u{2714}"),
        ("the mark: a name with a check mark emoji", "name", "\u{2705} Focus"),
        ("the mark: a name saying verified in mathematical bold", "name", "\u{1d42f}\u{1d41e}\u{1d42b}\u{1d422}\u{1d41f}\u{1d422}\u{1d41e}\u{1d41d} Focus"),
        ("the mark: a name saying verified in fullwidth letters", "name", "Focus \u{ff36}\u{ff25}\u{ff32}\u{ff29}\u{ff26}\u{ff29}\u{ff25}\u{ff24}"),
        ("the mark: a name with verified before an underscore", "name", "verified_focus"),
        ("the mark: a description saying official plugin", "description", "The official plugin for focus."),
        ("the mark: a description with a ballot box check", "description", "Boards \u{2611} done."),
        ("the mark: a description saying verified in capital I with a dot", "description", "VER\u{130}F\u{130}ED boards."),
        ("the mark: a name saying unofficial is fine", "name", "Unofficial Focus"),
        ("the mark: a name saying unverified is fine", "name", "Unverified Links"),
        ("the mark: a description saying officially is fine", "description", "Officially the best board."),
        ("the mark: a description saying verifiedx is fine", "description", "Boards by verifiedx."),
    ] {
        let mut m = base.clone();
        m[field] = json!(value);
        cases.push((name.into(), m, files.clone()));
    }
    // Spelled so it reads as the mark at a glance (the edge's attack cases,
    // each found by the identity lane): look-alike letters from UTS #39,
    // small capitals, digits, i and l for each other, cl for d, letters
    // spaced or dotted apart, characters drawn as nothing between letters.
    for (name, field, value) in [
        ("the mark: Cyrillic letters", "name", "V\u{435}r\u{456}f\u{456}\u{435}d Focus"),
        ("the mark: Cyrillic capitals", "name", "V\u{415}RIFI\u{415}D"),
        ("the mark: Cyrillic letters in official", "description", "The \u{43e}ffi\u{441}i\u{430}l plugin."),
        ("the mark: Cyrillic palochka for l", "description", "\u{43e}fficia\u{4cf} tools."),
        ("the mark: Cyrillic izhitsa for v", "name", "\u{475}erified"),
        ("the mark: Greek nu for v", "name", "\u{3bd}erified"),
        ("the mark: Greek letters", "description", "v\u{3b5}r\u{3b9}f\u{3b9}\u{3b5}d builds."),
        ("the mark: Greek capitals", "name", "\u{39f}FF\u{399}CIAL"),
        ("the mark: small capitals", "name", "\u{1d20}\u{1d07}\u{280}\u{26a}\u{a730}\u{26a}\u{1d07}\u{1d05} Focus"),
        ("the mark: Lisu capitals for verified", "name", "\u{a4e6}\u{a4f0}\u{a4e3}\u{a4f2}\u{a4dd}\u{a4f2}\u{a4f0}\u{a4d3} Focus"),
        ("the mark: Lisu capitals for official", "name", "\u{a4f3}\u{a4dd}\u{a4dd}\u{a4f2}\u{a4da}\u{a4f2}\u{a4ee}\u{a4e1} Git"),
        ("the mark: a small capital I", "name", "Ver\u{26a}f\u{26a}ed"),
        ("the mark: digits for I", "name", "VER1F1ED"),
        ("the mark: a digit 0 for O", "name", "0FFICIAL tools"),
        ("the mark: a digit 3 for e", "name", "v3rified"),
        ("the mark: a digit 1 for l", "name", "officia1"),
        ("the mark: a square root as a tick", "name", "Focus \u{221a}"),
        ("the mark: the Aegean check mark", "name", "Focus \u{10102}"),
        ("the mark: the inverse check mark", "name", "Focus \u{1fbb1}"),
        ("the mark: lowercase l for I in capitals", "name", "VERlFlED Focus"),
        ("the mark: lowercase l for I in OFFICIAL", "name", "OFFlClAL Git"),
        ("the mark: capital I for l in lowercase", "description", "The officiaI client."),
        ("the mark: cl for d", "name", "verifiecl tools"),
        ("the mark: letter-spaced", "name", "V E R I F I E D"),
        ("the mark: dotted capitals", "name", "V.E.R.I.F.I.E.D. Focus"),
        ("the mark: dot and space", "name", "V. E. R. I. F. I. E. D. Focus"),
        ("the mark: middle dots", "name", "V\u{b7}E\u{b7}R\u{b7}I\u{b7}F\u{b7}I\u{b7}E\u{b7}D"),
        ("the mark: en dashes", "name", "V\u{2013}E\u{2013}R\u{2013}I\u{2013}F\u{2013}I\u{2013}E\u{2013}D"),
        ("the mark: two spaces", "name", "V  E  R  I  F  I  E  D"),
        ("the mark: no-break spaces", "name", "V\u{a0}E\u{a0}R\u{a0}I\u{a0}F\u{a0}I\u{a0}E\u{a0}D"),
        ("the mark: ideographic spaces", "name", "V\u{3000}E\u{3000}R\u{3000}I\u{3000}F\u{3000}I\u{3000}E\u{3000}D"),
        ("the mark: spaced look-alikes", "name", "V E R \u{406} F \u{406} E D"),
        ("the mark: a word joiner between letters", "description", "Veri\u{2060}fied boards."),
        ("the mark: an enclosing keycap mark between letters", "name", "Veri\u{20e3}fied"),
        ("the mark: an enclosing circle between letters", "description", "Veri\u{20dd}fied boards."),
        ("the mark: negative circled capitals", "name", "\u{1f165}\u{1f154}\u{1f161}\u{1f158}\u{1f155}\u{1f158}\u{1f154}\u{1f153} Focus"),
        ("the mark: negative squared capitals", "name", "\u{1f185}\u{1f174}\u{1f181}\u{1f178}\u{1f175}\u{1f178}\u{1f174}\u{1f173} Focus"),
        ("the mark: spaced regional indicators", "name", "\u{1f1fb} \u{1f1ea} \u{1f1f7} \u{1f1ee} \u{1f1eb} \u{1f1ee} \u{1f1ea} \u{1f1e9}"),
    ] {
        let mut m = base.clone();
        m[field] = json!(value);
        cases.push((name.into(), m, files.clone()));
    }
    // Honest words, in those alphabets and mixed: nothing claimed.
    for (name, value) in [
        ("the mark: unverified with a Cyrillic e is fine", "Unv\u{435}rified Links"),
        ("the mark: officer with a Greek o is fine", "For every \u{3bf}fficer"),
        ("the mark: officially with a Cyrillic i is fine", "Off\u{456}cially the best"),
        ("the mark: Greek for official is fine", "\u{395}\u{3c0}\u{3af}\u{3c3}\u{3b7}\u{3bc}\u{3b7} \u{3b5}\u{3c6}\u{3b1}\u{3c1}\u{3bc}\u{3bf}\u{3b3}\u{3ae}"),
        ("the mark: verification is fine", "verification"),
        ("the mark: officialdom is fine", "officialdom"),
        ("the mark: a version and verifier are fine", "v1.0 verifier"),
        ("the mark: 100 percent is fine", "100% local"),
        ("the mark: verifiedly 3D is fine", "Verifiedly 3D"),
        ("the mark: A B testing is fine", "A B testing"),
        ("the mark: U.S. weather is fine", "U.S. weather"),
        ("the mark: V I M keys are fine", "V I M keys"),
        ("the mark: initials are fine", "J. R. R. Tolkien notes"),
        ("the mark: R.E.M. with middle dots is fine", "R\u{b7}E\u{b7}M songs"),
        ("the mark: oil fields are fine", "Oil fields"),
        ("the mark: civil filed is fine", "Civil filed"),
        ("the mark: an emoji with its variation selector is fine", "Stars \u{2b50}\u{fe0f}"),
        ("the mark: a keycap is fine", "Keys 1\u{fe0f}\u{20e3}"),
        ("the mark: a keycap in a name is fine", "Top 1\u{fe0f}\u{20e3} timer"),
        ("the mark: a ballot box with an X is fine", "Tasks \u{2612}"),
    ] {
        let mut m = base.clone();
        m["name"] = json!(value);
        cases.push((name.into(), m, files.clone()));
    }
    // Drawn as nothing: refused in a name or description at all (only an
    // emoji's own U+FE0F, right after it, is seen).
    for (name, field, value) in [
        ("unseen: U+FE0F after a letter", "name", "Focus\u{fe0f}"),
        ("unseen: U+FE0F twice after an emoji", "name", "Stars \u{2b50}\u{fe0f}\u{fe0f}"),
        ("unseen: text-style selector U+FE0E", "name", "Stars \u{2b50}\u{fe0e}"),
        ("unseen: variation selector 1", "description", "Focus\u{fe00} boards."),
        ("unseen: variation selector 17", "name", "Focus\u{e0100}"),
        ("unseen: Hangul filler", "name", "Focus\u{3164}Board"),
        ("unseen: Hangul choseong filler", "description", "Focus\u{115f}Board."),
        ("unseen: halfwidth Hangul filler", "name", "Focus\u{ffa0}Board"),
        ("unseen: U+FE0F at the start", "name", "\u{fe0f}Focus"),
        ("unseen: U+FE0E after an emoji in a description", "description", "Focus \u{2b50}\u{fe0e}"),
        ("unseen: a zero-width joiner sequence", "name", "\u{1f469}\u{200d}\u{1f4bb} Dev"),
        ("unseen: a tag sequence flag", "name", "\u{1f3f4}\u{e0067}\u{e0062}\u{e0073}\u{e0063}\u{e0074}\u{e007f} Focus"),
    ] {
        let mut m = base.clone();
        m[field] = json!(value);
        cases.push((name.into(), m, files.clone()));
    }
    let mut m = base.clone();
    m["keywords"] = json!(["timer\u{fe0f}"]);
    cases.push(("unseen: a variation selector in a keyword".into(), m, files.clone()));
    // Saying it's Zeron's own: a warning on a device (it can't know the
    // publisher); the edge refuses it, claims_zeron, unless the publisher is
    // Zeron's (maintainer). In a name, "zeron" only as what it's for; in a
    // description, not after a word of origin nor beside one of standing.
    for (name, field, value) in [
        ("claims_zeron: a name leading with Zeron", "name", "Zeron Git"),
        ("claims_zeron: a name saying Zeron Labs", "name", "Zeron Labs notes"),
        ("claims_zeron: a name saying Team Zeron", "name", "Team Zeron"),
        ("claims_zeron: a name saying Zeron's", "name", "Zeron's git"),
        ("claims_zeron: a name saying Zeron-made", "name", "Zeron-made boards"),
        ("claims_zeron: a name saying by Zeron", "name", "Notes, by Zeron"),
        ("claims_zeron: a name with a digit 0 for o", "name", "Git by Zer0n"),
        ("claims_zeron: a name with a Cyrillic e", "name", "Git by Z\u{435}ron"),
        ("claims_zeron: a name in small capitals", "name", "Git by \u{1d22}\u{1d07}\u{280}\u{1d0f}\u{274}"),
        ("claims_zeron: a name with a Cyrillic u for y", "name", "Git b\u{443} Zeron"),
        ("claims_zeron: a name in dotted capitals", "name", "Z.E.R.O.N. approved"),
        ("claims_zeron: a name spaced", "name", "z e r o n approved"),
        ("claims_zeron: a name spaced after by", "name", "Git by Z e r o n"),
        ("claims_zeron: a name with Zeron in Chinese text", "name", "Zeron\u{63d2}\u{4ef6}"),
        ("claims_zeron: a name saying by Zeron in Chinese", "name", "\u{7531}Zeron\u{5236}\u{4f5c}\u{7684}\u{7b14}\u{8bb0}"),
        ("claims_zeron: a description saying by Zeron", "description", "Boards, by Zeron."),
        ("claims_zeron: a description saying from ZERON", "description", "From ZERON: the board."),
        ("claims_zeron: a description saying made by the Zeron team", "description", "Made by the Zeron team."),
        ("claims_zeron: a description saying Zeron's own", "description", "Zeron\u{2019}s own notes."),
        ("claims_zeron: a description saying Zeron approved", "description", "A Zeron approved board."),
        ("claims_zeron: a description saying recommended after a dash", "description", "zeron\u{2014}recommended."),
        ("claims_zeron: a description saying von Zeron", "description", "Ein Plugin von Zeron."),
        ("claims_zeron: a description saying par Zeron", "description", "Un th\u{e8}me par Zeron."),
        ("claims_zeron: a description saying Zeron HQ", "description", "Straight from Zeron HQ."),
        ("claims_zeron: a description saying Zeron official in Chinese is the mark first", "description", "Zeron\u{5b98}\u{65b9}\u{63d2}\u{4ef6}\u{3002}"),
        ("claims_zeron: a description with a modifier apostrophe", "description", "Zeron\u{2bc}s own board."),
        ("claims_zeron: a name for Zeron is fine", "name", "Themes for Zeron"),
        ("claims_zeron: a name in Zeron is fine", "name", "Git in Zeron"),
        ("claims_zeron: a name with Zeron is fine", "name", "Notes with Zeron"),
        ("claims_zeron: a name for Zeron in Chinese is fine", "name", "\u{9002}\u{7528}\u{4e8e}Zeron\u{7684}\u{4e3b}\u{9898}"),
        ("claims_zeron: a name saying Zeronaut is fine", "name", "Zeronaut"),
        ("claims_zeron: a name saying Heron is fine", "name", "Heron"),
        ("claims_zeron: a name using Zeron in Chinese is fine", "name", "\u{4f7f}\u{7528} Zeron \u{7684}\u{6280}\u{5de7}"),
        ("claims_zeron: a description of a zero-noise timer is fine", "description", "a zero-noise timer"),
        ("claims_zeron: a name saying Lazeron is fine", "name", "Lazeron approved"),
        ("claims_zeron: a name saying Zenon is fine", "name", "A board by Zenon"),
        ("claims_zeron: a description about Zeron's sidebar is fine", "description", "Adds a panel to Zeron's sidebar."),
        ("claims_zeron: a description made for Zeron is fine", "description", "Made for Zeron."),
        ("claims_zeron: a description works with Zeron is fine", "description", "Works with Zeron."),
        ("claims_zeron: a description saying Zeron notes is fine", "description", "Zeron notes, kept tidy."),
        ("claims_zeron: a description saying zero n is fine", "description", "Counts from zero n times."),
    ] {
        let mut m = base.clone();
        m[field] = json!(value);
        cases.push((name.into(), m, files.clone()));
    }
    // The mark's words in the store's other languages: never an ordinary
    // word. (name or description, text)
    for (name, field, value) in [
        ("the mark: Spanish oficial", "name", "Cliente oficial"),
        ("the mark: Spanish no oficial in a description is fine", "description", "Cliente no oficial."),
        ("the mark: French v\u{e9}rifi\u{e9} in a description", "description", "Th\u{e8}me v\u{e9}rifi\u{e9}."),
        ("the mark: French v\u{e9}rifie in a name, folded", "name", "V\u{e9}rifie la syntaxe"),
        ("the mark: French officielle", "name", "Extension officielle"),
        ("the mark: German offizieller", "description", "Offizieller Client."),
        ("the mark: German verifiziert in a name", "name", "Verifiziert"),
        ("the mark: German offiziell in a name", "name", "Offiziell"),
        ("the mark: Italian ufficiale", "name", "Client ufficiale"),
        ("the mark: Portuguese verificado", "description", "Tema verificado."),
        ("the mark: Russian official", "name", "\u{41e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{439} \u{43a}\u{43b}\u{438}\u{435}\u{43d}\u{442}"),
        ("the mark: Russian verified in a name", "name", "\u{41f}\u{440}\u{43e}\u{432}\u{435}\u{440}\u{435}\u{43d}\u{43e}"),
        ("the mark: Chinese official", "name", "\u{5b98}\u{65b9}\u{63d2}\u{4ef6}"),
        ("the mark: Chinese not official in a description is fine", "description", "\u{975e}\u{5b98}\u{65b9}\u{63d2}\u{4ef6}\u{3002}"),
        ("the mark: Chinese verified", "name", "\u{5df2}\u{9a8c}\u{8bc1}\u{7684}\u{7b14}\u{8bb0}"),
        ("the mark: Japanese official beside kana", "name", "\u{516c}\u{5f0f}\u{30d7}\u{30e9}\u{30b0}\u{30a4}\u{30f3}"),
        ("the mark: Japanese verified", "description", "\u{691c}\u{8a3c}\u{6e08}\u{307f}\u{306e}\u{30e1}\u{30e2}\u{3002}"),
        ("the mark: Korean verified", "name", "\u{ac80}\u{c99d}\u{b428} \u{d50c}\u{b7ec}\u{adf8}\u{c778}"),
        ("the mark: French v\u{e9}rifie in a description is fine", "description", "V\u{e9}rifie la syntaxe de vos fichiers"),
        ("the mark: German verifiziert in a description is fine", "description", "Verifiziert Signaturen."),
        ("the mark: German offiziell in a description is fine", "description", "Offiziell unterst\u{fc}tzt."),
        ("the mark: German gepr\u{fc}ft in a description is fine", "description", "Eingaben werden gepr\u{fc}ft."),
        ("the mark: Russian checked in a description is fine", "description", "\u{41f}\u{440}\u{43e}\u{432}\u{435}\u{440}\u{435}\u{43d}\u{43e}: 5 \u{444}\u{430}\u{439}\u{43b}\u{43e}\u{432}."),
        ("the mark: Italian verifica is fine", "name", "Verifica la sintassi"),
        ("the mark: Spanish verifica is fine", "name", "Verifica tus archivos"),
        ("the mark: German inoffizieller is fine", "name", "Inoffizieller Client"),
        ("the mark: Chinese formula is fine", "name", "\u{6570}\u{5b66}\u{516c}\u{5f0f}\u{7f16}\u{8f91}\u{5668}"),
        ("the mark: Korean formula is fine", "name", "\u{c218}\u{d559} \u{acf5}\u{c2dd}"),
        ("the mark: Chinese authentication is fine", "name", "\u{8eab}\u{4efd}\u{8ba4}\u{8bc1}"),
        ("the mark: Japanese authentication token is fine", "name", "\u{8a8d}\u{8a3c}\u{30c8}\u{30fc}\u{30af}\u{30f3}"),
        ("the mark: Korean authentication token is fine", "name", "\u{c778}\u{c99d} \u{d1a0}\u{d070}"),
        ("the mark: Spanish oficina is fine", "name", "Oficina de notas"),
        ("the mark: French officier is fine", "name", "Officier de garde"),
        ("the mark: Russian unofficial in one word is fine", "name", "\u{43d}\u{435}\u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{439} \u{43a}\u{43b}\u{438}\u{435}\u{43d}\u{442}"),
        ("the mark: Chinese notes are fine", "name", "\u{7b14}\u{8bb0}\u{677f}"),
        ("the mark: Japanese notes are fine", "name", "\u{30e1}\u{30e2}\u{5e33}"),
        ("the mark: Korean notes are fine", "name", "\u{ba54}\u{baa8}\u{c7a5}"),
    ] {
        let mut m = base.clone();
        m[field] = json!(value);
        cases.push((name.into(), m, files.clone()));
    }
    // Beside "zeron", each is a claim; and Chinese words of origin.
    for (name, value) in [
        ("claims_zeron: Zeron beside Japanese official", "Zeron \u{516c}\u{5f0f}\u{306e}\u{30e1}\u{30e2}\u{3002}"),
        ("claims_zeron: Zeron beside Korean official", "\u{acf5}\u{c2dd} Zeron \u{d50c}\u{b7ec}\u{adf8}\u{c778}."),
        ("claims_zeron: Zeron beside Chinese certified", "Zeron \u{8ba4}\u{8bc1}\u{63d2}\u{4ef6}\u{3002}"),
        ("claims_zeron: Zeron beside French officiel", "Le th\u{e8}me Zeron officiel."),
        ("claims_zeron: by Zeron in Chinese", "\u{7531} Zeron \u{5236}\u{4f5c}\u{3002}"),
        ("claims_zeron: from Zeron in Chinese", "\u{6765}\u{81ea}Zeron\u{7684}\u{7b14}\u{8bb0}\u{3002}"),
    ] {
        let mut m = base.clone();
        m["description"] = json!(value);
        cases.push((name.into(), m, files.clone()));
    }
    // In a description, the mark only when said of the plugin itself:
    // 1 first word or alone; 2 near a word for the plugin; 3 after a word
    // that says it of the subject; 4 beside Zeron or the store (no
    // negation lifts it); a negation directly before lifts 1 to 3.
    for (name, value) in [
        ("description: the first word", "Official Git tools."),
        ("description: alone as a sentence", "Notes, kept tidy. Verified."),
        ("description: near plugin", "A verified plugin for notes."),
        ("description: near extension, a word between", "The official Git extension."),
        ("description: near version, after a comma", "A board, the official version."),
        ("description: Spanish near cliente", "Cliente oficial para Git."),
        ("description: Japanese \u{516c}\u{5f0f} near \u{62e1}\u{5f35}, no kana needed", "\u{516c}\u{5f0f} Git \u{62e1}\u{5f35}\u{3002}"),
        ("description: Korean \u{acf5}\u{c2dd} near \u{d50c}\u{b7ec}\u{adf8}\u{c778}", "\u{acf5}\u{c2dd} Git \u{d50c}\u{b7ec}\u{adf8}\u{c778}"),
        ("description: Chinese \u{5df2}\u{8ba4}\u{8bc1} near \u{63d2}\u{4ef6}", "\u{5df2}\u{8ba4}\u{8bc1}\u{63d2}\u{4ef6}"),
        ("description: Japanese \u{8a8d}\u{8a3c}\u{6e08}\u{307f} near \u{30d7}\u{30e9}\u{30b0}\u{30a4}\u{30f3}", "\u{8a8d}\u{8a3c}\u{6e08}\u{307f}\u{30d7}\u{30e9}\u{30b0}\u{30a4}\u{30f3}"),
        ("description: after is", "This board is verified."),
        ("description: after now", "Now official: boards for everyone."),
        ("description: after fully", "Fully verified and signed."),
        ("description: German after ist", "Der Client ist offizielle Software."),
        ("description: beside Zeron", "Boards, Zeron verified."),
        ("description: beside Zeron, a negation doesn't lift it", "Not Zeron official."),
        ("description: by the store", "Checked: verified by the store."),
        ("description: spaced capitals alone", "Notes. V.E.R.I.F.I.E.D."),
        ("description: a digit for e near plugin", "A v3rified plugin."),
        ("description: someone else's official docs are fine", "Opens the official Rust documentation."),
        ("description: GitHub's verified badge is fine", "Shows GitHub's verified badge next to commits."),
        ("description: not official is fine", "Not official, just useful."),
        ("description: unofficial is fine", "An unofficial client for Git."),
        ("description: non official is fine", "A non official client for Git."),
        ("description: Korean \u{be44}\u{acf5}\u{c2dd} is fine", "\u{c774} \u{d50c}\u{b7ec}\u{adf8}\u{c778}\u{c740} \u{be44}\u{acf5}\u{c2dd} \u{d074}\u{b77c}\u{c774}\u{c5b8}\u{d2b8}\u{c785}\u{b2c8}\u{b2e4}"),
        ("description: verified email addresses are fine", "Lists your verified email addresses."),
        ("description: a sentence between is fine", "Links to the official site. A tool for boards."),
        ("description: German verifiziert near Plugin is fine", "Das Plugin verifiziert Signaturen."),
    ] {
        let mut m = base.clone();
        m["description"] = json!(value);
        cases.push((name.into(), m, files.clone()));
    }
    // Saying it's part of Zeron (words of belonging), and "made"/"built"
    // only after a bare "zeron".
    for (name, field, value) in [
        ("claims_zeron: a description saying ships with Zeron", "description", "Ships with Zeron."),
        ("claims_zeron: a description saying included in Zeron", "description", "Included in Zeron."),
        ("claims_zeron: a description saying part of Zeron", "description", "Part of Zeron."),
        ("claims_zeron: a description saying comes with Zeron", "description", "Comes with Zeron."),
        ("claims_zeron: a description saying built into Zeron", "description", "Built into Zeron."),
        ("claims_zeron: a description saying Zeron-made", "description", "Zeron-made boards."),
        ("claims_zeron: a description saying built into Zeron in Chinese", "description", "\u{5185}\u{7f6e}\u{4e8e}Zeron\u{3002}"),
        ("claims_zeron: a name saying Built-in Zeron", "name", "Built-in Zeron Git"),
        ("claims_zeron: a name saying Default Git for Zeron", "name", "Default Git for Zeron"),
        ("claims_zeron: a description about Zeron's built-in terminal is fine", "description", "Adds a button to Zeron's built-in terminal."),
        ("claims_zeron: a description saying Zeron built-in is fine", "description", "Zeron built-in terminal tools."),
        ("claims_zeron: a description about Zeron's built-in terminal in Chinese is fine", "description", "Zeron\u{5185}\u{7f6e}\u{7ec8}\u{7aef}\u{7684}\u{6309}\u{94ae}"),
    ] {
        let mut m = base.clone();
        m[field] = json!(value);
        cases.push((name.into(), m, files.clone()));
    }
    // Russian case forms (stems), and look-alikes in every language.
    for (name, field, value) in [
        ("the mark: Russian genitive official in a name", "name", "\u{41e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{43e}\u{433}\u{43e} \u{43a}\u{43b}\u{438}\u{435}\u{43d}\u{442}\u{430}"),
        ("the mark: Russian verified, a stem, in a name", "name", "\u{412}\u{435}\u{440}\u{438}\u{444}\u{438}\u{446}\u{438}\u{440}\u{43e}\u{432}\u{430}\u{43d}\u{43d}\u{43e}\u{433}\u{43e} \u{43f}\u{43b}\u{430}\u{433}\u{438}\u{43d}\u{430}"),
        ("the mark: Russian official with a Latin o", "name", "o\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{44b}\u{439} \u{43a}\u{43b}\u{438}\u{435}\u{43d}\u{442}"),
        ("the mark: Spanish oficial with a digit 0", "name", "0ficial"),
        ("the mark: Spanish oficial with Cyrillic letters", "name", "\u{43e}fi\u{441}ial"),
        ("the mark: Spanish verificado with a digit 0", "name", "Verificad0"),
        ("the mark: Russian official theme in the accusative", "description", "\u{423}\u{441}\u{442}\u{430}\u{43d}\u{43e}\u{432}\u{438}\u{442}\u{435} \u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{443}\u{44e} \u{442}\u{435}\u{43c}\u{443}."),
        ("the mark: Russian official plugin in the genitive", "description", "\u{414}\u{43b}\u{44f} \u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{43e}\u{433}\u{43e} \u{43f}\u{43b}\u{430}\u{433}\u{438}\u{43d}\u{430} Git."),
        ("the mark: Spanish oficial with a Cyrillic o near cliente", "description", "Cliente \u{43e}ficial para Git."),
        ("the mark: Russian officially is fine", "name", "\u{41e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{43e} \u{43d}\u{435}\u{442}"),
        ("the mark: Russian not an official app is fine", "description", "\u{42d}\u{442}\u{43e} \u{43d}\u{435} \u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{43e}\u{435} \u{43f}\u{440}\u{438}\u{43b}\u{43e}\u{436}\u{435}\u{43d}\u{438}\u{435}."),
        ("the mark: Russian a proven way is fine", "description", "\u{41f}\u{440}\u{43e}\u{432}\u{435}\u{440}\u{435}\u{43d}\u{43d}\u{44b}\u{439} \u{441}\u{43f}\u{43e}\u{441}\u{43e}\u{431} \u{445}\u{440}\u{430}\u{43d}\u{438}\u{442}\u{44c} \u{437}\u{430}\u{43c}\u{435}\u{442}\u{43a}\u{438}."),
        ("the mark: Russian official Rust documentation is fine", "description", "\u{41f}\u{43e}\u{43a}\u{430}\u{437}\u{44b}\u{432}\u{430}\u{435}\u{442} \u{43e}\u{444}\u{438}\u{446}\u{438}\u{430}\u{43b}\u{44c}\u{43d}\u{443}\u{44e} \u{434}\u{43e}\u{43a}\u{443}\u{43c}\u{435}\u{43d}\u{442}\u{430}\u{446}\u{438}\u{44e} Rust."),
    ] {
        let mut m = base.clone();
        m[field] = json!(value);
        cases.push((name.into(), m, files.clone()));
    }
    // Keywords: neither the mark (any language a keyword can write, a part
    // or its parts joined) nor "zeron"; no badge lifts it.
    for (name, keyword) in [
        ("keywords: verified", "verified"),
        ("keywords: veri-fied joined", "veri-fied"),
        ("keywords: a digit for O", "0fficial"),
        ("keywords: Spanish oficial", "oficial"),
        ("keywords: French verifie, folded", "verifie"),
        ("keywords: zeron", "zeron"),
        ("keywords: zeron as a part", "zeron-themes"),
        ("keywords: zeron after a space", "themes zeron"),
        ("keywords: zer0n", "zer0n"),
        ("keywords: 2eron", "2eron"),
        ("keywords: ze-ron joined", "ze-ron"),
        ("keywords: themes are fine", "themes"),
        ("keywords: zero is fine", "zero"),
        ("keywords: heron is fine", "heron"),
        ("keywords: verify is fine", "verify"),
        ("keywords: zeronaut is fine", "zeronaut"),
        ("keywords: ver-ified joined", "ver-ified"),
        ("keywords: ver1fied", "ver1fied"),
        ("keywords: official as a part", "official-docs"),
        ("keywords: by-zeron", "by-zeron"),
        ("keywords: zeron-team", "zeron-team"),
        ("keywords: zer0n-approved", "zer0n-approved"),
        ("keywords: zero-config is fine", "zero-config"),
        ("keywords: verification is fine", "verification"),
        ("keywords: officially-unsupported is fine", "officially-unsupported"),
    ] {
        let mut m = base.clone();
        m["keywords"] = json!(["focus", keyword]);
        cases.push((name.into(), m, files.clone()));
    }
    cases.push(("a binary file (a warning)".into(), base.clone(), with_file("bin/tool.wasm", b"\0asm\x01\0\0\0".to_vec())));
    for (name, a, b) in [
        ("two paths differing in case", "README.md", "readme.md"),
        ("two paths differing in Unicode form", "caf\u{e9}.txt", "cafe\u{301}.txt"),
        ("two folders differing in case", "shots/a.png", "Shots/A.png"),
        ("a path and the same with a trailing dot", "x.txt", "x.txt."),
        ("a file that is its manifest on a Mac", "ZERON-PLUGIN.JSON", "init2.luau"),
        // (Amendment 42: a file where another needs a folder of that name.)
        ("a file and a folder that fold alike", "ui", "UI/x.luau"),
        ("a folder and a file that fold alike", "Ui", "ui/x.luau"),
        ("its manifest's name as a folder on a Mac", "ZERON-PLUGIN.JSON/x.luau", "init2.luau"),
    ] {
        let mut two = files.clone();
        two.insert(a.into(), b"1".to_vec());
        two.insert(b.into(), b"2".to_vec());
        cases.push((name.into(), base.clone(), two));
    }
    // Three paths, in code point order: a path that clashes records nothing
    // (no file, no folders), so a later one is held only to what came
    // before it without a problem. The first two decide that: a folder, a
    // file folding onto it (the clash), then a path inside that folder,
    // which doesn't clash.
    for (name, paths) in [
        ("a file on a folder, then a path inside it", ["UI/a", "Ui", "ui/b"]),
        ("a file on a folder in Unicode's other spelling, then a path inside it", ["cafe\u{301}/a", "caf\u{e9}", "caf\u{e9}/b"]),
        ("a file, a path under it, a path deeper under it", ["A", "A/B", "a/b/c"]),
        ("a file, the same file, a path under it", ["A", "a", "a/x"]),
    ] {
        let mut three = files.clone();
        for rel in paths {
            three.insert(rel.into(), b"1".to_vec());
        }
        cases.push((name.into(), base.clone(), three));
    }
    Value::Array(
        cases
            .into_iter()
            .map(|(name, manifest, files)| {
                let (problems, warnings) = listing::check(&Manifest::from_value(manifest.clone()).unwrap(), &files);
                let said = |list: &[listing::Problem]| -> Value { list.iter().map(|p| json!({ "rule": p.rule, "field": p.field, "path": p.path })).collect() };
                json!({
                    "name": name, "manifest": compact(&text(&manifest)), "files_b64": files_b64(&files),
                    "problems": said(&problems), "warnings": said(&warnings), "listing": listing::Listing::of(&manifest),
                })
            })
            .collect(),
    )
}

fn changelog_cases() -> Value {
    let log = "# Changelog\n\n## [1.2.0] - 2026-11-01\n\n### Added\n- Boards.\n\n## v1.1.0\n- First.\n\n## 1.0.0\n\n## 0.9.0\n- Old.\n";
    Value::Array(
        ["1.2.0", "1.1.0", "1.0.0", "0.9.0", "0.8.0"]
            .iter()
            .map(|v| json!({ "changelog": log, "version": v, "notes": listing::changelog_notes(log, v) }))
            .collect(),
    )
}

// ── Deploy keys and signed requests ─────────────────────────────────────────

/// A signed request's two headers, as sent.
fn headers(authorization: String, body_sha256: &str) -> Value {
    let mut headers = serde_json::Map::new();
    headers.insert("authorization".into(), json!(authorization));
    headers.insert(deploy_key::CONTENT_SHA256_HEADER.to_ascii_lowercase(), json!(body_sha256));
    Value::Object(headers)
}

fn deploy_key_cases() -> Value {
    let key = DeployKey::from_seed([3; 32]).unwrap();
    let request = |method: &str, path: &str, body: &[u8], ts: i64| {
        let body_sha256 = signing::sha256_hex(body);
        json!({
            "method": method, "path": path, "body_b64": B64.encode(body), "body_sha256": body_sha256, "ts": ts,
            "message": deploy_key::request_message(method, path, &body_sha256, ts),
            "headers": headers(key.authorization(method, path, body, ts), &body_sha256),
        })
    };
    // Proof of possession, v2: one account's key (its publisher id), one
    // lifetime (`expires_at`, or never), one scope (`*`, or the ids sorted
    // and joined with `,`), the key last.
    let publisher_id = "pub_0123456789abcdef0123456789abcdef";
    let proofs: Vec<Value> = [
        (deploy_key::Scope::All, None),
        (deploy_key::Scope::plugins(&["git".into(), "focus".into()]).unwrap(), Some(1_800_000_000_000_i64)),
    ]
    .iter()
    .map(|(scope, expires_at)| {
        json!({
            "publisher_id": publisher_id, "expires_at": expires_at,
            "scope": scope.to_value(), "scope_text": scope.text(),
            "message": deploy_key::proof_message(publisher_id, *expires_at, scope, &key.public()),
            "proof": key.proof(publisher_id, *expires_at, scope),
        })
    })
    .collect();
    json!({
        "secret_prefix": deploy_key::SECRET_PREFIX,
        "secret": key.secret(),
        "public": key.public(),
        "proofs": proofs,
        "max_skew_ms": deploy_key::MAX_SKEW_MS,
        "content_sha256_header": deploy_key::CONTENT_SHA256_HEADER,
        "request": request("POST", "/store/v1/plugins/focus/versions", br#"{"format":"zeron-plugin:v1"}"#, 1_763_000_000_000),
        "get": request("GET", "/store/v1/me/plugins", b"", 1_763_000_000_000),
        "bad_secrets": [
            { "secret": "ghp_notours", "error": "not_a_secret" },
            { "secret": &key.secret()[..key.secret().len() - 2], "error": "damaged" },
        ],
    })
}

// ── What a plugin asks for ──────────────────────────────────────────────────

/// The tables behind the consent sheet's ratings, as data
/// (`zeron_plugin_guard::{commands, consent, net}`), and what always waits
/// for the person (`capability::OUTSIDE_SANDBOX`).
fn consent_tables() -> Value {
    use zeron_plugin_guard::commands;
    json!({
        "contained": commands::CONTAINED,
        "full_control": commands::FULL_CONTROL,
        "held_to_its_own_rules": commands::HELD_TO_ITS_OWN_RULES,
        "windows_extensions": commands::WINDOWS_EXTENSIONS,
        "about": "A program's name is lowercased and loses one Windows extension (windows_extensions) before it's looked up. It can do anything you can unless it's in contained. It gives full control when it's in full_control, as it is or with its trailing digits and dots taken off (python3.12 → python). commands_anything is every program that can do anything you can and is neither full_control nor held_to_its_own_rules. A host is local when it's localhost, a private or loopback address, ends .localhost .local .internal .lan .home.arpa .intranet .corp, has one label, or ends in a number (decimal or 0x hex); network_local is every host but * that is local, network_named every host that names one host exactly (not * or *.…) and isn't local.",
        "always_ask": capability::OUTSIDE_SANDBOX,
    })
}

fn consent_item_cases() -> Value {
    let base = json!({ "id": "focus", "name": "Focus", "version": "1.0.0", "entry": "init.luau" });
    let with = |extra: Value| {
        let mut m = base.clone();
        for (k, v) in extra.as_object().unwrap() {
            m[k] = v.clone();
        }
        m
    };
    let cases = [
        ("asks for nothing", base.clone()),
        ("ui", with(json!({ "permissions": ["ui.add", "ui.change", "ui.replace"], "ui": { "change": ["#sidebar >> .session-row"], "replace": ["@app/screens/sidebar"] } }))),
        ("programs, rated", with(json!({ "permissions": ["commands"], "commands": ["git", "gh", "jq", "cat", "curl", "Python3.12.exe", "bash", "node", "git-lfs", "scalar", "osascript"] }))),
        ("hosts, rated", with(json!({ "permissions": ["network"], "network": { "hosts": ["api.example.com", "*.example.org", "localhost:8080", "nas.local", "10.0.0.2", "127.1", "intranet", "printer.lan", "[::1]:9000", "0x7f.1"] } }))),
        ("tools", with(json!({ "permissions": ["tools"], "tools": [{ "name": "count_words", "description": "Count words", "input": { "type": "object" } }] }))),
        ("fractional settings", fractional()),
        ("everything", with(json!({ "permissions": ["native", "service", "commands", "network", "tools"], "commands": ["sh"], "service": "service.luau", "mcp": { "command": "./bin/x" }, "network": { "hosts": ["api.example.com"] }, "tools": [{ "name": "t", "description": "d" }] }))),
    ];
    Value::Array(
        cases
            .into_iter()
            .map(|(name, manifest)| {
                let m = Manifest::from_value(manifest.clone()).unwrap();
                assert_eq!(m.problem(), None, "{name}");
                json!({ "name": name, "manifest": text(&manifest), "item": capability::consent_item(&m) })
            })
            .collect(),
    )
}

/// `docs/plugin-store-data.json`: the store's rules as data, small enough
/// for the edge to import at run time (its Worker never bundles the test
/// vectors): formats and prefixes, limits, categories, kept ids, report
/// reasons, deploy-key constants, and the tables behind the consent
/// sheet's ratings.
fn data() -> Value {
    json!({
        "format": "zeron-plugin-store-data:v1",
        "about": "Made by crates/engine/src/plugins/vectors from the engine's own constants; the edge imports it. max_manifest_bytes is the manifest's canonical JSON without its signature; max_manifest_json_bytes and max_manifest_values are the manifest as written in the package, signature included, every JSON value counted once (keys not); max_publish_body_bytes is the .zeron-plugin file as uploaded. reserved_ids are the manifest rule's: ids Zeron itself won't run ('Zeron already uses that name', the manifests cases' id_error). site_ids are the store site's own pages beside /plugins/<id>: a device runs a plugin with one, but the store never lists it (the listing rule id_reserved, which no reservation lifts). The cases that hold both sides to these rules are in docs/plugin-package-vectors.json.",
        "package_format": FORMAT,
        "payload_prefix": PAYLOAD_PREFIX,
        "template_payload_prefix": TEMPLATE_PAYLOAD_PREFIX,
        "key_proof_prefix": signing::KEY_PROOF_PREFIX,
        "deploy_key": {
            "secret_prefix": deploy_key::SECRET_PREFIX,
            "proof_prefix": deploy_key::PROOF_PREFIX,
            "request_prefix": deploy_key::REQUEST_PREFIX,
            "content_sha256_header": deploy_key::CONTENT_SHA256_HEADER,
            "max_skew_ms": deploy_key::MAX_SKEW_MS,
        },
        "limits": {
            "max_files": super::files::MAX_FILES,
            "max_bytes": super::files::MAX_BYTES,
            "max_package_bytes": MAX_PACKAGE_BYTES,
            "max_path": zeron_plugin_guard::path::MAX_PATH,
            "max_hosts": zeron_plugin_guard::net::MAX_HOSTS,
            "max_version_len": version::MAX_LEN,
            "max_range_len": version::MAX_RANGE_LEN,
            "max_number": version::MAX_NUMBER,
            "max_categories": listing::MAX_CATEGORIES,
            "max_keywords": listing::MAX_KEYWORDS,
            "max_keyword_len": listing::MAX_KEYWORD_LEN,
            "max_screenshots": listing::MAX_SCREENSHOTS,
            "max_url_len": listing::MAX_URL_LEN,
            "max_license_len": listing::MAX_LICENSE_LEN,
            "max_name_len": listing::MAX_NAME_LEN,
            "max_description_len": listing::MAX_DESCRIPTION_LEN,
            "max_icon_bytes": listing::MAX_ICON_BYTES,
            "max_readme_bytes": listing::MAX_README_BYTES,
            "max_changelog_bytes": listing::MAX_CHANGELOG_BYTES,
            "max_manifest_bytes": listing::MAX_MANIFEST_BYTES,
            "max_publish_body_bytes": listing::MAX_PUBLISH_BYTES,
            "max_manifest_json_bytes": listing::MAX_MANIFEST_JSON_BYTES,
            "max_manifest_values": listing::MAX_MANIFEST_VALUES,
            "max_notes_bytes": listing::MAX_NOTES_BYTES,
            "screenshot_types": listing::SCREENSHOT_TYPES,
            "max_update_plugins": super::store::MAX_UPDATE_PLUGINS,
        },
        "newer_rule": "A store version is newer by SemVer 2.0.0 precedence, build metadata ignored: equal precedence is the same version. A publish must be later than every version the plugin has had, in any state, pre-releases included.",
        "mark_claims": {
            "about": "A listing's name or description may not claim the store's mark (amendment 45; bad_listing on that field). One rule, the device's check and the edge alike (crates/engine/src/plugins/mark.rs). Fold: Unicode NFKC, then full lowercase (as JavaScript's toLowerCase), then NFKD with every nonspacing mark (General_Category Mn) dropped, then every Default_Ignorable_Code_Point (variation selectors among them) and every enclosing mark (General_Category Me) removed. It claims the mark when the folded text has any of glyphs anywhere; or any of words as a whole word, as written (no ASCII letter or digit right before or after it); or, as a whole word in the folded text or in it with its spaced runs read as words, any of words spelled letter by letter with: the letter itself or any of lookalikes[letter] (for each letter of interchangeable's pairs, those of both letters), or for a letter in pairs, its characters in order, each it or one of its own lookalikes. A spaced run: single characters, each with a separator or the text's edge on either side, apart by 1 to max_spacing separators (White_Space, U+FEFF, or General_Category P*), read as one word with the separators gone. lookalikes is generated from source (Unicode's confusables.txt, at version, whose file's sha256 is sha256): every single-character source whose prototype folds to one of the letters of words or of claims_zeron's phrases, kept both folded and as written unless either is an ASCII letter, plus the Latin small capitals, the boxed emoji letters (NEGATIVE CIRCLED and NEGATIVE SQUARED LATIN CAPITAL LETTERs, REGIONAL INDICATOR SYMBOL LETTERs), the digit 3 for e and the digit 2 for z. Then, in the store's other languages: each of languages[lang].words, in either reading, spelled letter by letter as the words are (each letter itself or one of its lookalikes; a letter that is itself one of lookalikes[x] reads as x, so a Cyrillic word's о takes a Latin o), as a substring where substring is true (Chinese, Japanese, Korean), else whole (no character of General_Category L* or N* right before or after it); a word ending in '*' is a stem: no letter or digit right before it, any letters after (Russian's case forms). In a name, every word, folded the same way. In a description, a word whose description is 'folded' the same way; 'accented' read with accents kept (NFKC, lowercase, ignorables and Me gone, on both the text and the word; French 'vérifié', whose folded form is also 'vérifie', checks); 'never' not at all (it's also an ordinary word there). A word with with_kana is refused only in a text that also holds kana (U+3040–30FF, U+31F0–31FF, U+FF66–FF9F): Japanese 公式 is official, Chinese 公式 a formula. The listing cases named 'the mark:' hold both sides to it.",
            "words": listing::MARK_WORDS,
            "glyphs": listing::MARK_GLYPHS.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
            "lookalikes": super::mark::LOOKALIKES.iter().map(|(letter, chars)| (letter.to_string(), json!(chars.chars().map(|c| c.to_string()).collect::<Vec<_>>()))).collect::<serde_json::Map<_, _>>(),
            "interchangeable": super::mark::INTERCHANGEABLE.iter().map(|(a, b)| json!([a.to_string(), b.to_string()])).collect::<Vec<_>>(),
            "pairs": super::mark::PAIRS.iter().map(|(letter, pair)| (letter.to_string(), json!([pair]))).collect::<serde_json::Map<_, _>>(),
            "max_spacing": super::mark::MAX_SPACING,
            "source": { "name": super::mark::SOURCE, "version": super::mark::SOURCE_VERSION, "sha256": super::mark::SOURCE_SHA256 },
            "languages": super::mark::OTHER_LANGUAGES
                .iter()
                .map(|(lang, substring, words)| {
                    let words: Vec<Value> = words
                        .iter()
                        .map(|w| {
                            let description = match w.description {
                                super::mark::InDescription::Folded => "folded",
                                super::mark::InDescription::Accented => "accented",
                                super::mark::InDescription::NameOnly => "never",
                            };
                            let mut word = json!({ "word": w.word, "description": description });
                            if w.with_kana {
                                word["with_kana"] = json!(true);
                            }
                            if !w.before.is_empty() {
                                word["before"] = json!(w.before);
                            }
                            word
                        })
                        .collect();
                    (lang.to_string(), json!({ "substring": substring, "words": words }))
                })
                .collect::<serde_json::Map<_, _>>(),
            "description": {
                "about": "In a description, only a glyph refuses anywhere. A mark word is refused only when said of the plugin itself. Read the folded text (and its spaced runs read as words) as words: each run of characters that aren't separators is a word, but each Han ideograph (U+3400–4DBF, U+4E00–9FFF, U+F900–FAFF, U+20000–3134F) is a word by itself. The mark words here: words spelled as mark_claims reads them (lookalikes and all); each word of languages whose description is 'folded' (where it is, substring or whole, any script's letters as the boundary); 'accented' ones the same in the text read with accents kept; and ambiguous, as substrings, for rule 2 and rule 4 only, with the languages words that have with_kana or before. Each occurrence covers the words it overlaps. It's refused when: (4) the words directly before it read as one of store_words, or the words directly after it do (nothing in sentence_ends between); else not when the word directly before it is one of negations, or (in a word that holds it) the characters directly before it are; else (1) its first word is the text's first word, or every word of its sentence (sentence_ends bound it) is one it covers; or (3) the word directly before it is one of subject_words; or (2) a word or phrase of plugin_words (whole words in order, or substrings where substring) has at most two words between it and the occurrence, and nothing in sentence_ends between them. Words and phrases of these lists are compared in the reading's own form (folded, or with accents kept). The listing cases named 'description:' hold both sides to it.",
                "plugin_words": super::mark::PLUGIN_WORDS.iter().map(|(lang, substring, words)| (lang.to_string(), json!({ "substring": substring, "words": words }))).collect::<serde_json::Map<_, _>>(),
                "subject_words": super::mark::SUBJECT_WORDS,
                "negations": super::mark::NEGATIONS,
                "sentence_ends": super::mark::SENTENCE_ENDS.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
                "store_words": super::mark::STORE_WORDS,
                "ambiguous": super::mark::AMBIGUOUS,
            },
        },
        "unseen": "A name or description may hold no character drawn as nothing (bad_listing): a control or format character (General_Category Cc, Cf), any other Default_Ignorable_Code_Point (the Hangul fillers among them), any Variation_Selector but U+FE0F, and U+FE0F only directly after a character with the Emoji property. Keywords are ASCII. The listing cases named 'unseen:' hold both sides to it.",
        "claims_zeron": {
            "about": "A name or description may not say the plugin is Zeron's own unless its publisher is (the maintainer badge). Every spelling below is read as mark_claims reads its words: in the folded text, or in it with its spaced runs read as words, whole (no ASCII letter or digit right before or after it), each letter itself or one of mark_claims' lookalikes (interchangeable and pairs too), an apostrophe any of ' U+2019 U+02BC, a space a run of one or more separators (White_Space, U+FEFF, General_Category P*) but an apostrophe, and between a word in a script that doesn't space its words and the next a run of none or more. Name: each whole word's end at which word is spelled must also be the end of '<form> word' for a form of name.allowed_before (any language); one that isn't is claims_zeron. Description: any of '<origin> word' (description.origin_before), '<standing> word', 'word <standing>' and, for a standing word in ASCII, 'word's <standing>' (description.standing) is claims_zeron. The edge refuses a publish 400 claims_zeron unless the publisher carries maintainer; a device, which can't know the publisher, gives the warning claims_zeron on that field. The listing cases named 'claims_zeron:' hold both sides to it (warnings).",
            "word": super::mark::ZERON,
            "name": { "allowed_before": super::mark::NAME_FORMS.iter().map(|(lang, forms)| (lang.to_string(), json!(forms))).collect::<serde_json::Map<_, _>>() },
            "description": {
                "origin_before": super::mark::ORIGIN_WORDS,
                "standing": super::mark::STANDING_WORDS,
                "standing_other": super::mark::STANDING_OTHER,
                "standing_other_about": "Standing too: standing_other, and every word of mark_claims.languages (any language, any description reading), beside the word. 'made' and 'built' (standing_after_word) only directly after a bare word, never followed by a separator and the whole word 'in' (not 'built-in', 'built in'), never after the possessive.",
                "standing_after_word": super::mark::STANDING_AFTER_ZERON,
                "belonging": super::mark::BELONGING,
                "belonging_joins": super::mark::BELONGING_JOINS,
                "belonging_zh": super::mark::BELONGING_ZH,
                "belonging_about": "Saying the plugin is part of Zeron. A name in which word is spelled anywhere and any of belonging or belonging_zh too is claims_zeron. A description is claims_zeron when it spells '<b> <join> word' for b of belonging and join of belonging_joins (for a b that ends in ' <join>', '<b> word'), or for b of belonging_zh '<b>~<j>~word' for j of 于 於 在 到, or '随~word~<b>' / '隨~word~<b>'.",
            },
        },
        "keyword_claims": "A keyword (already lowercase letters, digits, dashes and single spaces) is refused (bad_listing, field keywords) when, its parts split at '-' and ' ' and each folded as mark_claims folds, a part or all its parts joined spells, whole, with mark_claims' lookalikes (interchangeable and pairs too): one of mark_claims.words; a word of mark_claims.languages whose folded form is ASCII (any description reading); or claims_zeron.word. No badge lifts it. The listing cases named 'keywords:' hold both sides to it.",
        "categories": CATEGORIES.iter().map(|(id, name)| json!({ "id": id, "name": name })).collect::<Vec<_>>(),
        "builtin_ids": listing::BUILTIN_IDS,
        "reserved_ids": manifest::RESERVED_IDS,
        "site_ids": listing::SITE_IDS,
        "report_reasons": super::store::REPORT_REASONS,
        "consent": consent_tables(),
    })
}

/// `docs/plugin-package-vectors.json`: the cases, for tests alone.
fn build() -> Value {
    json!({
        "format": "zeron-plugin-package-vectors:v1",
        "about": "Made by crates/engine/src/plugins/vectors from the engine's own code; the edge's package verification must agree with every case. A string field written as {parts: [...]} is its parts concatenated, each a string or {repeat, count} (that string count times); a files_b64 value written so is that text's UTF-8 bytes, and package_parts is a package's bytes so. The constants and tables these cases hold to are in docs/plugin-store-data.json. JSON inputs are text (input, manifest), parsed as the engine parses them. Keys are ed25519, base64url without padding; hashes are lowercase hex sha256; package_b64 and files_b64 are standard base64 of the exact bytes.",
        "canonical_json": canonical_cases(),
        "content_hash": content_hash_cases(),
        "signatures": signature_cases(),
        "manifests": manifest_cases(),
        "packages": package_cases(),
        "paths": path_cases(),
        "semver": semver_cases(),
        "newer": newer_cases(),
        "ranges": range_cases(),
        "listing": listing_cases(),
        "changelog": changelog_cases(),
        "deploy_key": deploy_key_cases(),
        "consent_items": consent_item_cases(),
    })
}

/// Each generated file and what it says now.
fn generated() -> [(PathBuf, Value); 2] {
    let docs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    [(docs.join("plugin-store-data.json"), data()), (docs.join("plugin-package-vectors.json"), build())]
}

#[test]
fn vectors_are_current() {
    for (path, value) in generated() {
        let mut text = serde_json::to_string_pretty(&value).unwrap();
        text.push('\n');
        if std::env::var_os("UPDATE_VECTORS").is_some() {
            std::fs::write(&path, &text).unwrap();
            continue;
        }
        let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            on_disk == text,
            "{} is out of date with the engine's rules: run `UPDATE_VECTORS=1 cargo test -p zeron-engine vectors_are_current` and commit it (the edge reads it)",
            path.display()
        );
    }
}

/// How serde_json writes floats is part of what a signature covers: the
/// edge writes them the same way. A serde_json upgrade that changes it
/// (as the move from ryu did) must fail here, loudly, before any signature
/// made on one side stops checking out on the other.
#[test]
fn serde_json_writes_floats_as_the_vectors_say() {
    let cases = [
        ("1.0", "1.0"), ("1.5", "1.5"), ("-0", "-0.0"), ("0.1", "0.1"), ("1e2", "100.0"), ("1e15", "1000000000000000.0"),
        ("1e16", "1e+16"), ("1e21", "1e+21"), ("1e-5", "0.00001"), ("1e-6", "1e-6"), ("1e-7", "1e-7"),
        ("0.30000000000000004", "0.30000000000000004"), ("1.7976931348623157e308", "1.7976931348623157e+308"), ("5e-324", "5e-324"),
        ("123456789012345678901234567890", "1.2345678901234568e+29"), ("18446744073709551615", "18446744073709551615"),
        ("-9223372036854775809", "-9.223372036854776e+18"), ("3.14159265358979323846264338327950288419716939937510", "3.141592653589793"),
    ];
    for (input, written) in cases {
        let value: Value = serde_json::from_str(input).unwrap();
        assert_eq!(
            String::from_utf8(signing::canonical_json(&value)).unwrap(),
            written,
            "serde_json now writes {input} differently: the canonical JSON every plugin signature covers changed. Pin serde_json, or change the edge's canonical JSON with it and regenerate the vectors."
        );
    }
    for refused in ["1e400", "\"\\ud800\"", "01", "NaN"] {
        assert!(serde_json::from_str::<Value>(refused).is_err(), "{refused}");
    }
}

/// Numbers survive the whole path a package takes: a manifest carrying
/// each, signed, packed, read back and verified, has the same content hash
/// and a signature that checks out, and canonicalizes as it did.
#[test]
fn numbers_survive_sign_pack_parse_verify() {
    let numbers = [
        "0.5", "0.25", "0.1", "1.0", "100.0", "1e21", "1e-7", "1.5e300", "5e-324", "-0.0", "9007199254740992", "9007199254740993",
        "-9223372036854775808", "18446744073709551615", "18446744073709551616", "1e16", "123456789012345678901234567890",
    ];
    let key = author();
    let files = files(&[("init.luau", b"return 1")]);
    for number in numbers {
        let text = format!(r#"{{"id":"dimmer","name":"Dimmer","version":"1.0.0","entry":"init.luau","settings":[{{"key":"n","type":"number","default":{number}}}]}}"#);
        let manifest: Value = serde_json::from_str(&text).unwrap();
        let before = signing::canonical_json(&manifest);
        let raw = package::pack(FORMAT, &key.sign_manifest(&manifest, &package::hashes(&files)), &files).unwrap();
        let parsed = install::parse(raw).unwrap_or_else(|e| panic!("{number}: {e}"));
        assert_eq!(parsed.key, key.public(), "{number}");
        assert_eq!(signing::canonical_json(&parsed.manifest.unsigned_value()), before, "{number} changed on its way");
        let expected = signing::content_hash(&Manifest::from_value(manifest).unwrap(), &package::hashes(&files));
        assert_eq!(parsed.hash, expected, "{number}");
    }
}

/// What the file says agrees with the code it came from, case by case (a
/// guard on this generator, not only on drift).
#[test]
fn vectors_say_what_the_code_does() {
    let v = build();
    for case in v["packages"].as_array().unwrap() {
        let Some(b64) = case["package_b64"].as_str() else { continue };
        let raw = B64.decode(b64).unwrap();
        if case["result"]["ok"] == true {
            assert_eq!(case["result"]["sha256"].as_str().unwrap(), signing::sha256_hex(&raw));
        }
    }
    let valid = &v["packages"][0]["result"];
    assert_eq!((valid["ok"].as_bool(), valid["key"].as_str()), (Some(true), Some(author().public().as_str())));
    let words: Vec<&str> = v["packages"].as_array().unwrap().iter().filter_map(|c| c["result"]["error"].as_str()).collect();
    for word in ["not_a_package", "unsigned", "tampered", "bad_signature", "bad_file", "paths_collide", "bad_manifest", "bad_id", "missing_entry"] {
        assert!(words.contains(&word), "{word} has a case");
    }
    let fractional = v["packages"].as_array().unwrap().iter().find(|c| c["name"] == "fractional settings").unwrap();
    assert_eq!(fractional["result"]["ok"], true, "a fraction is never refused");
    let signatures: Vec<&str> = v["signatures"].as_array().unwrap().iter().map(|c| c["result"].as_str().unwrap()).collect();
    for word in ["valid", "unsigned", "files_differ", "bad_signature"] {
        assert!(signatures.contains(&word), "{word} has a case");
    }
    assert!(v["canonical_json"].as_array().unwrap().iter().any(|c| c["error"] == true));
    let secret = v["deploy_key"]["secret"].as_str().unwrap();
    assert_eq!(DeployKey::from_secret(secret).unwrap().public(), v["deploy_key"]["public"]);
    // git and gh are the programs held to rules of their own.
    for program in zeron_plugin_guard::commands::HELD_TO_ITS_OWN_RULES {
        assert!(zeron_plugin_guard::commands::held_to_its_own_rules(program));
    }
}

