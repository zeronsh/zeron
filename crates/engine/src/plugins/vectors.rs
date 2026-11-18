//! `docs/plugin-package-vectors.json`: the cases the edge's TypeScript
//! verification of a plugin package must agree with, made by this crate's
//! own code (canonical JSON, content hashes, signatures, packages, path
//! rules, limits, strict versions and their order, `engines.zeron` ranges,
//! the store's listing rules, changelog notes, deploy keys).
//!
//! The file is checked in. `vectors_are_current` builds it again and fails
//! when it differs: a change to any of these rules shows up as a change to
//! the file, which the edge's tests read. `UPDATE_VECTORS=1 cargo test -p
//! zeron-engine vectors_are_current` writes it.
//!
//! Every key and signature comes from a fixed seed, so the file is the same
//! on every run (ed25519 signatures are deterministic).

use std::collections::BTreeMap;
use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};

use super::install::{self, FORMAT};
use super::listing::{self, CATEGORIES};
use super::manifest::Manifest;
use super::package::{self, MAX_PACKAGE_BYTES};
use super::signing::{
    self, FileHashes, PAYLOAD_PREFIX, SigCheck, SigningKey, TEMPLATE_PAYLOAD_PREFIX,
};
use super::store::deploy_key::{self, DeployKey};
use super::version::{self, Range, Version};

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/plugin-package-vectors.json")
}

/// The key every signed case is signed with, and another for "someone else".
fn author() -> SigningKey {
    SigningKey::from_seed(&[1; 32]).unwrap()
}

fn other() -> SigningKey {
    SigningKey::from_seed(&[2; 32]).unwrap()
}

fn hashes(files: &BTreeMap<String, Vec<u8>>) -> FileHashes {
    package::hashes(files)
}

fn files(list: &[(&str, &[u8])]) -> BTreeMap<String, Vec<u8>> {
    list.iter()
        .map(|(k, v)| ((*k).to_owned(), v.to_vec()))
        .collect()
}

fn files_b64(files: &BTreeMap<String, Vec<u8>>) -> Value {
    Value::Object(
        files
            .iter()
            .map(|(k, v)| (k.clone(), json!(B64.encode(v))))
            .collect(),
    )
}

fn sig_word(check: &SigCheck) -> &'static str {
    match check {
        SigCheck::Valid(_) => "valid",
        SigCheck::Unsigned => "unsigned",
        SigCheck::FilesDiffer => "files_differ",
        SigCheck::BadSignature => "bad_signature",
    }
}

fn canonical_cases() -> Value {
    let cases = [
        (
            "keys sorted, no whitespace",
            json!({ "b": 1, "a": [true, false, null], "c": { "z": "", "y": {} } }),
        ),
        (
            "keys sorted by UTF-8 bytes, not UTF-16 units",
            json!({ "\u{e000}": 1, "\u{1f600}": 2, "Z": 3, "a": 4, "é": 5 }),
        ),
        (
            "strings escaped as JSON.stringify does",
            json!({ "s": "quote \" backslash \\ slash / newline \n tab \t nul \u{0} unit sep \u{1f} del \u{7f} line sep \u{2028} é 😀" }),
        ),
        ("arrays keep their order", json!([3, 1, 2, [], {}])),
        (
            "integers",
            json!({ "zero": 0, "neg": -7, "max_safe": 9007199254740991_i64 }),
        ),
        // A number that isn't a plain integer is written as serde_json
        // writes it (`1.5`; but `1.0` as `1.0`, where JavaScript writes
        // `1`): only integers canonicalize the same everywhere.
        ("a fraction", json!({ "f": 1.5 })),
    ];
    Value::Array(
        cases
            .into_iter()
            .map(|(name, input)| json!({ "name": name, "input": input, "canonical": String::from_utf8(signing::canonical_json(&input)).unwrap() }))
            .collect(),
    )
}

