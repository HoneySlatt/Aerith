// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A request and response connection that carries JSON values, as mpv's IPC and Kodi's JSON-RPC
//! do. Requests carry an id; the answer with the same id is returned, and every other value that
//! arrives in between (mpv's events, Kodi's notifications) is kept, in order, for the caller.
//!
//! mpv ends each value with a new line and Kodi does not: the stream is read value by value, so
//! both work.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

use serde_json::Value;

/// The largest value accepted from a player, in bytes. A player's answers are small; a value
/// larger than this is a fault, not something to buffer.
const LARGEST_VALUE: usize = 4 << 20;

/// How each side names the id of a request.
#[derive(Clone, Copy, Debug)]
pub enum Protocol {
    /// mpv's IPC: `{"command": [...], "request_id": n}`.
    Mpv,
    /// JSON-RPC 2.0, as Kodi speaks it: `{"jsonrpc": "2.0", "method": ..., "id": n}`.
    JsonRpc,
}

impl Protocol {
    fn id_key(self) -> &'static str {
        match self {
            Protocol::Mpv => "request_id",
            Protocol::JsonRpc => "id",
        }
    }
}

/// A transport the connection can bound in time: a Unix or TCP socket.
pub trait Timed: Read + Write {
    /// Sets how long one read may wait; `None` waits for ever.
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
}

#[cfg(unix)]
impl Timed for std::os::unix::net::UnixStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        std::os::unix::net::UnixStream::set_read_timeout(self, timeout)
    }
}

impl Timed for std::net::TcpStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        std::net::TcpStream::set_read_timeout(self, timeout)
    }
}

/// One connection to a player.
pub struct JsonStream<S: Timed> {
    writer: S,
    reader: S,
    protocol: Protocol,
    next_id: u64,
    /// Values that are not answers, oldest first.
    unsolicited: VecDeque<Value>,
    /// Bytes read but not yet returned as a value.
    pending: Vec<u8>,
    framer: Framer,
}

impl<S: Timed> JsonStream<S> {
    /// Wraps a connected transport; `reader` is a second handle on the same socket.
    pub fn new(writer: S, reader: S, protocol: Protocol) -> Self {
        Self {
            writer,
            reader,
            protocol,
            next_id: 1,
            unsolicited: VecDeque::new(),
            pending: Vec::new(),
            framer: Framer::default(),
        }
    }

