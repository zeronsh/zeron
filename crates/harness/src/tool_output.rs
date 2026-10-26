//! Tool output as harnesses hand it to the engine: capped, head and tail
//! kept. Only gen-3 chats store it (docs/chat3-sync.md); gen-2 folds keep
//! outputs out of every doc, as before.

/// Cap for one tool's output. Build logs can run to megabytes; the head and
/// the tail are where errors live.
pub const TOOL_OUTPUT_CAP: usize = 256 * 1024;

/// `text` within [`TOOL_OUTPUT_CAP`], the middle replaced by a marker.
pub fn cap_tool_output(text: &str) -> String {
    if text.len() <= TOOL_OUTPUT_CAP {
        return text.to_owned();
    }
    let half = TOOL_OUTPUT_CAP / 2;
    let mut head = half;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - half;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!(
        "{}\n… [{} bytes truncated] …\n{}",
        &text[..head],
        tail - head,
        &text[tail..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_output_is_untouched_and_long_output_keeps_both_ends() {
        assert_eq!(cap_tool_output("ok"), "ok");
        let long = format!("START{}END", "é".repeat(TOOL_OUTPUT_CAP));
        let capped = cap_tool_output(&long);
        assert!(capped.len() < TOOL_OUTPUT_CAP + 64);
        assert!(capped.starts_with("START"));
        assert!(capped.ends_with("END"));
        assert!(capped.contains("bytes truncated"));
    }
}