fn content_hash_cases() -> Value {
    let base = files(&[("init.luau", b"return 1"), ("ui/badge.luau", b"return {}")]);
    let manifest = json!({ "id": "focus", "name": "Focus", "version": "1.0.0", "entry": "init.luau", "permissions": ["ui.add"] });
    let signed = author().sign_manifest(&manifest, &hashes(&base));
    let cases = [
        ("unsigned", manifest.clone(), base.clone()),
        (
            "signed: the signature isn't part of it",
            signed,
            base.clone(),
        ),
        (
            "a file changed",
            manifest.clone(),
            files(&[("init.luau", b"return 2"), ("ui/badge.luau", b"return {}")]),
        ),
        (
            "unknown fields are part of it",
            {
                let mut m = manifest.clone();
                m["x-future"] = json!({ "k": [1] });
                m
            },
            base.clone(),
        ),
    ];
    Value::Array(
        cases
            .into_iter()
            .map(|(name, manifest, files)| {
                let parsed = Manifest::from_value(manifest.clone()).unwrap();
                json!({ "name": name, "manifest": manifest, "files": hashes(&files), "hash": signing::content_hash(&parsed, &hashes(&files)) })
            })
            .collect(),
    )
}

fn signature_cases() -> Value {
    let key = author();
    let base = files(&[("init.luau", b"return 1")]);
    let h = hashes(&base);
    let manifest =
        json!({ "id": "focus", "name": "Focus", "version": "1.0.0", "entry": "init.luau" });
    let signed = key.sign_manifest(&manifest, &h);
    let mut cases: Vec<(&str, &str, Value, FileHashes)> = vec![
        ("valid", PAYLOAD_PREFIX, signed.clone(), h.clone()),
        ("unsigned", PAYLOAD_PREFIX, manifest.clone(), h.clone()),
        (
            "an extra file",
            PAYLOAD_PREFIX,
            signed.clone(),
            hashes(&files(&[("init.luau", b"return 1"), ("evil.luau", b"x")])),
        ),
        (
            "a changed file",
            PAYLOAD_PREFIX,
            signed.clone(),
            hashes(&files(&[("init.luau", b"return 2")])),
        ),
        (
            "a missing file",
            PAYLOAD_PREFIX,
            signed.clone(),
            FileHashes::new(),
        ),
        (
            "a template's signature isn't a plugin's",
            PAYLOAD_PREFIX,
            key.sign_manifest_for(TEMPLATE_PAYLOAD_PREFIX, &manifest, &h),
            h.clone(),
        ),
        (
            "a plugin's signature isn't a template's",
            TEMPLATE_PAYLOAD_PREFIX,
            signed.clone(),
            h.clone(),
        ),
    ];
    let mut edited = signed.clone();
    edited["version"] = json!("2.0.0");
    cases.push((
        "a manifest field changed after signing",
        PAYLOAD_PREFIX,
        edited,
        h.clone(),
    ));
    let mut forged = signed.clone();
    forged["signature"]["sig"] = json!(key.sign(b"something else"));
    cases.push((
        "a signature over something else",
        PAYLOAD_PREFIX,
        forged,
        h.clone(),
    ));
    let mut swapped = signed.clone();
    swapped["signature"]["key"] = json!(other().public());
    cases.push(("another key named", PAYLOAD_PREFIX, swapped, h.clone()));
    let mut junk = signed.clone();
    junk["signature"] = json!("nope");
    cases.push((
        "a signature that isn't an object",
        PAYLOAD_PREFIX,
        junk,
        h.clone(),
    ));
    let mut short = signed.clone();
    short["signature"]["key"] = json!("AAAA");
    cases.push((
        "a key of the wrong length",
        PAYLOAD_PREFIX,
        short,
        h.clone(),
    ));
    let mut no_sig = signed.clone();
    no_sig["signature"].as_object_mut().unwrap().remove("sig");
    cases.push(("a signature without sig", PAYLOAD_PREFIX, no_sig, h.clone()));
    let mut padded = signed.clone();
    padded["signature"]["sig"] = json!(format!(
        "{}==",
        signed["signature"]["sig"].as_str().unwrap()
    ));
    cases.push((
        "base64url with padding isn't accepted",
        PAYLOAD_PREFIX,
        padded,
        h.clone(),
    ));
    Value::Array(
        cases
            .into_iter()
            .map(|(name, prefix, manifest, files)| {
                let result = signing::check_raw(prefix, &manifest, &files);
                let mut case = json!({ "name": name, "prefix": prefix, "manifest": manifest, "files": files, "result": sig_word(&result) });
                if let SigCheck::Valid(key) = &result {
                    case["key"] = json!(key);
                    case["payload"] = json!(String::from_utf8(signing::payload_for(prefix, &strip_sig(&manifest), key, &files)).unwrap());
                }
                case
            })
            .collect(),
    )
}

