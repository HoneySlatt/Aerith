// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! mpv's socket, which the relay reads itself.
//!
//! wayland-server, given a client's socket, reads it until it would block, answers `wl_display`'s
//! requests (`sync`, `get_registry`) without calling the relay, and keeps the file descriptors
//! that come with a request that takes none in a queue of its own. A client that keeps writing
//! would keep it reading, holding its lock, and could fill Aerith's file table, unseen by the
//! relay's handlers. So the relay does not hand it mpv's socket. It reads mpv's socket itself,
//! counts what arrives there (requests, registries, file descriptors) against ceilings, and passes
//! it on to the server through a socket pair of its own; what the server sends back is passed on
//! to mpv the same way. The bytes are passed as they are: the server parses them.
//!
//! Each wake-up of the relay thus handles a bounded part of what mpv sent, whichever object mpv
//! sent it to. What the ceilings are, and why, is with each one.
//!
//! One request is also checked here, as the server cannot take it: `wl_registry.bind` without an
//! interface's name, at which wayland-backend 0.3.17 panics (`handle_registry_request`'s
//! `unreachable!()`, there still upstream), which would end the relay's thread, or the interface's
//! process where it is built to abort on a panic.

use std::collections::HashSet;
use std::io::{IoSlice, IoSliceMut};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;

use rustix::io::Errno;
use rustix::net::{
    RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags,
};

/// The most requests mpv may send in one wake-up of the relay, whatever their object. mpv sends a
/// few dozen a frame; the ceiling bounds what one wake-up handles, and so how long the relay holds
/// its locks.
pub(super) const MOST_REQUESTS_AT_ONCE: usize = 4096;

/// The most file descriptors mpv may send in one wake-up. A swapchain of four dma-bufs of four
/// planes each takes sixteen.
pub(super) const MOST_FDS_AT_ONCE: usize = 128;

/// The most file descriptors one read of mpv's socket may carry: what libwayland sends with one
/// message at most (`MAX_FDS_OUT`), and what wayland-server takes with one read.
const MOST_FDS_A_READ: usize = 28;

/// The most registries mpv may make. The protocol cannot destroy one: each stays, and takes about
/// 120 bytes of the relay's (106 measured in the server, and its id here), until mpv's connection
/// ends. Mesa makes one for each query of a surface's formats, capabilities or present modes, at
/// each swapchain mpv makes, so a long session makes thousands; far fewer than this ceiling, which
/// bounds them to about 4 MB.
pub(super) const MOST_REGISTRIES: usize = 32_768;

/// How much is read from a socket at once, and passed on in one message.
const CHUNK: usize = 4096;

/// What mpv sent in the current wake-up, and in all, as counted at its socket.
#[derive(Default)]
pub(super) struct Counts {
    /// Requests and file descriptors in this wake-up.
    requests: usize,
    fds: usize,
    /// File descriptors since mpv connected: the relay's handlers count those they took, and the
    /// difference is what the server keeps queued ([`super::MOST_QUEUED_FDS`]).
    pub(super) fds_received: usize,
}

/// mpv's socket and the relay's socket pair to its Wayland server, and what waits to be passed
/// between them.
pub(super) struct MpvSocket {
    /// The relay's end of mpv's socket; `None` once mpv hung up or the relay closed it.
    mpv: Option<UnixStream>,
    /// The relay's end of the pair whose other end the server reads; `None` once the server
    /// closed its end, after mpv's connection ended.
    server: Option<UnixStream>,
    /// What mpv sent that the server's socket could not take yet.
    to_server: Pending,
    /// What the server sent that mpv's socket could not take yet.
    to_mpv: Pending,
    /// Where the reading stands in mpv's stream of messages.
    framing: Framing,
    /// The ids of mpv's registries, which the protocol never destroys.
    registries: HashSet<u32>,
    pub(super) counts: Counts,
}

/// Bytes to pass on, and the file descriptors to send with their first part.
#[derive(Default)]
struct Pending {
    bytes: Vec<u8>,
    fds: Vec<OwnedFd>,
}

/// mpv's stream cut into messages, by their heads: the header (an object's id, and a word holding
/// the message's size and opcode), then the first two words of the arguments. Only the heads are
/// read; the server parses the rest.
#[derive(Default)]
struct Framing {
    /// The bytes of the current message's head read so far.
    head: Vec<u8>,
    /// What remains of the current message after its head.
    remaining: usize,
}

/// The bytes of a message's head: its header and two words.
const HEAD: usize = 16;

