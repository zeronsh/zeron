//! An owned, exportable newline-framed reader.
//!
//! `BufReader` + `Lines` hold a read-ahead and a partial line that cannot be
//! taken back out, so a run that has to hand its child's stdout to another
//! process image (a live update `execve`) would lose bytes. [`LineReader`]
//! keeps every byte it has read but not yet returned in one visible buffer:
//!
//! - [`LineReader::next_line`] is cancel-safe. Bytes land in the buffer as soon
//!   as a read completes, so dropping the future (a `select!` arm losing, a
//!   freeze request arriving) never loses data.
//! - [`LineReader::into_parts`] returns the reader plus every unconsumed byte
//!   (complete lines not yet handed out and the trailing partial line), and
//!   [`LineReader::with_leftover`] replays such bytes before reading again.
//!
//! Decoding is deliberately lossy: a line that is not valid UTF-8 has its bad
//! bytes replaced with U+FFFD rather than failing the read. tokio's `Lines`
//! returns an `InvalidData` error there, which would end a run over one stray
//! byte from an agent or a tool it relays. A read error from the underlying
//! stream is still returned as an error.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt};

/// Minimum free space kept ahead of each read.
const READ_CHUNK: usize = 8 * 1024;
/// A buffer that once held a very long line gives its capacity back when it
/// empties, down to this.
const IDLE_CAPACITY: usize = 64 * 1024;

/// There is deliberately no line-length cap: like tokio's `Lines`, a child that
/// never sends `\n` grows the buffer (the agent protocols are line-framed JSON
/// and a truncated message would be worse than a large one).
pub struct LineReader<R> {
    inner: R,
    /// `buf[start..]` are the bytes read but not yet returned; it always
    /// begins at a line start. Returned lines only advance `start`; the
    /// consumed prefix is dropped in one go before the next read, so many
    /// lines in one read cost one move, not one per line.
    buf: Vec<u8>,
    start: usize,
    /// `buf[start..start + scanned]` is known to contain no `\n`, so a long
    /// partial line is not rescanned on every read.
    scanned: usize,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    pub fn new(inner: R) -> Self {
        Self::with_leftover(inner, Vec::new())
    }

    /// Resume reading `inner`, first replaying `leftover` (bytes a previous
    /// reader had taken off the stream but not yet returned as lines).
    pub fn with_leftover(inner: R, leftover: Vec<u8>) -> Self {
        Self {
            inner,
            buf: leftover,
            start: 0,
            scanned: 0,
        }
    }

    /// The next line without its `\n` (or `\r\n`), lossily decoded. At EOF an
    /// unterminated final line is returned once, then `None`.
    ///
    /// Cancel-safe: dropping the returned future loses nothing.
    pub async fn next_line(&mut self) -> io::Result<Option<String>> {
        loop {
            let from = self.start + self.scanned;
            if let Some(offset) = self.buf[from..].iter().position(|b| *b == b'\n') {
                let end = from + offset;
                let mut line = self.buf[self.start..end].to_vec();
                self.start = end + 1;
                self.scanned = 0;
                self.reclaim_if_empty();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(Some(decode(line)));
            }
            self.scanned = self.buf.len() - self.start;
            self.compact();
            if self.buf.capacity() - self.buf.len() < READ_CHUNK {
                self.buf.reserve(READ_CHUNK);
            }
            // `read_buf` appends to `buf` in the same poll that completes, so
            // there is no await point between bytes leaving the fd and
            // landing in the buffer.
            if self.inner.read_buf(&mut self.buf).await? == 0 {
                if self.buf.len() == self.start {
                    return Ok(None);
                }
                let mut line = self.buf[self.start..].to_vec();
                self.buf.clear();
                self.start = 0;
                self.scanned = 0;
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(Some(decode(line)));
            }
        }
    }

    /// Drop the consumed prefix so `buf` starts at the unconsumed bytes.
    fn compact(&mut self) {
        if self.start > 0 {
            self.buf.drain(..self.start);
            self.start = 0;
        }
    }

    fn reclaim_if_empty(&mut self) {
        if self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
            self.buf.shrink_to(IDLE_CAPACITY);
        }
    }

    /// Every byte read off the stream but not yet returned as a line — complete
    /// lines still buffered, then the partial line — WITHOUT consuming them.
    /// A freeze exports this while the reader stays intact, so a thaw simply
    /// carries on.
    pub fn leftover(&self) -> &[u8] {
        &self.buf[self.start..]
    }

    /// The underlying reader (a freeze reads its fd number from it).
    pub fn get_ref(&self) -> &R {
        &self.inner
    }

    /// Give back the underlying reader and every byte not yet returned as a
    /// line: complete lines still buffered, then the partial line.
    pub fn into_parts(mut self) -> (R, Vec<u8>) {
        self.compact();
        (self.inner, self.buf)
    }
}