/// The manifest without `signature.sig` (what [`signing::payload_for`] is
/// given: it puts `signature = {key, files}` in itself).
fn strip_sig(manifest: &Value) -> Value {
    let mut m = manifest.clone();
    m.as_object_mut().unwrap().remove("signature");
    m
}

/// A package's raw bytes: `files` signed by `key` (none: unsigned).
fn packed(
    key: Option<&SigningKey>,
    manifest: &Value,
    files: &BTreeMap<String, Vec<u8>>,
) -> Vec<u8> {
    let manifest = match key {
        Some(key) => key.sign_manifest(manifest, &hashes(files)),
        None => manifest.clone(),
    };
    package::pack(FORMAT, &manifest, files).unwrap()
}

fn package_cases() -> Value {
    let key = author();
    let manifest = json!({ "id": "focus", "name": "Focus", "version": "1.0.0", "entry": "init.luau", "permissions": ["ui.add"] });
    let good = files(&[("init.luau", b"return 1"), ("ui/badge.luau", b"return {}")]);
    let mut cases: Vec<(String, Vec<u8>)> =
        vec![("valid".into(), packed(Some(&key), &manifest, &good))];
    let edit = |raw: &[u8], change: &dyn Fn(&mut Value)| {
        let mut value: Value = serde_json::from_slice(raw).unwrap();
        change(&mut value);
        serde_json::to_vec(&value).unwrap()
    };
    let valid = cases[0].1.clone();
    cases.push(("not JSON".into(), b"PK\x03\x04".to_vec()));
    cases.push((
        "another format".into(),
        edit(&valid, &|v| v["format"] = json!("zeron-template:v1")),
    ));
    cases.push((
        "files isn't an object".into(),
        edit(&valid, &|v| v["files"] = json!([])),
    ));
    cases.push(("unsigned".into(), packed(None, &manifest, &good)));
    cases.push((
        "a file changed after signing".into(),
        edit(&valid, &|v| {
            v["files"]["init.luau"] = json!(B64.encode(b"return 2"))
        }),
    ));
    cases.push((
        "a file added after signing".into(),
        edit(&valid, &|v| {
            v["files"]["extra.luau"] = json!(B64.encode(b"x"))
        }),
    ));
    cases.push((
        "a manifest field changed after signing".into(),
        edit(&valid, &|v| {
            v["manifest"]["permissions"] = json!(["ui.add", "network"])
        }),
    ));
    cases.push((
        "a file that isn't base64".into(),
        edit(&valid, &|v| v["files"]["init.luau"] = json!("@@@")),
    ));
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
    let mut no_entry = good.clone();
    no_entry.remove("init.luau");
    cases.push((
        "its entry file is missing".into(),
        packed(Some(&key), &manifest, &no_entry),
    ));
    let mut unknown = manifest.clone();
    unknown["permissions"] = json!(["root"]);
    cases.push((
        "a permission Zeron doesn't know".into(),
        packed(Some(&key), &unknown, &good),
    ));
    let mut reserved = manifest.clone();
    reserved["id"] = json!("zeron");
    cases.push((
        "an id Zeron keeps".into(),
        packed(Some(&key), &reserved, &good),
    ));
    let mut bad_id = manifest.clone();
    bad_id["id"] = json!("Focus");
    cases.push((
        "an id that isn't one".into(),
        packed(Some(&key), &bad_id, &good),
    ));
    let mut future = manifest.clone();
    future["x-future"] = json!({ "unknown": "fields are kept and signed" });
    cases.push((
        "unknown fields are kept".into(),
        packed(Some(&key), &future, &good),
    ));
    Value::Array(
        cases
            .into_iter()
            .map(|(name, raw)| {
                let mut case = json!({ "name": name, "package_b64": B64.encode(&raw), "sha256": signing::sha256_hex(&raw) });
                match install::parse(raw) {
                    Ok(p) => {
                        case["result"] = json!("ok");
                        case["id"] = json!(p.id());
                        case["key"] = json!(p.key);
                        case["content_hash"] = json!(p.hash);
                    }
                    Err(error) => {
                        // A refusal of the package itself carries its word; a
                        // manifest Zeron won't run is `bad_manifest`.
                        case["result"] = json!(install::reason_of(&error).unwrap_or("bad_manifest"));
                    }
                }
                case
            })
            .collect(),
    )
}