/// What reading mpv's socket came to.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Read {
    /// mpv's socket held nothing.
    Nothing,
    /// What mpv sent reached the server's socket, and mpv's socket is empty for now.
    Passed,
    /// The server's socket is full: the server is to read before more is passed.
    ServerFull,
    /// mpv closed its end; the server is told.
    HungUp,
    /// mpv broke a ceiling, or sent what the server cannot take, for this reason. What it sent
    /// from there on is not passed on.
    Refused(String),
}

impl MpvSocket {
    /// Takes mpv's socket, and returns the end of the relay's pair that the server is to read.
    pub(super) fn new(mpv: UnixStream) -> std::io::Result<(Self, UnixStream)> {
        let (relay_end, server_end) = UnixStream::pair()?;
        mpv.set_nonblocking(true)?;
        relay_end.set_nonblocking(true)?;
        let socket = Self {
            mpv: Some(mpv),
            server: Some(relay_end),
            to_server: Pending::default(),
            to_mpv: Pending::default(),
            framing: Framing::default(),
            registries: HashSet::new(),
            counts: Counts::default(),
        };
        Ok((socket, server_end))
    }

    /// mpv's socket, while the relay reads it.
    pub(super) fn mpv(&self) -> Option<BorrowedFd<'_>> {
        self.mpv.as_ref().map(AsFd::as_fd)
    }

    /// The relay's end of its pair with the server, while open.
    pub(super) fn server(&self) -> Option<BorrowedFd<'_>> {
        self.server.as_ref().map(AsFd::as_fd)
    }

    /// Whether what mpv sent waits for the server's socket: mpv's socket is not read meanwhile.
    pub(super) fn waits_for_server(&self) -> bool {
        !self.to_server.bytes.is_empty()
    }

    /// Whether what the server sent waits for mpv's socket: the server's is not read meanwhile.
    pub(super) fn waits_for_mpv(&self) -> bool {
        !self.to_mpv.bytes.is_empty()
    }

    /// Starts a wake-up: its counts start from zero.
    pub(super) fn start_wake_up(&mut self) {
        self.counts.requests = 0;
        self.counts.fds = 0;
    }

    /// Reads what mpv sent, counts it, and passes it on to the server, until mpv's socket is
    /// empty, the server's is full, or mpv breaks a ceiling.
    pub(super) fn read_mpv(&mut self) -> Read {
        let mut passed = false;
        loop {
            if self.waits_for_server() {
                match pass(self.server.as_ref(), &mut self.to_server) {
                    Ok(true) => passed = true,
                    Ok(false) => return Read::ServerFull,
                    // The server's end is closed: mpv's connection ended there.
                    Err(()) => return Read::Nothing,
                }
            }
            let Some(mpv) = &self.mpv else {
                return Read::Nothing;
            };
            let mut bytes = [0; CHUNK];
            let (length, fds) = match receive(mpv, &mut bytes) {
                Ok(Received::Data(length, fds)) => (length, fds),
                Ok(Received::End) => {
                    self.mpv_hung_up();
                    return Read::HungUp;
                }
                Ok(Received::TooManyFds) => {
                    return Read::Refused(format!(
                        "mpv sent more than {MOST_FDS_A_READ} files with one message"
                    ));
                }
                Err(Errno::WOULDBLOCK) if passed => return Read::Passed,
                Err(Errno::WOULDBLOCK) => return Read::Nothing,
                // A broken socket is mpv's connection ending.
                Err(_) => {
                    self.mpv_hung_up();
                    return Read::HungUp;
                }
            };
            let bytes = &bytes[..length];
            if let Err(reason) = self.count(bytes, fds.len()) {
                return Read::Refused(reason);
            }
            self.to_server = Pending {
                bytes: bytes.to_vec(),
                fds,
            };
        }
    }

    /// Counts a read of mpv's (its messages, its registries, its file descriptors), and checks
    /// the binds of its registries.
    fn count(&mut self, bytes: &[u8], fds: usize) -> Result<(), String> {
        let counts = &mut self.counts;
        counts.fds += fds;
        counts.fds_received += fds;
        if counts.fds > MOST_FDS_AT_ONCE {
            return Err(format!(
                "mpv sent more than {MOST_FDS_AT_ONCE} files at once"
            ));
        }
        for head in self.framing.heads(bytes) {
            counts.requests += 1;
            // wl_display, always object 1: its request 1 is get_registry(new_id).
            if head.object == 1 && head.opcode == 1 {
                // A message too short for its id is malformed, and ends mpv at the server.
                if let Some(id) = head.args[0] {
                    self.registries.insert(id);
                }
            } else if head.opcode == 0 && self.registries.contains(&head.object) {
                // wl_registry.bind(name, interface, version, new_id): the interface's length,
                // its NUL counted, is 0 for none.
                if head.args[1] == Some(0) {
                    return Err("mpv bound a global without naming its interface".to_owned());
                }
            }
        }
        if counts.requests > MOST_REQUESTS_AT_ONCE {
            return Err(format!(
                "mpv sent more than {MOST_REQUESTS_AT_ONCE} requests at once"
            ));
        }
        if self.registries.len() > MOST_REGISTRIES {
            return Err(format!("mpv made more than {MOST_REGISTRIES} registries"));
        }
        Ok(())
    }

    /// mpv closed its end: the server sees its end of the pair close too, once it read the rest.
    fn mpv_hung_up(&mut self) {
        self.mpv = None;
        self.to_server = Pending::default();
        if let Some(server) = &self.server {
            let _ = server.shutdown(std::net::Shutdown::Write);
        }
    }

    /// Passes on to mpv what the server sent, until the server's socket is empty or mpv's full.
    /// Once the server closed its end, after mpv's connection ended there, mpv's socket is
    /// closed too, so that mpv sees the end.
    pub(super) fn write_mpv(&mut self) {
        loop {
            if self.waits_for_mpv() {
                match pass(self.mpv.as_ref(), &mut self.to_mpv) {
                    Ok(true) => {}
                    Ok(false) => return,
                    // mpv is gone: what the server sends it is dropped below.
                    Err(()) => self.to_mpv = Pending::default(),
                }
            }
            let Some(server) = &self.server else {
                return;
            };
            let mut bytes = [0; CHUNK];
            match receive(server, &mut bytes) {
                Ok(Received::Data(length, fds)) => {
                    self.to_mpv = Pending {
                        bytes: bytes[..length].to_vec(),
                        fds,
                    };
                }
                Err(Errno::WOULDBLOCK) => return,
                // The server never sends more files with a message than it reads; anything else
                // is its end closing.
                Ok(Received::End | Received::TooManyFds) | Err(_) => {
                    self.close();
                    return;
                }
            }
        }
    }

    /// Closes both of the relay's ends: mpv sees its connection end.
    pub(super) fn close(&mut self) {
        self.mpv = None;
        self.server = None;
        self.to_server = Pending::default();
        self.to_mpv = Pending::default();
    }
}

