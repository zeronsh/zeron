// Compile the same protocol limits that the edge imports directly.
fn main() {
    println!("cargo:rerun-if-changed=chat2-limits.json");
    let limits: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string("chat2-limits.json").expect("chat2 protocol limits"),
    )
    .expect("valid chat2 protocol limits");
    let mut rust = String::new();
    for (field, name) in [
        ("maxCheckpointBytes", "MAX_CHECKPOINT_BYTES"),
        ("maxPushBytes", "MAX_PUSH_BYTES"),
    ] {
        let value = limits[field]
            .as_u64()
            .filter(|n| *n > 0 && *n <= u32::MAX as u64)
            .expect("positive 32-bit protocol limit");
        rust.push_str(&format!("pub const {name}: usize = {value};\n"));
    }
    std::fs::write(
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("chat2_limits.rs"),
        rust,
    )
    .unwrap();
}