fn path_cases() -> Value {
    let paths = [
        "init.luau",
        "ui/badge.luau",
        "shots/board.png",
        "a-b_c.d/e",
        "README.md",
        "",
        "/etc/x",
        "../x",
        "a/../b",
        "a/./b",
        ".git/config",
        "a/.hidden",
        "a//b",
        "a/",
        "a\\b",
        "D:x.cmd",
        "C:../x",
        "CON",
        "con.txt",
        "com1.luau",
        "LPT9",
        "ui/NUL.luau",
        "AUX",
        "x.",
        "a. ",
        "a :b",
        "x:y",
    ];
    Value::Array(
        paths
            .iter()
            .map(|p| json!({ "path": p, "ok": package::plain_path(p) }))
            .collect(),
    )
}

fn semver_cases() -> Value {
    let inputs = [
        "0.0.0",
        "1.2.3",
        "1.0.0-alpha",
        "1.0.0-alpha.1",
        "1.0.0-0.3.7",
        "1.0.0-x.7.z.92",
        "1.0.0-x-y-z.--",
        "1.0.0+20130313144700",
        "1.0.0-beta+exp.sha.5114f85",
        "1.0.0+21AF26D3----117B344092BD",
        "1.0.0+001",
        "9007199254740991.9007199254740991.9007199254740991",
        "",
        "1",
        "1.2",
        "1.2.3.4",
        "v1.2.3",
        " 1.2.3",
        "1.2.3 ",
        "01.2.3",
        "1.02.3",
        "1.2.03",
        "1.2.3-",
        "1.2.3+",
        "1.2.3-01",
        "1.2.3-a..b",
        "1.2.3-a_b",
        "1.2.3+a..b",
        "1.2.3-é",
        "9007199254740992.0.0",
        "1.0.0-9007199254740992",
        "-1.0.0",
        "1.0.0++a",
        "1.0.0-+a",
    ];
    let long = format!("1.0.0-{}", "a".repeat(version::MAX_LEN - 6));
    let too_long = format!("{long}a");
    let mut parse: Vec<Value> = inputs
        .iter()
        .map(|i| json!({ "input": i, "ok": Version::parse(i).is_ok() }))
        .collect();
    parse.push(json!({ "input": long, "ok": Version::parse(&long).is_ok() }));
    parse.push(json!({ "input": too_long, "ok": Version::parse(&too_long).is_ok() }));
    let order = [
        "0.0.1",
        "0.1.0",
        "1.0.0-0",
        "1.0.0-1",
        "1.0.0-2",
        "1.0.0-10",
        "1.0.0-A",
        "1.0.0-a",
        "1.0.0-alpha",
        "1.0.0-alpha.1",
        "1.0.0-alpha.beta",
        "1.0.0-beta",
        "1.0.0-beta.2",
        "1.0.0-beta.11",
        "1.0.0-rc.1",
        "1.0.0",
        "1.0.1",
        "1.1.0",
        "2.0.0",
        "10.0.0",
    ];
    for pair in order.windows(2) {
        assert!(
            Version::parse(pair[0]).unwrap() < Version::parse(pair[1]).unwrap(),
            "{pair:?}"
        );
    }
    let compare = [
        ("1.0.0+a", "1.0.0+b"),
        ("1.0.0", "1.0.0+b"),
        ("1.0.0-a.1", "1.0.0-a.1.0"),
        ("1.0.0-1", "1.0.0-a"),
        ("2.0.0", "10.0.0"),
        ("1.0.0-rc.1", "1.0.0"),
    ];
    let compare: Vec<Value> = compare
        .iter()
        .map(|(a, b)| {
            let cmp = Version::parse(a).unwrap().cmp(&Version::parse(b).unwrap()) as i8;
            json!({ "a": a, "b": b, "cmp": cmp })
        })
        .collect();
    json!({ "parse": parse, "order": order, "compare": compare, "max_len": version::MAX_LEN, "max_number": version::MAX_NUMBER })
}