/// One message's head.
struct Head {
    object: u32,
    opcode: u16,
    /// The first two words of its arguments, those it has.
    args: [Option<u32>; 2],
}

impl Framing {
    /// The heads of the messages whose heads end in `bytes`, the next part of mpv's stream.
    fn heads(&mut self, mut bytes: &[u8]) -> Vec<Head> {
        let mut heads = Vec::new();
        while !bytes.is_empty() {
            if self.remaining > 0 {
                let skipped = self.remaining.min(bytes.len());
                self.remaining -= skipped;
                bytes = &bytes[skipped..];
                continue;
            }
            // The header first, then as much of the head as the message holds.
            let wanted = if self.head.len() < 8 {
                8
            } else {
                self.size().min(HEAD)
            };
            let taken = (wanted - self.head.len()).min(bytes.len());
            self.head.extend_from_slice(&bytes[..taken]);
            bytes = &bytes[taken..];
            if self.head.len() < wanted {
                break;
            }
            if self.head.len() == 8 && self.size() > 8 {
                continue;
            }
            let head = Head {
                object: self.word(0).unwrap_or_default(),
                // The low half of the second word.
                opcode: (self.word(4).unwrap_or_default() & 0xffff) as u16,
                args: [self.word(8), self.word(12)],
            };
            self.remaining = self.size() - self.head.len();
            self.head.clear();
            heads.push(head);
        }
        heads
    }

    /// The current message's size, from its header, which counts itself. A size below the
    /// header's own is malformed, and the server ends mpv's connection when it reads it; the
    /// message is taken as a header alone meanwhile.
    fn size(&self) -> usize {
        let size = self.word(4).map_or(8, |word| word >> 16);
        usize::try_from(size).unwrap_or(usize::MAX).max(8)
    }