    /// Sends `request` with a fresh id and returns the answer with that id, waiting at most
    /// `timeout`. Values that arrive in the meantime are kept for [`Self::next_unsolicited`].
    pub fn request(&mut self, mut request: Value, timeout: Duration) -> io::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let key = self.protocol.id_key();
        request[key] = Value::from(id);
        let mut line = serde_json::to_vec(&request).map_err(io::Error::other)?;
        line.push(b'\n');
        self.writer.write_all(&line)?;
        self.writer.flush()?;
        let deadline = Instant::now() + timeout;
        loop {
            let value = self.read_value(deadline)?;
            if value.get(key).and_then(Value::as_u64) == Some(id) {
                return Ok(value);
            }
            self.unsolicited.push_back(value);
        }
    }

    /// The oldest value that was not an answer, waiting at most until `deadline` for one.
    /// `Ok(None)` when none came in time.
    pub fn next_unsolicited(&mut self, deadline: Instant) -> io::Result<Option<Value>> {
        if let Some(value) = self.unsolicited.pop_front() {
            return Ok(Some(value));
        }
        match self.read_value(deadline) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.kind() == io::ErrorKind::TimedOut => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Reads one value, or fails with `TimedOut` once `deadline` has passed. The bytes of a value
    /// cut by a timeout are kept, so the next call goes on where this one stopped.
    fn read_value(&mut self, deadline: Instant) -> io::Result<Value> {
        loop {
            if let Some(end) = self.framer.complete_value(&self.pending) {
                let value = serde_json::from_slice(&self.pending[..end])
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
                self.pending.drain(..end);
                self.framer = Framer::default();
                return value;
            }
            if self.pending.len() > LARGEST_VALUE {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "value too large",
                ));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            self.reader.set_read_timeout(Some(remaining))?;
            let mut chunk = [0; 4096];
            match self.reader.read(&mut chunk) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(read) => self.pending.extend_from_slice(&chunk[..read]),
                // A socket's read timeout shows as WouldBlock on Unix, TimedOut on Windows.
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
}

/// Finds where the first top-level JSON value of a byte stream ends, so that values that follow
/// one another with or without a new line can be told apart. It tracks brackets outside strings
/// only; whether the value is valid JSON is serde_json's to say. A top-level value that is not an
/// object or an array (neither player sends one) is taken to end at the next white space.
#[derive(Default)]
struct Framer {
    /// How far `complete_value` has scanned.
    scanned: usize,
    depth: usize,
    started: bool,
    in_string: bool,
    escaped: bool,
}

impl Framer {
    /// The length of the first complete value in `bytes`, white space before it included.
    fn complete_value(&mut self, bytes: &[u8]) -> Option<usize> {
        while self.scanned < bytes.len() {
            let byte = bytes[self.scanned];
            self.scanned += 1;
            if self.in_string {
                if self.escaped {
                    self.escaped = false;
                } else if byte == b'\\' {
                    self.escaped = true;
                } else if byte == b'"' {
                    self.in_string = false;
                }
                continue;
            }
            match byte {
                b' ' | b'\n' | b'\r' | b'\t' if !self.started => {}
                b' ' | b'\n' | b'\r' | b'\t' if self.depth == 0 => return Some(self.scanned - 1),
                b'{' | b'[' => {
                    self.started = true;
                    self.depth += 1;
                }
                b'}' | b']' => {
                    self.started = true;
                    self.depth = self.depth.saturating_sub(1);
                    if self.depth == 0 {
                        return Some(self.scanned);
                    }
                }
                b'"' => {
                    self.started = true;
                    self.in_string = true;
                }
                _ => self.started = true,
            }
        }
        None
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::BufRead;
    use std::os::unix::net::UnixStream;
    use std::thread;

    fn connected(protocol: Protocol) -> (JsonStream<UnixStream>, UnixStream) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let reader = ours.try_clone().unwrap();
        (JsonStream::new(ours, reader, protocol), theirs)
    }

    #[test]
    fn an_answer_is_matched_by_its_id_and_events_are_kept_in_order() {
        let (mut stream, player) = connected(Protocol::Mpv);
        let server = thread::spawn(move || {
            let mut lines = io::BufReader::new(player.try_clone().unwrap()).lines();
            let request: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
            assert_eq!(request["command"], json!(["get_property", "avsync"]));
            let id = request["request_id"].clone();
            let mut player = player;
            // An event and an answer to another id come first, without a new line between them.
            writeln!(
                player,
                "{{\"event\":\"tick\"}}{{\"request_id\":99,\"data\":1}}"
            )
            .unwrap();
            writeln!(
                player,
                "{}",
                json!({"request_id": id, "data": 0.004, "error": "success"})
            )
            .unwrap();
        });
        let answer = stream
            .request(
                json!({"command": ["get_property", "avsync"]}),
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(answer["data"], json!(0.004));
        let deadline = Instant::now() + Duration::from_secs(1);
        assert_eq!(
            stream.next_unsolicited(deadline).unwrap(),
            Some(json!({"event": "tick"}))
        );
        assert_eq!(
            stream.next_unsolicited(deadline).unwrap(),
            Some(json!({"request_id": 99, "data": 1}))
        );
        server.join().unwrap();
    }

    #[test]
    fn json_rpc_uses_id() {
        let (mut stream, player) = connected(Protocol::JsonRpc);
        let server = thread::spawn(move || {
            let mut lines = io::BufReader::new(player.try_clone().unwrap()).lines();
            let request: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
            let mut player = player;
            write!(
                player,
                "{}",
                json!({"jsonrpc": "2.0", "id": request["id"], "result": "pong"})
            )
            .unwrap();
        });
        let answer = stream
            .request(
                json!({"jsonrpc": "2.0", "method": "JSONRPC.Ping"}),
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(answer["result"], json!("pong"));
        server.join().unwrap();
    }

    #[test]
    fn a_silent_player_times_out() {
        let (mut stream, _player) = connected(Protocol::Mpv);
        let error = stream
            .request(
                json!({"command": ["get_property", "pause"]}),
                Duration::from_millis(50),
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        let deadline = Instant::now() + Duration::from_millis(20);
        assert!(stream.next_unsolicited(deadline).unwrap().is_none());
    }

    #[test]
    fn a_closed_connection_is_an_error_not_a_hang() {
        let (mut stream, player) = connected(Protocol::Mpv);
        drop(player);
        let error = stream
            .request(
                json!({"command": ["get_property", "pause"]}),
                Duration::from_secs(5),
            )
            .unwrap_err();
        // A reset, too, when the request was written before the end showed: a process another
        // test forks holds the other end for an instant, and the end then drops what it did not
        // read.
        assert!(matches!(
            error.kind(),
            io::ErrorKind::UnexpectedEof
                | io::ErrorKind::BrokenPipe
                | io::ErrorKind::ConnectionReset
        ));
    }

    #[test]
    fn a_value_cut_by_a_timeout_is_finished_by_the_next_read() {
        let (mut stream, player) = connected(Protocol::Mpv);
        let mut player = player;
        player.write_all(b"{\"event\":\"end-").unwrap();
        let soon = Instant::now() + Duration::from_millis(30);
        assert!(stream.next_unsolicited(soon).unwrap().is_none());
        player
            .write_all(b"file\", \"text\": \"a } in a string\"}\n")
            .unwrap();
        let later = Instant::now() + Duration::from_secs(1);
        assert_eq!(
            stream.next_unsolicited(later).unwrap(),
            Some(json!({"event": "end-file", "text": "a } in a string"}))
        );
    }

    #[test]
    fn malformed_input_is_an_error() {
        let (mut stream, player) = connected(Protocol::Mpv);
        let mut player = player;
        player.write_all(b"{\"event\": nope}\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let error = stream.next_unsolicited(deadline).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_value_larger_than_the_limit_is_refused() {
        let (mut stream, player) = connected(Protocol::Mpv);
        let writer = thread::spawn(move || {
            let mut player = player;
            // An object that never closes, beyond the limit: the bench must give up, not grow.
            let _ = player.write_all(b"{\"data\": \"");
            let chunk = vec![b'x'; 1 << 16];
            for _ in 0..(LARGEST_VALUE / chunk.len() + 2) {
                if player.write_all(&chunk).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let error = stream.next_unsolicited(deadline).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        drop(stream);
        writer.join().unwrap();
    }
}
