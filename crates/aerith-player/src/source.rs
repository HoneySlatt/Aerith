// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the player can open: a file of the machine, or a stream over HTTP with the headers its
//! server asks for. Both come from untrusted places (a file name, an addon's answer), so they are
//! checked and bounded here, before any engine sees them.

use std::fmt;
use std::path::{Path, PathBuf};

/// The longest stream URL accepted, in bytes. Real URLs, tokens included, stay under a few KiB.
pub const LONGEST_URL: usize = 16 << 10;
/// The most headers a stream may carry.
pub const MOST_HEADERS: usize = 32;
/// The longest header accepted, name and value together, in bytes.
pub const LONGEST_HEADER: usize = 8 << 10;

/// Something to play.
///
/// Its `Debug` output hides what may be secret: header values, and all of a stream's URL but its
/// scheme and host.
#[derive(Clone, PartialEq, Eq)]
pub struct Source(Kind);

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Kind {
    /// An absolute path, valid UTF-8.
    File(String),
    /// An `http` or `https` URL and its headers.
    Stream { url: String, headers: Vec<Header> },
}

impl Source {
    /// A file of this machine. A relative path is taken from the current folder. The path must be
    /// valid UTF-8: the engine's protocol carries text. Whether the file exists is the engine's to
    /// find, and it reports it ([`crate::Ending::Failed`]).
    pub fn file(path: impl AsRef<Path>) -> Result<Self, InvalidSource> {
        // An absolute path starts with `/`, so an engine never mistakes a file named `x://y` for a
        // URL of another protocol.
        let path: PathBuf = std::path::absolute(path.as_ref())
            .map_err(|error| InvalidSource(format!("the path cannot be made absolute: {error}")))?;
        let text = path
            .into_os_string()
            .into_string()
            .map_err(|_| InvalidSource("the path is not valid UTF-8".to_owned()))?;
        if text.contains('\0') {
            return Err(InvalidSource("the path holds a NUL byte".to_owned()));
        }
        Ok(Self(Kind::File(text)))
    }

    /// A stream over HTTP or HTTPS. `headers` go to the stream's server with each request; they
    /// never appear on a command line or in a log.
    ///
    /// Other protocols are refused: an engine knows many (mpv reads `av://`, `edl://`, playlists),
    /// and a URL from an addon must not reach the machine's devices or files through them.
    pub fn stream(url: impl Into<String>, headers: Vec<Header>) -> Result<Self, InvalidSource> {
        let url = url.into();
        if url.len() > LONGEST_URL {
            return Err(InvalidSource(format!(
                "the URL is longer than {LONGEST_URL} bytes"
            )));
        }
        let scheme = url.split_once("://").map(|(scheme, _)| scheme);
        if !scheme
            .is_some_and(|s| s.eq_ignore_ascii_case("http") || s.eq_ignore_ascii_case("https"))
        {
            return Err(InvalidSource(
                "only http:// and https:// streams are played".to_owned(),
            ));
        }
        if url.chars().any(|c| c.is_control() || c == ' ') {
            return Err(InvalidSource(
                "the URL holds a space or a control character".to_owned(),
            ));
        }
        if headers.len() > MOST_HEADERS {
            return Err(InvalidSource(format!(
                "a stream carries at most {MOST_HEADERS} headers"
            )));
        }
        Ok(Self(Kind::Stream { url, headers }))
    }

    #[cfg_attr(
        not(target_os = "linux"),
        allow(dead_code, reason = "read by the engine, which Windows gains at M1.6")
    )]
    pub(crate) fn kind(&self) -> &Kind {
        &self.0
    }
}

impl fmt::Debug for Source {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Kind::File(path) => formatter.debug_tuple("File").field(path).finish(),
            Kind::Stream { url, headers } => {
                // The scheme and the host only: a token may sit in the user's part, the path (a
                // debrid resolver's key), the query or the fragment.
                let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
                let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
                let host = authority
                    .rsplit_once('@')
                    .map_or(authority, |(_, host)| host);
                let shown = format!("{scheme}://{host}/…");
                formatter
                    .debug_struct("Stream")
                    .field("url", &shown)
                    .field("headers", headers)
                    .finish()
            }
        }
    }
}

/// An HTTP header a stream's server asks for, such as a token or a referrer.
///
/// Its `Debug` output shows the name, never the value.
#[derive(Clone, PartialEq, Eq)]
pub struct Header {
    name: String,
    value: String,
}

