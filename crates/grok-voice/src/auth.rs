//! Local Grok Bot session. No API key. The access token and secret file are
//! never logged.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::Value;
use zeron_proto::voice::VoiceRejection;

const V10_KEY: [u8; 16] = [
    0xfd, 0x62, 0x1f, 0xe5, 0xa2, 0xb4, 0x02, 0x53, 0x9d, 0xfa, 0x14, 0x7c, 0xa9, 0x27, 0x27, 0x78,
];
const V10_IV: [u8; 16] = [b' '; 16];
const MINT_URL: &str = "https://api2.cursor.sh/aiserver.v1.GrokBotService/MintSandVoiceCallSecret";
const CLIENT_VERSION: &str = "0.78.1";

pub struct GrokBotCredentials;

#[async_trait::async_trait]
pub trait Credentials: Send + Sync {
    async fn mint(&self) -> Result<Minted, VoiceRejection>;
}

pub struct Minted {
    pub client_secret: String,
    pub model: String,
    pub websocket_url: String,
}

impl std::fmt::Debug for Minted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Minted")
            .field("model", &self.model)
            .field("websocket_url", &self.websocket_url)
            .finish_non_exhaustive()
    }
}

/// True only when a local access token is present and its `exp` is missing,
/// unreadable, or still in the future. No network and no signature check.
pub fn signed_in() -> bool {
    session_parts().is_some_and(|(token, _)| jwt_current(&token))
}

#[async_trait::async_trait]
impl Credentials for GrokBotCredentials {
    async fn mint(&self) -> Result<Minted, VoiceRejection> {
        let Some((token, machine_id)) = session_parts() else {
            return Err(VoiceRejection::GrokSignInRequired);
        };
        if !jwt_current(&token) {
            return Err(VoiceRejection::GrokSignInRequired);
        }
        request_mint(&token, &machine_id).await
    }
}

fn session_parts() -> Option<(String, String)> {
    let text = std::fs::read_to_string(secrets_path()?).ok()?;
    let root: Value = serde_json::from_str(&text).ok()?;
    let accounts = root.get("cursor-accounts").and_then(Value::as_str)?;
    let token = account_access_token(accounts)?;
    let machine = root.get("cursor-machine-id").and_then(Value::as_str)?;
    let machine_id = decrypt_v10(machine).ok()?;
    if machine_id.is_empty() {
        return None;
    }
    Some((token, machine_id))
}

fn secrets_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let root = std::env::var_os("APPDATA")?;
        Some(
            PathBuf::from(root)
                .join("Grok Bot")
                .join("sand-secrets.json"),
        )
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("Grok Bot")
                .join("sand-secrets.json"),
        )
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join(".config")
                .join("Grok Bot")
                .join("sand-secrets.json"),
        )
    }
}

fn account_access_token(raw: &str) -> Option<String> {
    let text = decrypt_v10(raw).unwrap_or_else(|_| raw.to_string());
    let mut value: Value = serde_json::from_str(&text).ok()?;
    if let Some(inner) = value.as_str() {
        value = serde_json::from_str(inner).ok()?;
    }
    let account = pick_account(&value)?;
    let token_raw = account.get("cursor-access-token")?.as_str()?;
    let token = if let Ok(plain) = decrypt_v10(token_raw) {
        plain
    } else if token_raw.starts_with("eyJ") {
        token_raw.to_string()
    } else {
        return None;
    };
    token.starts_with("eyJ").then_some(token)
}

fn pick_account(root: &Value) -> Option<&Value> {
    let accounts = root.get("accounts")?.as_object()?;
    if let Some(id) = root.get("active").and_then(Value::as_str) {
        if let Some(account) = accounts.get(id) {
            return Some(account);
        }
    }
    if let Some(account) = accounts.get("active") {
        if account.get("cursor-access-token").is_some() {
            return Some(account);
        }
    }
    accounts
        .values()
        .find(|account| account.get("active").and_then(Value::as_bool) == Some(true))
        .or_else(|| accounts.values().next())
}

