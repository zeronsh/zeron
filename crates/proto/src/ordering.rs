//! Dense fractional ordering shared by sidebar collections.
pub const MAX_ORDER_KEY_BYTES: usize = 8192;
const NONCE_BYTES: usize = 149; // Maximum wire HLC: 13 + 1 + 6 + 1 + 128.

pub fn valid_order_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_ORDER_KEY_BYTES
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && !key.ends_with('0')
}

/// Allocate a whole prefix interval strictly between the bounds, then append
/// a fixed-width encoding of the unique operation HLC. Unlike random jitter,
/// different operation clocks cannot collide, even while writers are offline.
pub fn order_key_between(
    lower: Option<&str>,
    upper: Option<&str>,
    nonce: &str,
) -> Result<String, &'static str> {
    if lower.is_some_and(|k| !valid_order_key(k))
        || upper.is_some_and(|k| !valid_order_key(k))
        || lower.zip(upper).is_some_and(|(a, b)| a >= b)
        || nonce.is_empty()
        || nonce.len() > NONCE_BYTES
        || !nonce.is_ascii()
        || nonce.as_bytes().contains(&0)
    {
        return Err("Invalid ordering bounds");
    }
    let lower = lower.unwrap_or("").as_bytes();
    let mut upper = upper.map(str::as_bytes);
    let mut key = String::new();
    let digit = |b: u8| if b <= b'9' { b - b'0' } else { b - b'a' + 10 };
    let hex = b"0123456789abcdef";
    for i in 0..MAX_ORDER_KEY_BYTES {
        let lo = lower.get(i).copied().map(digit).unwrap_or(0);
        let hi = upper
            .and_then(|u| u.get(i))
            .copied()
            .map(digit)
            .unwrap_or(16);
        if hi > lo + 1 {
            key.push(hex[((lo + hi) / 2) as usize] as char);
            for i in 0..NONCE_BYTES {
                let byte = nonce.as_bytes().get(i).copied().unwrap_or(0);
                key.push(hex[(byte >> 4) as usize] as char);
                key.push(hex[(byte & 15) as usize] as char);
            }
            key.push('8');
            return if key.len() <= MAX_ORDER_KEY_BYTES {
                Ok(key)
            } else {
                Err("Ordering key is too long")
            };
        }
        key.push(hex[lo as usize] as char);
        if lo != hi {
            upper = None;
        }
    }
    Err("Ordering key is too long")
}