fn decode(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn line_reader_reassembles_split_lines_and_is_cancel_safe() {
        let (mut tx, rx) = tokio::io::duplex(64);
        let mut r = LineReader::new(rx);
        tx.write_all(b"{\"a\":1}\n{\"b\"").await.unwrap();
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("{\"a\":1}"));
        // cancel while a partial line is buffered
        assert!(
            tokio::time::timeout(Duration::from_millis(50), r.next_line())
                .await
                .is_err()
        );
        tx.write_all(b":2}\n").await.unwrap();
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("{\"b\":2}"));
    }

    #[tokio::test]
    async fn cancelling_next_line_mid_partial_line_loses_no_bytes() {
        let (mut tx, rx) = tokio::io::duplex(64);
        let mut r = LineReader::new(rx);
        for chunk in [&b"ab"[..], b"cd", b"ef"] {
            tx.write_all(chunk).await.unwrap();
            // Each poll reads the fresh bytes into the buffer, then is dropped.
            assert!(
                tokio::time::timeout(Duration::from_millis(20), r.next_line())
                    .await
                    .is_err()
            );
        }
        tx.write_all(b"\n").await.unwrap();
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("abcdef"));
    }

    #[tokio::test]
    async fn into_parts_returns_the_partial_line_and_unread_complete_lines() {
        let (mut tx, rx) = tokio::io::duplex(64);
        let mut r = LineReader::new(rx);
        tx.write_all(b"one\ntwo\npart").await.unwrap();
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("one"));
        let (_rx, leftover) = r.into_parts();
        assert_eq!(leftover, b"two\npart");
    }

    #[tokio::test]
    async fn with_leftover_replays_bytes_before_reading_the_fd() {
        let (mut tx, rx) = tokio::io::duplex(64);
        let mut r = LineReader::with_leftover(rx, b"two\npar".to_vec());
        tx.write_all(b"t\n").await.unwrap();
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("two"));
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("part"));
    }

    #[tokio::test]
    async fn leftover_round_trips_through_into_parts_and_with_leftover() {
        let (mut tx, rx) = tokio::io::duplex(64);
        let mut r = LineReader::new(rx);
        tx.write_all(b"one\ntwo\npar").await.unwrap();
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("one"));
        let (rx, leftover) = r.into_parts();
        let mut r = LineReader::with_leftover(rx, leftover);
        tx.write_all(b"t\n").await.unwrap();
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("two"));
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("part"));
    }

    #[tokio::test]
    async fn leftover_is_a_non_consuming_view_a_thaw_can_carry_on_from() {
        let (mut tx, rx) = tokio::io::duplex(64);
        let mut r = LineReader::new(rx);
        tx.write_all(b"one\ntwo\npar").await.unwrap();
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("one"));
        assert_eq!(r.leftover(), b"two\npar");
        assert_eq!(r.leftover(), b"two\npar", "reading it changes nothing");
        // Carry on as if nothing had been exported.
        tx.write_all(b"t\n").await.unwrap();
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("two"));
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("part"));
        assert!(r.leftover().is_empty());
    }

    #[tokio::test]
    async fn many_lines_in_one_read_come_out_in_order_and_cheaply() {
        let (mut tx, rx) = tokio::io::duplex(8 * 1024 * 1024);
        let mut r = LineReader::new(rx);
        let all: String = (0..200_000).map(|i| format!("line-{i}\n")).collect();
        tx.write_all(all.as_bytes()).await.unwrap();
        drop(tx);
        let started = std::time::Instant::now();
        for i in 0..200_000 {
            assert_eq!(
                r.next_line().await.unwrap().as_deref(),
                Some(format!("line-{i}").as_str())
            );
        }
        assert_eq!(r.next_line().await.unwrap(), None);
        // A per-line drain of the whole buffer would take many seconds here.
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn the_buffer_gives_its_capacity_back_after_a_huge_line() {
        let (mut tx, rx) = tokio::io::duplex(1024);
        let mut r = LineReader::new(rx);
        let big = "x".repeat(2 * 1024 * 1024);
        let writer = tokio::spawn(async move {
            tx.write_all(big.as_bytes()).await.unwrap();
            tx.write_all(b"\nnext\n").await.unwrap();
        });
        assert_eq!(r.next_line().await.unwrap().unwrap().len(), 2 * 1024 * 1024);
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("next"));
        writer.await.unwrap();
        assert!(r.buf.capacity() <= IDLE_CAPACITY, "{}", r.buf.capacity());
    }

    #[tokio::test]
    async fn eof_yields_the_unterminated_tail_then_none() {
        let (mut tx, rx) = tokio::io::duplex(64);
        let mut r = LineReader::new(rx);
        tx.write_all(b"a\r\ntail").await.unwrap();
        drop(tx);
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("a"));
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("tail"));
        assert_eq!(r.next_line().await.unwrap(), None);
        assert_eq!(r.next_line().await.unwrap(), None);
    }

    #[tokio::test]
    async fn invalid_utf8_is_replaced_instead_of_failing_the_read() {
        let (mut tx, rx) = tokio::io::duplex(64);
        let mut r = LineReader::new(rx);
        tx.write_all(b"ok \xff\xfe bytes\nnext\n").await.unwrap();
        assert_eq!(
            r.next_line().await.unwrap().as_deref(),
            Some("ok \u{fffd}\u{fffd} bytes")
        );
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("next"));
    }

    #[tokio::test]
    async fn lines_larger_than_one_read_chunk_are_reassembled() {
        let (mut tx, rx) = tokio::io::duplex(1024);
        let mut r = LineReader::new(rx);
        let big = "x".repeat(40_000);
        let expected = big.clone();
        let writer = tokio::spawn(async move {
            tx.write_all(big.as_bytes()).await.unwrap();
            tx.write_all(b"\nsecond\n").await.unwrap();
        });
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some(&*expected));
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("second"));
        writer.await.unwrap();
    }
}
