//! The agent's stdio as lines of JSON-RPC: one message per line, in both directions. A line the agent sends
//! is read into a bounded buffer; one that grows past [`MAX_LINE_BYTES`] ends the connection with an error
//! instead of filling memory (the agent's output is untrusted).
//!
//! The lines read are handed to the SDK, which queues them without a bound until its handlers have dealt
//! with each. [`Unread`] is the bound: it counts the bytes handed over and not yet taken in, and a line
//! that would pass its limit ends the connection instead of growing that queue.

use futures::{Sink, Stream};
use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

/// The longest message accepted from the agent. A whole file can travel in one diff, so this is generous.
pub const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;
/// The most bytes of the agent's lines held between the pipe and the handlers: four of the longest lines.
pub const MAX_UNREAD_BYTES: usize = 4 * MAX_LINE_BYTES;

/// The agent's lines that were handed to the connection and that it has not taken in yet, in the order
/// they were read. The connection deals with them in that order, so when one of its handlers has dealt
/// with a line, that line and every line before it are no longer held anywhere.
#[derive(Debug)]
pub struct Unread {
    limit: usize,
    held: Mutex<Held>,
}

#[derive(Debug, Default)]
struct Held {
    /// A line did not fit: the connection is ending for that reason.
    is_overrun: bool,
    bytes: usize,
    /// Each line's size, and whether a handler will say when it has been dealt with.
    lines: VecDeque<(usize, bool)>,
}

impl Unread {
    /// A count that lets at most `limit` bytes wait.
    pub fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit,
            held: Mutex::new(Held::default()),
        })
    }

    /// Why the connection ended, when it ended because a line did not fit.
    pub fn overrun(&self) -> Option<String> {
        self.held().is_overrun.then(|| self.overrun_sentence())
    }

    fn overrun_sentence(&self) -> String {
        let limit = self.limit;
        format!("the agent sent more than {limit} bytes faster than they could be taken in")
    }

    /// How many bytes wait now.
    pub fn bytes(&self) -> usize {
        self.held().bytes
    }

    /// A line of `bytes` is handed to the connection; `is_awaited` says a handler will take it in. An
    /// error when it does not fit with what already waits.
    fn hand_over(&self, bytes: usize, is_awaited: bool) -> io::Result<()> {
        let mut held = self.held();
        if held.bytes + bytes > self.limit {
            held.is_overrun = true;
            return Err(io::Error::other(self.overrun_sentence()));
        }
        held.bytes += bytes;
        held.lines.push_back((bytes, is_awaited));
        Ok(())
    }

    /// A handler has dealt with a line: the oldest line that awaited one, and every line before it, are
    /// no longer held.
    pub fn take_in(&self) {
        let mut held = self.held();
        while let Some((bytes, is_awaited)) = held.lines.pop_front() {
            held.bytes -= bytes;
            if is_awaited {
                break;
            }
        }
    }

    /// The count, also after a panic elsewhere poisoned the lock: it holds plain numbers.
    fn held(&self) -> MutexGuard<'_, Held> {
        self.held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Calls [`Unread::take_in`] when it is dropped: held by a handler for as long as it deals with a line.
pub struct TakingIn(Arc<Unread>);

impl TakingIn {
    pub fn of(unread: &Arc<Unread>) -> Self {
        Self(unread.clone())
    }
}

impl Drop for TakingIn {
    fn drop(&mut self) {
        self.0.take_in();
    }
}

/// The lines read from `reader`, without their line endings, until end of file or an error.
pub fn incoming<R>(
    reader: R,
    max_line_bytes: usize,
) -> impl Stream<Item = io::Result<String>> + Send
where
    R: AsyncBufRead + Unpin + Send + 'static,
{
    checked(reader, max_line_bytes, |_| Ok(()))
}

/// The lines read from `reader` for a connection that holds at most what `unread` allows: each line is
/// counted as it is handed over (`is_awaited` tells whether a handler will take it in), and the line that
/// does not fit is an error that ends the stream.
pub fn incoming_counted<R>(
    reader: R,
    max_line_bytes: usize,
    unread: Arc<Unread>,
    is_awaited: fn(&str) -> bool,
) -> impl Stream<Item = io::Result<String>> + Send
where
    R: AsyncBufRead + Unpin + Send + 'static,
{
    checked(reader, max_line_bytes, move |line| {
        unread.hand_over(line.len(), is_awaited(line))
    })
}

/// The lines of `reader`, each passed to `check` before it is yielded; the first error ends the stream.
fn checked<R, C>(
    reader: R,
    max_line_bytes: usize,
    check: C,
) -> impl Stream<Item = io::Result<String>> + Send
where
    R: AsyncBufRead + Unpin + Send + 'static,
    C: FnMut(&str) -> io::Result<()> + Send + 'static,
{
    futures::stream::unfold(Some((reader, check)), move |state| async move {
        let (mut reader, mut check) = state?;
        let line = match read_line(&mut reader, max_line_bytes).await {
            Ok(Some(line)) => line,
            Ok(None) => return None,
            Err(error) => return Some((Err(error), None)),
        };
        match check(&line) {
            Ok(()) => Some((Ok(line), Some((reader, check)))),
            Err(error) => Some((Err(error), None)),
        }
    })
}

