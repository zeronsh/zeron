//! Newline-framed reading of a child's stdio that survives any bytes.
//!
//! `tokio::io::Lines` fails the whole stream on the first byte sequence that
//! is not UTF-8. A child's output is not ours to vouch for: Windows tools and
//! localized messages print in the console's ANSI/OEM code page, agents echo
//! raw tool output into log lines, and a multibyte character can be split by
//! a crash mid-write. With `Lines` one such byte ended a stderr drain — the
//! read end then closed, so the child's next write hit a broken pipe — or
//! ended a protocol reader as though the agent had exited. Here a bad byte
//! becomes U+FFFD in that one line and reading continues.

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

pub(crate) struct LossyLines<R> {
    reader: BufReader<R>,
    /// Bytes of the line being assembled. Kept across calls: a `next_line`
    /// cancelled inside `select!` resumes the same line on the next call.
    pending: Vec<u8>,
}

impl<R: AsyncRead + Unpin> LossyLines<R> {
    pub(crate) fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
            pending: Vec::new(),
        }
    }

    /// The next line without its `\n` / `\r\n`, invalid UTF-8 replaced.
    /// `Ok(None)` at end of stream; a final unterminated line is returned
    /// before that. Cancel safe.
    pub(crate) async fn next_line(&mut self) -> std::io::Result<Option<String>> {
        let read = self.reader.read_until(b'\n', &mut self.pending).await?;
        if read == 0 && self.pending.is_empty() {
            return Ok(None);
        }
        let mut line = std::mem::take(&mut self.pending);
        if line.last() == Some(&b'\n') {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
        }
        Ok(Some(match String::from_utf8(line) {
            Ok(line) => line,
            Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
        }))
    }
}

pub(crate) fn lossy_lines<R: AsyncRead + Unpin>(reader: R) -> LossyLines<R> {
    LossyLines::new(reader)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalid_utf8_crlf_and_unterminated_lines_keep_reading() {
        let bytes: &[u8] = b"one\r\nC:\\Users\\J\xf6rg\\x\nthree\xe2\x82\nlast";
        let mut lines = lossy_lines(bytes);
        assert_eq!(lines.next_line().await.unwrap().as_deref(), Some("one"));
        assert_eq!(
            lines.next_line().await.unwrap().as_deref(),
            Some("C:\\Users\\J\u{fffd}rg\\x")
        );
        assert_eq!(
            lines.next_line().await.unwrap().as_deref(),
            Some("three\u{fffd}")
        );
        assert_eq!(lines.next_line().await.unwrap().as_deref(), Some("last"));
        assert_eq!(lines.next_line().await.unwrap(), None);
        assert_eq!(lines.next_line().await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_cancelled_read_resumes_the_same_line() {
        let (mut tx, rx) = tokio::io::duplex(64);
        let mut lines = lossy_lines(rx);
        use tokio::io::AsyncWriteExt;
        tx.write_all(b"{\"half\":").await.unwrap();
        // Cancelled mid-line: the partial bytes must not be lost.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), lines.next_line())
                .await
                .is_err()
        );
        tx.write_all(b"1}\n").await.unwrap();
        assert_eq!(
            lines.next_line().await.unwrap().as_deref(),
            Some("{\"half\":1}")
        );
    }
}
