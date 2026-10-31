use crate::HarnessError;
pub(crate) use crate::catalog::Catalog;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

/// The SDK's saved login (`Cursor.auth.login`'s default store).
pub(crate) fn sdk_auth_file() -> std::path::PathBuf {
    crate::executable::home_or_current_dir().join(".cursor/sdk/auth.json")
}

/// Hash (never log) the SDK's auth inputs. Contents, not mtime, detect atomic
/// account swaps. Expiry crossing also invalidates the prior account's cache.
pub(crate) fn credential_context() -> Result<[u8; 32], HarnessError> {
    let bytes = match std::fs::read(sdk_auth_file()) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    let expiry = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| v.get("apiKeyExpiresAtMs").and_then(|v| v.as_u64()));
    let expired = expiry.is_some_and(|at| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            >= u128::from(at)
    });
    let mut hash = Sha256::new();
    hash.update(bytes);
    hash.update([u8::from(expired)]);
    let mut env: Vec<_> = std::env::vars_os()
        .filter(|(key, _)| key.to_string_lossy().starts_with("CURSOR_"))
        .collect();
    env.sort();
    for (key, value) in env {
        hash.update(key.as_encoded_bytes());
        hash.update([0]);
        hash.update(value.as_encoded_bytes());
        hash.update([0]);
    }
    Ok(hash.finalize().into())
}
