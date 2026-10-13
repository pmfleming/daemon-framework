//! Bounded, cancellation-safe record framing. Never parse a truncated request.
use std::io;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Frame {
    Line(String),
    TooLarge,
}

pub(super) struct Frames<R> {
    reader: R,
    bytes: Vec<u8>,
    maximum: usize,
    oversized: bool,
}
impl<R: AsyncBufRead + Unpin> Frames<R> {
    pub(super) fn new(reader: R, maximum: usize) -> Self {
        assert!(maximum < usize::MAX);
        Self {
            reader,
            bytes: Vec::new(),
            maximum,
            oversized: false,
        }
    }

    pub(super) async fn next(&mut self) -> io::Result<Option<Frame>> {
        loop {
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                return if self.oversized || !self.bytes.is_empty() {
                    self.finish(false).map(Some)
                } else {
                    Ok(None)
                };
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let count = newline.unwrap_or(available.len());
            if !self.oversized {
                // One extra byte permits CR at the end of a maximum-sized CRLF
                // record, even when the newline arrives in the next read.
                self.oversized =
                    !extend_bounded(&mut self.bytes, &available[..count], self.maximum + 1);
            }
            self.reader.consume(count + usize::from(newline.is_some()));
            if newline.is_some() {
                return self.finish(true).map(Some);
            }
            // Oversized input is drained, without retaining it, until LF/EOF.
            // The next record starts fresh; no suffix is interpreted as a call.
        }
    }

    fn finish(&mut self, newline: bool) -> io::Result<Frame> {
        if newline && self.bytes.last() == Some(&b'\r') {
            self.bytes.pop();
        }
        if self.oversized || self.bytes.len() > self.maximum {
            self.bytes.clear();
            self.oversized = false;
            return Ok(Frame::TooLarge);
        }
        String::from_utf8(std::mem::take(&mut self.bytes))
            .map(Frame::Line)
            // Keep the existing fatal I/O behavior for invalid UTF-8 in a
            // bounded record; JSON syntax errors are handled by the parser.
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "stream did not contain valid UTF-8",
                )
            })
    }
}

// Retain geometric growth without allocating beyond the record ceiling.
// On overflow discard the entire prefix, never a suffix that could become a call.
fn extend_bounded(bytes: &mut Vec<u8>, chunk: &[u8], ceiling: usize) -> bool {
    if chunk.len() > ceiling - bytes.len() {
        bytes.clear();
        return false;
    }
    let needed = bytes.len() + chunk.len();
    if needed > bytes.capacity() {
        let capacity = bytes.capacity().saturating_mul(2).max(needed).min(ceiling);
        bytes.reserve_exact(capacity - bytes.len());
    }
    bytes.extend_from_slice(chunk);
    true
}

#[cfg(test)]
mod tests {
    use super::{Frame, Frames};
    use std::io;
    use tokio::io::{AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn boundaries_crlf_blank_lines_eof_and_split_utf8() -> io::Result<()> {
        for (capacity, maximum) in
            (1..=12).flat_map(|capacity| [0, 1, 3, 8].map(|maximum| (capacity, maximum)))
        {
            let input = b"12345678\n12345678\r\n\n\r\n\xc3\xa9\xc3\xa9\n123456789\n12345678\n\r";
            let mut frames = Frames::new(BufReader::with_capacity(capacity, &input[..]), maximum);
            for expected in [
                Frame::Line("12345678".into()),
                Frame::Line("12345678".into()),
                Frame::Line(String::new()),
                Frame::Line(String::new()),
                Frame::Line("éé".into()),
                Frame::TooLarge,
                Frame::Line("12345678".into()),
                Frame::Line("\r".into()), // Bare CR at EOF is data, not a CRLF delimiter.
            ] {
                let expected = match expected {
                    Frame::Line(line) if line.len() > maximum => Frame::TooLarge,
                    frame => frame,
                };
                assert_eq!(frames.next().await?, Some(expected));
                assert!(frames.bytes.capacity() <= maximum + 1);
            }
            assert_eq!(frames.next().await?, None);
        }
        let mut frames = Frames::new(BufReader::new(&b"\xff\n"[..]), 8);
        let error = frames.next().await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "stream did not contain valid UTF-8");
        Ok(())
    }

    #[tokio::test]
    async fn drains_large_records_once_and_never_dispatches_their_suffix() -> io::Result<()> {
        for newline in [false, true] {
            let mut input = vec![b'x'; 1_000_000];
            input.extend_from_slice(br#"{"op":"shutdown","id":"hidden"}"#);
            if newline {
                input.extend_from_slice(b"\nvalid\n");
            }
            let mut frames = Frames::new(BufReader::with_capacity(7, &input[..]), 32);
            assert_eq!(frames.next().await?, Some(Frame::TooLarge));
            assert!(frames.bytes.capacity() <= 33);
            if newline {
                assert_eq!(frames.next().await?, Some(Frame::Line("valid".into())));
            }
            assert_eq!(frames.next().await?, None);
        }
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_read_retains_partial_record_and_discard_state() -> io::Result<()> {
        let (mut writer, reader) = tokio::io::duplex(64);
        let mut frames = Frames::new(BufReader::with_capacity(2, reader), 8);
        writer.write_all(b"part").await?;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), frames.next())
                .await
                .is_err()
        );
        writer.write_all(b"ial\n").await?;
        assert_eq!(frames.next().await?, Some(Frame::Line("partial".into())));
        writer.write_all(b"0123456789abcdef").await?;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), frames.next())
                .await
                .is_err()
        );
        assert!(frames.oversized);
        assert!(frames.bytes.capacity() <= 9);
        writer.write_all(b"ignored\nnext\n").await?;
        assert_eq!(frames.next().await?, Some(Frame::TooLarge));
        assert_eq!(frames.next().await?, Some(Frame::Line("next".into())));
        Ok(())
    }
}