fn jwt_current(token: &str) -> bool {
    // `None`: no payload (or the signature / JSON around it). `Some(None)`:
    // `exp` is missing or not a number, which still counts as signed in.
    fn exp_claim(token: &str) -> Option<Option<u64>> {
        let mut parts = token.split('.');
        let _header = parts.next()?;
        let payload = parts.next()?;
        if parts.next().is_none() {
            return None;
        }
        let bytes = b64url(payload)?;
        let value: Value = serde_json::from_slice(&bytes).ok()?;
        Some(match value.get("exp") {
            None | Some(Value::Null) => None,
            Some(exp) => json_u64(exp),
        })
    }

    if !token.starts_with("eyJ") {
        return false;
    }
    match exp_claim(token) {
        None => false,
        Some(None) => true,
        Some(Some(exp)) => exp > unix_now(),
    }
}

fn b64url(input: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(input))
        .ok()
}

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().filter(|n| *n >= 0).map(|n| n as u64))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn decrypt_v10(encoded: &str) -> Result<String, ()> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|_| ())?;
    if raw.len() < 3 + 16 || &raw[..3] != b"v10" {
        return Err(());
    }
    let mut buf = raw[3..].to_vec();
    let plain = cbc::Decryptor::<aes::Aes128>::new(&V10_KEY.into(), &V10_IV.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|_| ())?;
    String::from_utf8(plain.to_vec()).map_err(|_| ())
}

/// `x-cursor-checksum`. `t = now_ms / 1_000_000` mixed into six bytes.
pub(crate) fn checksum(now_ms: u128, machine_id: &str) -> String {
    let t = (now_ms / 1_000_000) as u64;
    let mut bytes = [
        ((t >> 40) & 255) as u8,
        ((t >> 32) & 255) as u8,
        ((t >> 24) & 255) as u8,
        ((t >> 16) & 255) as u8,
        ((t >> 8) & 255) as u8,
        (t & 255) as u8,
    ];
    let mut x: u16 = 165;
    for (i, byte) in bytes.iter_mut().enumerate() {
        let mixed = ((*byte as u16) ^ x) + (i as u16);
        *byte = (mixed & 255) as u8;
        x = u16::from(*byte);
    }
    let prefix = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    format!("{prefix}{machine_id}")
}

fn status_only(code: u16) -> anyhow::Error {
    anyhow::anyhow!("mint failed: {code}")
}

async fn request_mint(token: &str, machine_id: &str) -> Result<Minted, VoiceRejection> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|_| VoiceRejection::Protocol)?;
    let response = client
        .post(MINT_URL)
        .header("connect-protocol-version", "1")
        .header("authorization", format!("Bearer {token}"))
        .header("x-cursor-checksum", checksum(now_ms, machine_id))
        .header("x-ghost-mode", "true")
        .header("x-cursor-client-type", "sand")
        .header("x-cursor-client-version", CLIENT_VERSION)
        .header("x-cursor-client-os", client_os())
        .header("x-cursor-client-source", "sand-desktop")
        .header("user-agent", user_agent())
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|_| VoiceRejection::Protocol)?;
    let status = response.status();
    if !status.is_success() {
        let code = status.as_u16();
        drop(response);
        let err = status_only(code);
        tracing::warn!(error = %err, "voice mint failed");
        return Err(if code == 401 {
            VoiceRejection::GrokSignInRequired
        } else {
            VoiceRejection::Protocol
        });
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| VoiceRejection::Protocol)?;
    let parsed: MintBody = serde_json::from_slice(&bytes).map_err(|_| VoiceRejection::Protocol)?;
    if !parsed.websocket_url.starts_with("wss://api.x.ai/") {
        return Err(VoiceRejection::Protocol);
    }
    Ok(Minted {
        client_secret: parsed.client_secret,
        model: parsed.model,
        websocket_url: parsed.websocket_url,
    })
}

#[derive(Deserialize)]
struct MintBody {
    #[serde(rename = "clientSecret")]
    client_secret: String,
    model: String,
    #[serde(rename = "websocketUrl")]
    websocket_url: String,
}

