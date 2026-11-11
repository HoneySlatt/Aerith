// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Serves one clip over HTTP on the loopback interface, so that a rival opens it as a stream, the
//! way it opens a Jellyfin server's or an addon's: with range requests, which players use to read
//! a file's index and to seek. Loopback has no network delay: the figure it gives is the stream
//! path's own cost, a lower bound of a stream on the local network.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;

/// The longest request head accepted, in bytes; a player's is a few hundred.
const LARGEST_HEAD: u64 = 16 << 10;

/// A server running on its own thread for the run's length; it stops with the process.
pub struct ClipServer {
    address: SocketAddr,
}

impl ClipServer {
    /// Starts serving `clip` on a free port of 127.0.0.1.
    pub fn start(clip: PathBuf) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let address = listener.local_addr()?;
        let length = clip.metadata()?.len();
        thread::spawn(move || {
            for connection in listener.incoming().flatten() {
                let clip = clip.clone();
                thread::spawn(move || {
                    // A player closes connections it no longer needs: that is not an error.
                    let _ = serve(connection, &clip, length);
                });
            }
        });
        Ok(Self { address })
    }

    /// The URL a player opens.
    pub fn url(&self) -> String {
        format!("http://{}/clip", self.address)
    }
}

/// Answers the requests of one connection, kept alive, until the player closes it.
fn serve(connection: TcpStream, clip: &PathBuf, length: u64) -> io::Result<()> {
    let mut reader = BufReader::new(connection.try_clone()?);
    let mut writer = connection;
    loop {
        let Some(request) = read_head(&mut reader)? else {
            return Ok(());
        };
        let mut lines = request.lines();
        let first = lines.next().unwrap_or_default();
        let mut parts = first.split_whitespace();
        let (method, path) = (
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default(),
        );
        if !matches!(method, "GET" | "HEAD") || path != "/clip" {
            writer.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")?;
            continue;
        }
        let range = lines
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.trim().eq_ignore_ascii_case("range"))
            .map(|(_, value)| parse_range(value.trim(), length));
        let (status, start, end) = match range {
            None => ("200 OK", 0, length),
            Some(Some((start, end))) => ("206 Partial Content", start, end),
            Some(None) => {
                write!(
                    writer,
                    "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{length}\r\nContent-Length: 0\r\n\r\n"
                )?;
                continue;
            }
        };
        write!(
            writer,
            "HTTP/1.1 {status}\r\nContent-Type: video/x-matroska\r\nAccept-Ranges: bytes\r\n\
             Content-Length: {}\r\n",
            end - start
        )?;
        if status.starts_with("206") {
            write!(
                writer,
                "Content-Range: bytes {start}-{}/{length}\r\n",
                end - 1
            )?;
        }
        writer.write_all(b"\r\n")?;
        if method == "GET" {
            let mut file = File::open(clip)?;
            file.seek(SeekFrom::Start(start))?;
            io::copy(&mut file.take(end - start), &mut writer)?;
        }
    }
}

/// Reads a request's head, up to its empty line; `None` when the player closed the connection.
fn read_head(reader: &mut BufReader<TcpStream>) -> io::Result<Option<String>> {
    let mut head = String::new();
    let mut limited = reader.take(LARGEST_HEAD);
    loop {
        let mut line = String::new();
        if limited.read_line(&mut line)? == 0 {
            return if head.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "request head cut or too long",
                ))
            };
        }
        if line == "\r\n" || line == "\n" {
            return Ok(Some(head));
        }
        head.push_str(&line);
    }
}

/// The bytes a `Range` header asks for, as a half-open interval of the file, or `None` when it
/// cannot be served. One range only, as players ask.
fn parse_range(value: &str, length: u64) -> Option<(u64, u64)> {
    let spec = value.strip_prefix("bytes=")?;
    let (first, last) = spec.split_once('-')?;
    let (start, end) = match (first.trim(), last.trim()) {
        ("", suffix) => {
            let suffix: u64 = suffix.parse().ok()?;
            (length.saturating_sub(suffix), length)
        }
        (start, "") => (start.parse().ok()?, length),
        (start, last) => (
            start.parse().ok()?,
            last.parse::<u64>().ok()?.saturating_add(1).min(length),
        ),
    };
    (start < end && start < length).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_read_as_players_send_them() {
        assert_eq!(parse_range("bytes=0-", 100), Some((0, 100)));
        assert_eq!(parse_range("bytes=10-19", 100), Some((10, 20)));
        assert_eq!(parse_range("bytes=90-200", 100), Some((90, 100)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 100)));
        assert_eq!(parse_range("bytes=100-", 100), None);
        assert_eq!(parse_range("bytes=20-10", 100), None);
        assert_eq!(parse_range("bytes=a-b", 100), None);
        assert_eq!(parse_range("items=0-1", 100), None);
        assert_eq!(parse_range("bytes=0-", 0), None);
    }

    #[test]
    fn the_server_answers_a_range_and_the_whole_file() {
        let clip = std::env::temp_dir().join(format!("aerith-bench-serve-{}", std::process::id()));
        std::fs::write(&clip, b"0123456789").unwrap();
        let server = ClipServer::start(clip.clone()).unwrap();
        let address = server
            .url()
            .trim_start_matches("http://")
            .trim_end_matches("/clip")
            .to_owned();
        let ask = |request: &str| {
            let mut stream = TcpStream::connect(&address).unwrap();
            stream.write_all(request.as_bytes()).unwrap();
            stream.shutdown(std::net::Shutdown::Write).unwrap();
            let mut answer = String::new();
            stream.read_to_string(&mut answer).unwrap();
            answer
        };
        let partial = ask("GET /clip HTTP/1.1\r\nHost: x\r\nRange: bytes=2-4\r\n\r\n");
        assert!(partial.starts_with("HTTP/1.1 206"), "{partial}");
        assert!(partial.contains("Content-Range: bytes 2-4/10"), "{partial}");
        assert!(partial.ends_with("\r\n\r\n234"), "{partial}");
        let whole = ask("GET /clip HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(
            whole.starts_with("HTTP/1.1 200") && whole.ends_with("0123456789"),
            "{whole}"
        );
        let head = ask("HEAD /clip HTTP/1.1\r\n\r\n");
        assert!(
            head.contains("Content-Length: 10") && head.ends_with("\r\n\r\n"),
            "{head}"
        );
        let missing = ask("GET /other HTTP/1.1\r\n\r\n");
        assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");
        let beyond = ask("GET /clip HTTP/1.1\r\nRange: bytes=50-\r\n\r\n");
        assert!(beyond.starts_with("HTTP/1.1 416"), "{beyond}");
        std::fs::remove_file(clip).unwrap();
    }

    #[test]
    fn a_request_head_beyond_the_limit_is_refused() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let client = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            let header = format!("GET /clip HTTP/1.1\r\nX-Long: {}\r\n", "y".repeat(32 << 10));
            let _ = stream.write_all(header.as_bytes());
        });
        let (connection, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(connection);
        let error = read_head(&mut reader).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        client.join().unwrap();
    }
}
