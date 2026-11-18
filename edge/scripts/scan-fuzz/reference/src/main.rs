// Reads records (u32 LE length + bytes) from argv[1]; for each prints one line:
// {"doc":bool,"pkg":null|{format, manifest, files:[[k, hex|null]]}}
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde_json::{json, Value};
use std::io::Write;
fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }
fn main() {
    let data = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
    let mut out = std::io::BufWriter::new(std::io::stdout());
    let mut at = 0;
    while at < data.len() {
        let n = u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        let raw = &data[at + 4..at + 4 + n];
        at += 4 + n;
        let parsed: Result<Value, _> = serde_json::from_slice(raw);
        let line = match parsed {
            Err(_) => json!({"doc": false, "pkg": null}),
            Ok(v) => {
                let pkg = if v.is_object() {
                    let format = v["format"].as_str().map(|s| s.to_string());
                    let manifest = if v["manifest"].is_object() { Some(serde_json::to_string(&v["manifest"]).unwrap()) } else { None };
                    let files = v["files"].as_object().map(|m| {
                        m.iter().map(|(k, d)| json!([k, d.as_str().and_then(|s| B64.decode(s).ok()).map(|b| hex(&b))])).collect::<Vec<_>>()
                    });
                    json!({"format": format, "manifest": manifest, "files": files})
                } else { Value::Null };
                json!({"doc": true, "pkg": pkg})
            }
        };
        writeln!(out, "{}", line).unwrap();
    }
}
