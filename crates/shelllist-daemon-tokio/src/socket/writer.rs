//! Bound complete records, not individual OS writes. A partial write never resets
//! the deadline; on timeout the connection is retired, not resumed mid-record.
use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::AsyncWrite;

pub(super) struct DeadlineWriter<W> {
    writer: W,
    limit: usize,
    written: usize,
    timeout: Duration,
    deadline: Option<Pin<Box<tokio::time::Sleep>>>,
}
impl<W> DeadlineWriter<W> {
    pub fn new(writer: W, limit: usize, timeout: Duration) -> Self {
        Self {
            writer,
            limit,
            written: 0,
            timeout,
            deadline: None,
        }
    }
    fn expired(&mut self, cx: &mut Context<'_>) -> bool {
        self.deadline
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(self.timeout)))
            .as_mut()
            .poll(cx)
            .is_ready()
    }
}
impl<W: AsyncWrite + Unpin> AsyncWrite for DeadlineWriter<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if bytes.len() > self.limit.saturating_sub(self.written) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "JSONL response exceeds frame limit",
            )));
        }
        if self.expired(cx) {
            return Poll::Ready(Err(io::ErrorKind::TimedOut.into()));
        }
        match Pin::new(&mut self.writer).poll_write(cx, bytes) {
            Poll::Ready(Ok(n)) => {
                self.written += n;
                Poll::Ready(Ok(n))
            }
            result => result,
        }
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.expired(cx) {
            return Poll::Ready(Err(io::ErrorKind::TimedOut.into()));
        }
        match Pin::new(&mut self.writer).poll_flush(cx) {
            Poll::Ready(Ok(())) => {
                self.written = 0;
                self.deadline = None;
                Poll::Ready(Ok(()))
            }
            result => result,
        }
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.writer).poll_shutdown(cx)
    }
}