fn range_cases() -> Value {
    let cases: &[(&str, &[&str])] = &[
        (">=0.4.0", &["0.4.0", "0.3.9", "0.4.0-beta.1", "1.0.0"]),
        (">= 0.4.0 <0.6.0", &["0.5.9", "0.6.0", "0.6.0-0", "0.4.0"]),
        (">0.4.0", &["0.4.0", "0.4.1-0"]),
        ("<=1.0.0", &["1.0.0", "1.0.0+b", "1.0.1-0"]),
        ("<1.0.0", &["0.9.9", "1.0.0-rc.1", "1.0.0"]),
        (
            "^1.2.3",
            &["1.2.3", "1.9.0", "2.0.0-alpha", "2.0.0", "1.2.2"],
        ),
        ("^0.2.3", &["0.2.9", "0.3.0", "0.3.0-0", "0.2.2"]),
        ("^0.0.3", &["0.0.3", "0.0.4", "0.0.4-0"]),
        (
            "^1.2.3-beta.2",
            &["1.2.3-beta.2", "1.2.3-beta.1", "1.2.3", "1.3.0-alpha"],
        ),
        ("~1.2.3", &["1.2.9", "1.3.0", "1.3.0-rc.1", "1.2.3-rc.1"]),
        ("~0.0.3", &["0.0.9", "0.1.0"]),
        ("1.2.3", &["1.2.3", "1.2.3+build", "1.2.4"]),
        ("=1.2.3", &["1.2.3"]),
        ("*", &["0.0.1-alpha", "99.0.0"]),
        ("<0.3.0 || >=0.5.0", &["0.4.0", "0.5.1", "0.2.0"]),
        ("<0.3.0 || *", &["9.9.9"]),
        ("  >=1.0.0   <2.0.0  ", &["1.5.0"]),
        (">=1.0.0 <1.0.0", &["1.0.0"]),
    ];
    let mut out = Vec::new();
    for (range, versions) in cases {
        let parsed = Range::parse(range).unwrap();
        for v in *versions {
            out.push(json!({ "range": range, "version": v, "matches": parsed.matches(&Version::parse(v).unwrap()) }));
        }
    }
    for bad in [
        "",
        " ",
        "1.2",
        ">=",
        ">= ",
        "1.x",
        "1.2.3 - 2.0.0",
        "||",
        ">=1.0.0 ||",
        "^v1.0.0",
        ">=1.0.0\t<2.0.0",
        ">> 1.0.0",
        "* 1.0.0",
        ">=*",
        "latest",
        "~>1.0.0",
    ] {
        assert!(Range::parse(bad).is_err(), "{bad:?}");
        out.push(json!({ "range": bad, "error": true }));
    }
    let long = format!(">=1.0.0{}", " ".repeat(version::MAX_RANGE_LEN));
    assert!(Range::parse(&long).is_err());
    out.push(json!({ "range": long, "error": true }));
    Value::Array(out)
}