/// A sink that writes each line and its newline to `writer` and flushes it.
pub fn outgoing<W>(writer: W) -> impl Sink<String, Error = io::Error> + Send
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    futures::sink::unfold(writer, |mut writer, line: String| async move {
        writer.write_all(line.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        Ok(writer)
    })
}

/// Reads one line of at most `max` bytes; `None` at end of file.
async fn read_line<R>(reader: &mut R, max: usize) -> io::Result<Option<String>>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return finish(line, false);
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let taken = newline.map_or(available.len(), |at| at + 1);
        if line.len() + taken > max {
            return Err(too_long(max));
        }
        line.extend_from_slice(&available[..taken]);
        reader.consume(taken);
        if newline.is_some() {
            return finish(line, true);
        }
    }
}

/// The line as text; an unterminated empty read is the end of the stream.
fn finish(mut line: Vec<u8>, is_terminated: bool) -> io::Result<Option<String>> {
    if line.is_empty() && !is_terminated {
        return Ok(None);
    }
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    String::from_utf8(line).map(Some).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "the agent sent a line that is not UTF-8",
        )
    })
}

fn too_long(max: usize) -> io::Error {
    let message = format!("the agent sent a line longer than {max} bytes");
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{SinkExt, StreamExt};

    async fn read_all(bytes: &'static [u8], max: usize) -> Vec<io::Result<String>> {
        incoming(bytes, max).collect().await
    }

    #[tokio::test]
    async fn incoming_lines_split_on_newlines_without_their_endings() {
        let lines = read_all(b"{\"a\":1}\r\n{\"b\":2}\nlast", 64).await;
        let lines: Vec<String> = lines.into_iter().map(Result::unwrap).collect();
        assert_eq!(lines, ["{\"a\":1}", "{\"b\":2}", "last"]);
    }

    #[tokio::test]
    async fn incoming_empty_input_yields_nothing() {
        assert!(read_all(b"", 64).await.is_empty());
    }

    #[tokio::test]
    async fn incoming_line_over_the_limit_is_an_error_and_ends_the_stream() {
        let lines = read_all(b"short\nthis line is far too long\nnever read\n", 8).await;
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].as_deref().unwrap(), "short");
        assert_eq!(
            lines[1].as_ref().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    fn is_update(line: &str) -> bool {
        line.contains("update")
    }

    #[tokio::test]
    async fn incoming_counted_ends_with_an_error_when_nothing_takes_the_lines_in() {
        let unread = Unread::new(24);
        let bytes: &[u8] = b"update 1\nupdate 2\nupdate 3\nupdate 4\nupdate 5\n";
        let lines: Vec<_> = incoming_counted(bytes, 64, unread.clone(), is_update)
            .collect()
            .await;
        let read: Vec<&str> = lines.iter().flatten().map(String::as_str).collect();
        assert_eq!(read, ["update 1", "update 2", "update 3"], "24 bytes fit");
        assert_eq!(lines.len(), 4, "the line that does not fit ends the stream");
        let error = lines[3].as_ref().unwrap_err();
        assert!(error.to_string().contains("24 bytes"), "{error}");
        assert_eq!(unread.bytes(), 24);
        assert!(unread.overrun().is_some_and(|why| why.contains("24 bytes")));
    }

    #[tokio::test]
    async fn incoming_counted_goes_on_while_the_lines_are_taken_in() {
        let unread = Unread::new(24);
        let bytes: &[u8] = b"update 1\nupdate 2\nupdate 3\nupdate 4\nupdate 5\n";
        let mut lines = Box::pin(incoming_counted(bytes, 64, unread.clone(), is_update));
        let mut read = 0;
        while let Some(line) = lines.next().await {
            line.expect("every line fits once the one before it was taken in");
            read += 1;
            drop(TakingIn::of(&unread));
        }
        assert_eq!(read, 5);
        assert_eq!(unread.bytes(), 0);
        assert_eq!(unread.overrun(), None);
    }

    #[test]
    fn a_line_taken_in_frees_its_room_and_that_of_the_lines_before_it() {
        let unread = Unread::new(100);
        // A response nobody will report, an update, a response, an update.
        for (bytes, is_awaited) in [(10, false), (20, true), (30, false), (5, true)] {
            unread.hand_over(bytes, is_awaited).unwrap();
        }
        assert_eq!(unread.bytes(), 65);
        unread.take_in();
        assert_eq!(
            unread.bytes(),
            35,
            "the first update and the response before it"
        );
        unread.take_in();
        assert_eq!(unread.bytes(), 0);
        unread.take_in();
        assert_eq!(unread.bytes(), 0, "nothing waits: nothing to free");
    }

    #[tokio::test]
    async fn incoming_invalid_utf8_is_an_error() {
        let lines = read_all(b"\xff\xfe\n", 64).await;
        assert_eq!(
            lines[0].as_ref().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[tokio::test]
    async fn outgoing_writes_each_line_with_a_newline() {
        let (writer, mut reader) = tokio::io::duplex(256);
        let mut sink = Box::pin(outgoing(writer));
        sink.send("one".to_string()).await.unwrap();
        sink.send("two".to_string()).await.unwrap();
        drop(sink);
        let mut written = String::new();
        tokio::io::AsyncReadExt::read_to_string(&mut reader, &mut written)
            .await
            .unwrap();
        assert_eq!(written, "one\ntwo\n");
    }
}