impl Header {
    /// A header, checked: the name is an HTTP token, the value holds no line break or other
    /// control character but tabs, so that it cannot add headers of its own.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Result<Self, InvalidSource> {
        let (name, value) = (name.into(), value.into());
        if name.len() + value.len() > LONGEST_HEADER {
            return Err(InvalidSource(format!(
                "a header is longer than {LONGEST_HEADER} bytes"
            )));
        }
        // RFC 9110's token characters.
        let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
        if name.is_empty() || !name.chars().all(token) {
            // The name is not repeated: a header pasted whole, value included, lands here.
            return Err(InvalidSource(
                "a header name is empty or holds a character HTTP does not allow".to_owned(),
            ));
        }
        if value.chars().any(|c| c.is_control() && c != '\t') {
            return Err(InvalidSource(format!(
                "the value of the header {name} holds a control character"
            )));
        }
        Ok(Self { name, value })
    }

    /// The header as one line, `Name: value`, as HTTP writes it.
    #[cfg_attr(
        not(target_os = "linux"),
        allow(dead_code, reason = "read by the engine, which Windows gains at M1.6")
    )]
    pub(crate) fn line(&self) -> String {
        format!("{}: {}", self.name, self.value)
    }
}

impl fmt::Debug for Header {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: …", self.name)
    }
}

/// Why a source or a header was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidSource(String);

impl fmt::Display for InvalidSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for InvalidSource {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_file_is_made_absolute() {
        let source = Source::file("clip.mkv").unwrap();
        let Kind::File(path) = source.kind() else {
            panic!("a file")
        };
        assert!(Path::new(path).is_absolute());
        assert!(path.ends_with("clip.mkv"));
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_is_refused() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(OsStr::from_bytes(b"/tmp/\xff.mkv"));
        assert!(Source::file(path).is_err());
    }

    #[test]
    fn only_http_and_https_streams_are_accepted() {
        assert!(Source::stream("https://example.org/a.mkv", vec![]).is_ok());
        assert!(Source::stream("HTTP://example.org/a.mkv", vec![]).is_ok());
        for url in [
            "file:///etc/passwd",
            "av://lavfi",
            "edl://a.mkv",
            "example.org/a.mkv",
            "",
        ] {
            assert!(Source::stream(url, vec![]).is_err(), "{url}");
        }
    }

    #[test]
    fn a_url_cannot_carry_a_line_break_and_is_bounded() {
        assert!(Source::stream("http://a/b\r\nX: y", vec![]).is_err());
        assert!(Source::stream("http://a/b c", vec![]).is_err());
        let long = format!("http://a/{}", "x".repeat(LONGEST_URL));
        assert!(Source::stream(long, vec![]).is_err());
    }

    #[test]
    fn a_header_cannot_add_another() {
        assert!(Header::new("Authorization", "Bearer abc, def").is_ok());
        assert!(Header::new("X-Token", "a\r\nX-Other: b").is_err());
        assert!(Header::new("X Token", "a").is_err());
        assert!(Header::new("", "a").is_err());
        assert!(Header::new("X-Token", "a\0").is_err());
        assert!(Header::new("X-Token", "x".repeat(LONGEST_HEADER)).is_err());
    }

    #[test]
    fn the_number_of_headers_is_bounded() {
        let header = Header::new("X-A", "b").unwrap();
        assert!(Source::stream("http://a/b", vec![header.clone(); MOST_HEADERS]).is_ok());
        assert!(Source::stream("http://a/b", vec![header; MOST_HEADERS + 1]).is_err());
    }

    #[test]
    fn debug_output_hides_secrets() {
        for url in [
            "https://host/video.mkv?api_key=secret1",
            "https://user:secret1@host/resolve/realdebrid/secret1/h/f.mkv#t=secret1",
            "https://host",
        ] {
            let source =
                Source::stream(url, vec![Header::new("Authorization", "secret2").unwrap()])
                    .unwrap();
            let shown = format!("{source:?}");
            assert!(!shown.contains("secret"), "{shown}");
            assert!(shown.contains("https://host/…"), "{shown}");
            assert!(shown.contains("Authorization"), "{shown}");
        }
    }

    #[test]
    fn a_refused_header_name_is_not_repeated() {
        let error = Header::new("Authorization: Bearer secret", "x").unwrap_err();
        assert!(!error.to_string().contains("secret"), "{error}");
    }
}
