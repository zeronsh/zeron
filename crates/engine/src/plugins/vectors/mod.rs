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
    cases.push(("a binary file (a warning)".into(), base.clone(), with_file("bin/tool.wasm", b"\0asm\x01\0\0\0".to_vec())));
    for (name, a, b) in [
        ("two paths differing in case", "README.md", "readme.md"),
        ("two paths differing in Unicode form", "caf\u{e9}.txt", "cafe\u{301}.txt"),
        ("two folders differing in case", "shots/a.png", "Shots/A.png"),
        ("a path and the same with a trailing dot", "x.txt", "x.txt."),
        ("a file that is its manifest on a Mac", "ZERON-PLUGIN.JSON", "init2.luau"),
    ] {
        let mut two = files.clone();
        two.insert(a.into(), b"1".to_vec());
        two.insert(b.into(), b"2".to_vec());
        cases.push((name.into(), base.clone(), two));
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
    json!({
        "secret_prefix": deploy_key::SECRET_PREFIX,
        "secret": key.secret(),
        "public": key.public(),
        "proof_message": deploy_key::proof_message("focus", &key.public()),
        "proof": key.proof("focus"),
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
        "newer_rule": "A store version is newer by SemVer 2.0.0 precedence, build metadata ignored: equal precedence is the same version.",
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