fn listing_cases() -> Value {
    let (base, files) = listing::tests::listed();
    let mut cases: Vec<(String, Value, BTreeMap<String, Vec<u8>>)> =
        vec![("meets every rule".into(), base.clone(), files.clone())];
    let changes: &[(&str, &str, Value)] = &[
        ("no category", "categories", Value::Null),
        ("an empty category list", "categories", json!([])),
        ("a category not on the list", "categories", json!(["games"])),
        (
            "three categories",
            "categories",
            json!(["git", "chat", "agents"]),
        ),
        ("a category twice", "categories", json!(["git", "git"])),
        ("a keyword in capitals", "keywords", json!(["Focus"])),
        (
            "nine keywords",
            "keywords",
            json!(["a", "b", "c", "d", "e", "f", "g", "h", "i"]),
        ),
        (
            "keywords with spaces and dashes",
            "keywords",
            json!(["code review", "git-flow"]),
        ),
        (
            "a keyword with two spaces",
            "keywords",
            json!(["code  review"]),
        ),
        ("an http homepage", "homepage", json!("http://example.com")),
        (
            "a repository with a space",
            "repository",
            json!("https://exa mple.com"),
        ),
        ("an empty license", "license", json!("")),
        (
            "a screenshot that isn't there",
            "screenshots",
            json!(["shots/missing.png"]),
        ),
        (
            "a screenshot that isn't a picture",
            "screenshots",
            json!(["init.luau"]),
        ),
        (
            "a screenshot outside the folder",
            "screenshots",
            json!(["../x.png"]),
        ),
        (
            "a partial engines range",
            "engines",
            json!({ "zeron": "0.4" }),
        ),
        ("engines as text", "engines", json!(">=0.4.0")),
        ("an icon that isn't an SVG", "icon", json!("README.md")),
        ("no icon", "icon", Value::Null),
        ("no description", "description", Value::Null),
        ("a description of two lines", "description", json!("a\nb")),
        ("a loose version", "version", json!("1.2")),
        ("an id starting zeron", "id", json!("zeron-tools")),
        ("a built-in plugin's id", "id", json!("github")),
        (
            "a permission Zeron doesn't know",
            "permissions",
            json!(["root"]),
        ),
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
    let mut no_readme = files.clone();
    no_readme.remove(listing::README);
    cases.push(("no README.md".into(), base.clone(), no_readme));
    let mut binary = files.clone();
    binary.insert("bin/tool.wasm".into(), b"\0asm\x01\0\0\0".to_vec());
    cases.push(("a binary file (a warning)".into(), base.clone(), binary));
    Value::Array(
        cases
            .into_iter()
            .map(|(name, manifest, files)| {
                let (problems, warnings) = listing::check(&Manifest::from_value(manifest.clone()).unwrap(), &files);
                let said = |list: &[listing::Problem]| -> Value {
                    list.iter().map(|p| json!({ "rule": p.rule, "field": p.field, "path": p.path })).collect()
                };
                json!({ "name": name, "manifest": manifest, "files_b64": files_b64(&files), "problems": said(&problems), "warnings": said(&warnings) })
            })
            .collect(),
    )
}

fn changelog_cases() -> Value {
    let log = "# Changelog\n\n## [1.2.0] - 2026-11-01\n\n### Added\n- Boards.\n\n## v1.1.0\n- First.\n\n## 1.0.0\n\n## 0.9.0\n- Old.\n";
    let versions = ["1.2.0", "1.1.0", "1.0.0", "0.9.0", "0.8.0"];
    Value::Array(
        versions
            .iter()
            .map(|v| json!({ "changelog": log, "version": v, "notes": listing::changelog_notes(log, v) }))
            .collect(),
    )
}

fn deploy_key_cases() -> Value {
    let key = DeployKey::from_seed([3; 32]).unwrap();
    let body = br#"{"format":"zeron-plugin:v1"}"#;
    let (method, at, ts) = (
        "POST",
        "/store/v1/plugins/focus/versions",
        1_763_000_000_000_i64,
    );
    let body_sha256 = signing::sha256_hex(body);
    json!({
        "secret_prefix": deploy_key::SECRET_PREFIX,
        "secret": key.secret(),
        "public": key.public(),
        "proof_message": deploy_key::proof_message("focus", &key.public()),
        "proof": key.proof("focus"),
        "max_skew_ms": deploy_key::MAX_SKEW_MS,
        "request": {
            "method": method, "path": at, "body_b64": B64.encode(body), "body_sha256": body_sha256, "ts": ts,
            "message": deploy_key::request_message(method, at, &body_sha256, ts),
            "authorization": key.authorization(method, at, body, ts),
        },
        "bad_secrets": [
            { "secret": "ghp_notours", "error": "not_a_secret" },
            { "secret": &key.secret()[..key.secret().len() - 2], "error": "damaged" },
        ],
    })
}

fn build() -> Value {
    json!({
        "format": "zeron-plugin-package-vectors:v1",
        "about": "Made by crates/engine/src/plugins/vectors.rs from the engine's own code; the edge's package verification must agree with every case. Keys are ed25519, base64url without padding; hashes are lowercase hex sha256; package_b64 and files_b64 are standard base64 of the exact bytes.",
        "package_format": FORMAT,
        "payload_prefix": PAYLOAD_PREFIX,
        "template_payload_prefix": TEMPLATE_PAYLOAD_PREFIX,
        "key_proof_prefix": signing::KEY_PROOF_PREFIX,
        "limits": {
            "max_files": super::files::MAX_FILES,
            "max_bytes": super::files::MAX_BYTES,
            "max_package_bytes": MAX_PACKAGE_BYTES,
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
            "screenshot_types": listing::SCREENSHOT_TYPES,
        },
        "categories": CATEGORIES.iter().map(|(id, name)| json!({ "id": id, "name": name })).collect::<Vec<_>>(),
        "builtin_ids": listing::BUILTIN_IDS,
        "canonical_json": canonical_cases(),
        "content_hash": content_hash_cases(),
        "signatures": signature_cases(),
        "packages": package_cases(),
        "paths": path_cases(),
        "semver": semver_cases(),
        "ranges": range_cases(),
        "listing": listing_cases(),
        "changelog": changelog_cases(),
        "deploy_key": deploy_key_cases(),
    })
}

#[test]
fn vectors_are_current() {
    let mut text = serde_json::to_string_pretty(&build()).unwrap();
    text.push('\n');
    let path = path();
    if std::env::var_os("UPDATE_VECTORS").is_some() {
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        on_disk == text,
        "docs/plugin-package-vectors.json is out of date with the engine's rules: run `UPDATE_VECTORS=1 cargo test -p zeron-engine vectors_are_current` and commit it (the edge's tests read it)"
    );
}

/// What the file says agrees with the code it came from, case by case (a
/// guard on this generator, not only on drift).
#[test]
fn vectors_say_what_the_code_does() {
    let v = build();
    for case in v["packages"].as_array().unwrap() {
        let raw = B64.decode(case["package_b64"].as_str().unwrap()).unwrap();
        assert_eq!(signing::sha256_hex(&raw), case["sha256"]);
    }
    let valid = &v["packages"][0];
    assert_eq!(
        (valid["result"].as_str(), valid["key"].as_str()),
        (Some("ok"), Some(author().public().as_str()))
    );
    let words: Vec<&str> = v["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["result"].as_str().unwrap())
        .collect();
    for word in [
        "not_a_package",
        "unsigned",
        "tampered",
        "bad_signature",
        "bad_file",
        "bad_manifest",
    ] {
        assert!(words.contains(&word), "{word} has a case");
    }
    let signatures: Vec<&str> = v["signatures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["result"].as_str().unwrap())
        .collect();
    for word in ["valid", "unsigned", "files_differ", "bad_signature"] {
        assert!(signatures.contains(&word), "{word} has a case");
    }
    assert!(
        v["paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["ok"] == true)
            && v["paths"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["ok"] == false)
    );
    let secret = v["deploy_key"]["secret"].as_str().unwrap();
    assert_eq!(
        DeployKey::from_secret(secret).unwrap().public(),
        v["deploy_key"]["public"]
    );
}