    /// The word at byte `at` of the head, if read.
    fn word(&self, at: usize) -> Option<u32> {
        let bytes = self.head.get(at..at + 4)?;
        Some(u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

/// What a read of a socket brought.
enum Received {
    /// Bytes, and the file descriptors sent with them.
    Data(usize, Vec<OwnedFd>),
    /// The other end closed.
    End,
    /// More file descriptors than one read takes: the kernel closed those that did not fit.
    TooManyFds,
}

/// Reads once from `socket` into `bytes`, without waiting.
fn receive(socket: &UnixStream, bytes: &mut [u8]) -> rustix::io::Result<Received> {
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MOST_FDS_A_READ))];
    let mut control = RecvAncillaryBuffer::new(&mut space);
    let mut iov = [IoSliceMut::new(bytes)];
    let read = loop {
        match rustix::net::recvmsg(
            socket,
            &mut iov,
            &mut control,
            RecvFlags::DONTWAIT | RecvFlags::CMSG_CLOEXEC,
        ) {
            Err(Errno::INTR) => {}
            other => break other?,
        }
    };
    let fds: Vec<OwnedFd> = control
        .drain()
        .filter_map(|message| match message {
            RecvAncillaryMessage::ScmRights(fds) => Some(fds),
            _ => None,
        })
        .flatten()
        .collect();
    if read.flags.contains(ReturnFlags::CTRUNC) {
        return Ok(Received::TooManyFds);
    }
    if read.bytes == 0 && fds.is_empty() {
        return Ok(Received::End);
    }
    Ok(Received::Data(read.bytes, fds))
}

/// Sends what `pending` holds on `socket`, its file descriptors with the first bytes, without
/// waiting. Returns whether all of it went; `Err` if the socket is closed or broken.
fn pass(socket: Option<&UnixStream>, pending: &mut Pending) -> Result<bool, ()> {
    let Some(socket) = socket else {
        return Err(());
    };
    while !pending.bytes.is_empty() {
        match send(socket, pending) {
            Ok(count) => {
                // The kernel sends the file descriptors with the first bytes it takes; ours are
                // closed, the receiver holds its own.
                pending.fds.clear();
                pending.bytes.drain(..count);
            }
            Err(Errno::INTR) => {}
            Err(Errno::WOULDBLOCK) => return Ok(false),
            Err(_) => return Err(()),
        }
    }
    Ok(true)
}

/// Sends once, without waiting, as much of `pending`'s bytes as the socket takes, with its file
/// descriptors. Returns how many bytes went.
fn send(socket: &UnixStream, pending: &Pending) -> rustix::io::Result<usize> {
    let fds: Vec<BorrowedFd<'_>> = pending.fds.iter().map(AsFd::as_fd).collect();
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MOST_FDS_A_READ))];
    let mut control = SendAncillaryBuffer::new(&mut space);
    // `pending` holds at most what one read takes, which the space was made for.
    if !fds.is_empty() && !control.push(SendAncillaryMessage::ScmRights(&fds)) {
        return Err(Errno::NOBUFS);
    }
    rustix::net::sendmsg(
        socket,
        &[IoSlice::new(&pending.bytes)],
        &mut control,
        SendFlags::DONTWAIT | SendFlags::NOSIGNAL,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(object: u32, opcode: u16, size: u16) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(object.to_ne_bytes());
        bytes.extend(((u32::from(size) << 16) | u32::from(opcode)).to_ne_bytes());
        bytes.resize(usize::from(size).max(8), 0);
        bytes
    }

    #[test]
    fn messages_are_read_by_their_heads_across_reads() {
        let mut stream = Vec::new();
        stream.extend(message(1, 1, 12));
        stream.extend(message(3, 6, 24));
        stream.extend(message(1, 0, 8));
        // The words after each header, as `message` writes them: zeroes.
        let expected = [
            (1, 1, [Some(0), None]),
            (3, 6, [Some(0), Some(0)]),
            (1, 0, [None, None]),
        ];
        // Cut at every place: a header, a head or a body split between two reads.
        for cut in 0..stream.len() {
            let mut framing = Framing::default();
            let (first, second) = stream.split_at(cut);
            let mut found: Vec<(u32, u16, [Option<u32>; 2])> = framing
                .heads(first)
                .into_iter()
                .map(|h| (h.object, h.opcode, h.args))
                .collect();
            found.extend(
                framing
                    .heads(second)
                    .into_iter()
                    .map(|h| (h.object, h.opcode, h.args)),
            );
            assert_eq!(found, expected, "cut at {cut}");
        }
        // A size below the header's own is one header.
        let mut framing = Framing::default();
        assert_eq!(framing.heads(&message(2, 0, 0)).len(), 1);
        assert_eq!(framing.heads(&message(2, 0, 0)).len(), 1);
    }
}