fn client_os() -> &'static str {
    match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

fn user_agent() -> &'static str {
    match std::env::consts::OS {
        "macos" => {
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Sand/0.78.1 Safari/537.36"
        }
        "windows" => {
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Sand/0.78.1 Safari/537.36"
        }
        _ => {
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Sand/0.78.1 Safari/537.36"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockEncryptMut;

    fn encrypt_v10(plain: &[u8]) -> String {
        let mut buf = vec![0u8; plain.len() + 16];
        buf[..plain.len()].copy_from_slice(plain);
        let len = {
            let encrypted = cbc::Encryptor::<aes::Aes128>::new(&V10_KEY.into(), &V10_IV.into())
                .encrypt_padded_mut::<Pkcs7>(&mut buf, plain.len())
                .expect("encrypt");
            encrypted.len()
        };
        let mut blob = b"v10".to_vec();
        blob.extend_from_slice(&buf[..len]);
        base64::engine::general_purpose::STANDARD.encode(blob)
    }

    fn token(payload: &str) -> String {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.as_bytes());
        format!("{header}.{body}.sig")
    }

    #[test]
    fn checksum_prefixes_the_mixed_time() {
        // `(byte ^ x) + i`, then x becomes that byte. t = 0 starts at
        // six zero bytes and yields [165, 166, 168, 171, 175, 180].
        let bytes = [165u8, 166, 168, 171, 175, 180];
        let machine = "machine-1";
        let prefix = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        assert_eq!(checksum(0, machine), format!("{prefix}{machine}"));
        assert_eq!(checksum(0, machine), checksum(999_999, machine));
        assert_eq!(checksum(4_200_000, machine), checksum(4_200_000, machine));
        assert_ne!(checksum(0, machine), checksum(1_000_000, machine));
    }

    #[test]
    fn decrypt_v10_round_trips_with_the_peanuts_key() {
        let encoded = encrypt_v10(b"round-trip");
        assert_eq!(decrypt_v10(&encoded).as_deref(), Ok("round-trip"));
        assert!(decrypt_v10("not-a-blob").is_err());
    }

    #[test]
    fn accounts_parser_reads_plaintext_and_double_encoded_json() {
        let plain = serde_json::json!({
            "active": "live",
            "accounts": {
                "stale": {"cursor-access-token": "eyJstale.payload.sig"},
                "live": {"cursor-access-token": "eyJlive.payload.sig"}
            }
        })
        .to_string();
        assert_eq!(
            account_access_token(&plain).as_deref(),
            Some("eyJlive.payload.sig")
        );
        let double = serde_json::Value::String(plain).to_string();
        assert_eq!(
            account_access_token(&double).as_deref(),
            Some("eyJlive.payload.sig")
        );
        let only = r#"{"accounts":{"only":{"cursor-access-token":"eyJonly.payload.sig"}}}"#;
        assert_eq!(
            account_access_token(only).as_deref(),
            Some("eyJonly.payload.sig")
        );
        let blob =
            encrypt_v10(br#"{"accounts":{"only":{"cursor-access-token":"eyJblob.payload.sig"}}}"#);
        assert_eq!(
            account_access_token(&blob).as_deref(),
            Some("eyJblob.payload.sig")
        );
    }

    #[test]
    fn missing_exp_is_signed_in_and_a_past_exp_is_not() {
        assert!(jwt_current(&token("{}")));
        assert!(jwt_current(&token(r#"{"exp":4000000000}"#)));
        assert!(!jwt_current(&token(r#"{"exp":1}"#)));
        assert!(!jwt_current("not-a-jwt"));
    }

    #[test]
    fn minted_debug_omits_the_secret() {
        let minted = Minted {
            client_secret: "xai-real-secret".into(),
            model: "test".into(),
            websocket_url: "ws://127.0.0.1/v1/realtime".into(),
        };
        let rendered = format!("{minted:?}");
        assert!(!rendered.contains("xai-real-secret"));
        assert!(!rendered.contains("client_secret"));
        assert!(rendered.contains("test"));
    }
}
