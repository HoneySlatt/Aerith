// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The relay's server side against a fake mpv that sends what the fuzzer gives: Wayland messages
//! to any object, with any arguments, some with a file descriptor, between changes of the
//! interface's (place, show, hide, commit).
//!
//! Each input runs a whole session: the fake compositor of the player's tests
//! (`tests/common/host.rs`) and an interface connected to it, the relay on the interface's window,
//! and mpv's end of the relay's socket. A prelude first makes what mpv makes (every global bound,
//! a toplevel configured, a frame on the plane, a viewport), so that the input starts from mpv's
//! usual state; the input's messages follow.
//!
//! What must hold, whatever the input: nothing panics, in any thread (libFuzzer aborts the
//! process on a panic); the relay reads mpv's socket to its end and closes it, or closes it
//! before; and the interface's connection lives on, the fake compositor having raised no error on
//! it, during the session and after the plane is closed.

#![no_main]

#[allow(
    dead_code,
    reason = "the fake compositor is shared with the player's tests, which use more of it"
)]
#[path = "../../tests/common/host.rs"]
mod host;

use std::io::{IoSlice, Read};
use std::mem::MaybeUninit;
use std::net::Shutdown;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::thread::{self, JoinHandle};

use aerith_player::{Plane, Window};
use host::{FakeHost, Interface};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| session(input));

/// One session: the prelude, then each step of `input`, then the end of mpv's socket.
fn session(input: &[u8]) {
    let host = FakeHost::start();
    // The interface's own subsurface or none: the plane goes below it, or below the parent.
    let mut interface = host.connect(input.first().is_some_and(|byte| byte & 1 == 1));
    let (relay_end, mpv_end) = UnixStream::pair().expect("a socket pair");
    let (display, parent, below) = interface.handles();
    // SAFETY: the interface, and so its display and surfaces, lives to the end of this function,
    // after the plane is closed.
    let window = unsafe { Window::new(display, parent, below) };
    let plane = aerith_player::fuzzing::plane(window, relay_end, Box::new(|_| {}))
        .expect("the relay starts");
    plane.place(10, 20, 320, 180).expect("a valid place");
    plane.show();
    interface.commit();
    let reader = read_to_end(&mpv_end);
    let mut mpv = Mpv { socket: mpv_end };
    let mut sent = mpv.prelude();
    for step in steps(input.get(1..).unwrap_or_default()) {
        if !sent {
            break;
        }
        sent = match step {
            Step::Interface(change) => {
                change.apply(&plane, &interface);
                true
            }
            Step::Message(message) => mpv.send(&message),
        };
    }
    let _ = mpv.socket.shutdown(Shutdown::Write);
    // The relay reads all mpv sent, and closes its end once its server is done with mpv.
    reader.join().expect("the reader");
    interface
        .roundtrip()
        .expect("the window's connection lives on");
    host.with(|recorded| assert!(recorded.errors.is_empty(), "{:?}", recorded.errors));
    plane.close();
    interface
        .roundtrip()
        .expect("the window's connection lives on after the plane closed");
    host.with(|recorded| assert!(recorded.errors.is_empty(), "{:?}", recorded.errors));
}

/// Reads what the relay sends mpv until it closes mpv's end.
fn read_to_end(socket: &UnixStream) -> JoinHandle<()> {
    let mut reader = socket.try_clone().expect("a second handle on mpv's end");
    thread::spawn(move || {
        let mut bytes = [0; 65536];
        while let Ok(1..) = reader.read(&mut bytes) {}
    })
}

/// A change of the interface's, made while mpv's messages flow.
enum Change {
    Place(i32, i32, u32, u32),
    Show,
    Hide,
    Commit,
}

impl Change {
    fn apply(&self, plane: &Plane, interface: &Interface) {
        match *self {
            // A size out of range is refused by the plane itself, before the relay.
            Change::Place(x, y, width, height) => {
                let _ = plane.place(x, y, width, height);
            }
            Change::Show => plane.show(),
            Change::Hide => plane.hide(),
            Change::Commit => interface.commit(),
        }
    }
}

/// What a file descriptor sent with a message is.
#[derive(Clone, Copy)]
enum FdKind {
    /// A memory file of this size, as Mesa and mpv send.
    Memory(u32),
    /// A memory file sealed against writing.
    SealedMemory(u32),
    /// One end of a socket: no file at all.
    Socket,
}

/// One message of mpv's.
struct Message {
    object: u32,
    opcode: u16,
    words: Vec<u32>,
    fd: Option<FdKind>,
}

/// One step of an input.
enum Step {
    Interface(Change),
    Message(Message),
}

/// The first id the prelude leaves free.
const FIRST_FREE_ID: u32 = 18;

/// The steps an input holds, each a header of four bytes and the words it announces:
/// - byte 0: the object, 1 to 47, or 0 for a change of the interface's;
/// - byte 1: the opcode, or the kind of change;
/// - byte 2: how many words of arguments follow, up to 8;
/// - byte 3: a file descriptor sent with the message (bits 0-1: none, a memory file, a sealed one,
///   a socket), and whether the first word is replaced by the next free id (bit 2), as a request
///   that makes an object would carry.
///
/// Missing bytes at the end are zeroes.
fn steps(input: &[u8]) -> impl Iterator<Item = Step> + '_ {
    let mut rest = input;
    let mut next_id = FIRST_FREE_ID;
    std::iter::from_fn(move || {
        let (header, after) = rest.split_first_chunk::<4>()?;
        let count = usize::from(header[2] % 9);
        let mut words = Vec::with_capacity(count);
        let mut after = after;
        for _ in 0..count {
            let mut word = [0; 4];
            let taken = after.len().min(4);
            word[..taken].copy_from_slice(&after[..taken]);
            after = &after[taken..];
            words.push(u32::from_le_bytes(word));
        }
        rest = after;
        let object = u32::from(header[0] % 48);
        if object == 0 {
            let word = |at: usize| words.get(at).copied().unwrap_or(0);
            let change = match header[1] % 4 {
                // Signed: a plane may start outside its parent.
                0 => Change::Place(word(0) as i32, word(1) as i32, word(2), word(3)),
                1 => Change::Show,
                2 => Change::Hide,
                _ => Change::Commit,
            };
            return Some(Step::Interface(change));
        }
        if header[3] & 4 != 0
            && let Some(first) = words.first_mut()
        {
            *first = next_id;
            next_id += 1;
        }
        let size = words.get(1).copied().unwrap_or(4096) % (1 << 20);
        let fd = match header[3] & 3 {
            0 => None,
            1 => Some(FdKind::Memory(size)),
            2 => Some(FdKind::SealedMemory(size)),
            _ => Some(FdKind::Socket),
        };
        Some(Step::Message(Message {
            object,
            opcode: u16::from(header[1] % 16),
            words,
            fd,
        }))
    })
}

/// mpv's end of the relay's socket.
struct Mpv {
    socket: UnixStream,
}

/// An argument of the prelude's messages.
enum Arg<'a> {
    Word(u32),
    Text(&'a str),
}

impl Mpv {
    /// Makes what mpv makes at its start: every global bound, a toplevel configured with a frame
    /// of 64x48 on it, a viewport. Returns whether the socket took it all.
    fn prelude(&mut self) -> bool {
        use Arg::{Text, Word};
        let mut sent = self.raw(1, 1, &[Word(2)], None); // wl_display.get_registry
        // The relay's globals, by their names, in the order it makes them, bound from id 3: the
        // output first, as the compositor tells of it while the relay starts.
        let globals = [
            ("wl_output", 4),
            ("wl_compositor", 6),
            ("wl_shm", 1),
            ("zwp_linux_dmabuf_v1", 4),
            ("wp_viewporter", 1),
            ("wp_presentation", 2),
            ("wp_fractional_scale_manager_v1", 1),
            ("wp_content_type_manager_v1", 1),
            ("xdg_wm_base", 6),
        ];
        for (name, (interface, version)) in (1..).zip(globals) {
            let id = name + 2;
            // wl_registry.bind
            sent &= self.raw(
                2,
                0,
                &[Word(name), Text(interface), Word(version), Word(id)],
                None,
            );
        }
        let (compositor, shm, viewporter, shell) = (4, 5, 7, 11);
        let (surface, xdg_surface, toplevel, pool, buffer, viewport) = (12, 13, 14, 15, 16, 17);
        sent &= self.raw(compositor, 0, &[Word(surface)], None); // create_surface
        sent &= self.raw(shell, 2, &[Word(xdg_surface), Word(surface)], None); // get_xdg_surface
        sent &= self.raw(xdg_surface, 1, &[Word(toplevel)], None); // get_toplevel
        sent &= self.raw(surface, 6, &[], None); // commit: the relay sends configure 1
        sent &= self.raw(xdg_surface, 4, &[Word(1)], None); // ack_configure
        let size = 64 * 48 * 4;
        sent &= self.raw(
            shm,
            0,
            &[Word(pool), Word(size)],
            Some(FdKind::Memory(size)),
        ); // create_pool
        // create_buffer: 64x48, stride 256, xrgb8888.
        let buffer_args = [0, 64, 48, 256, 1].map(Word);
        let mut args = vec![Word(buffer)];
        args.extend(buffer_args);
        sent &= self.raw(pool, 0, &args, None);
        sent &= self.raw(viewporter, 1, &[Word(viewport), Word(surface)], None); // get_viewport
        sent &= self.raw(surface, 1, &[Word(buffer), Word(0), Word(0)], None); // attach
        sent &= self.raw(surface, 6, &[], None); // commit
        sent
    }

    /// Sends one of the input's messages. Returns whether the socket took it.
    fn send(&mut self, message: &Message) -> bool {
        let args: Vec<Arg<'_>> = message.words.iter().copied().map(Arg::Word).collect();
        self.raw(message.object, message.opcode, &args, message.fd)
    }

    /// Sends a message, with a file descriptor of `fd`'s kind if any. Returns whether the socket
    /// took it: it does not once the relay closed mpv's connection.
    fn raw(&mut self, object: u32, opcode: u16, args: &[Arg<'_>], fd: Option<FdKind>) -> bool {
        let mut body = Vec::new();
        for arg in args {
            match arg {
                Arg::Word(word) => body.extend(word.to_ne_bytes()),
                Arg::Text(text) => {
                    let length = u32::try_from(text.len() + 1).expect("a short name");
                    body.extend(length.to_ne_bytes());
                    body.extend(text.as_bytes());
                    body.push(0);
                    while body.len() % 4 != 0 {
                        body.push(0);
                    }
                }
            }
        }
        let size = u32::try_from(8 + body.len()).expect("a short message");
        let mut bytes = Vec::with_capacity(8 + body.len());
        bytes.extend(object.to_ne_bytes());
        bytes.extend(((size << 16) | u32::from(opcode)).to_ne_bytes());
        bytes.extend(body);
        let file = fd.map(make_fd);
        send(&self.socket, &bytes, file.as_ref())
    }
}

/// Makes a file descriptor of `kind`.
fn make_fd(kind: FdKind) -> OwnedFd {
    let memory = |size: u32| {
        let flags = rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING;
        let fd = rustix::fs::memfd_create("fuzz-mpv", flags).expect("a memory file");
        rustix::fs::ftruncate(&fd, u64::from(size)).expect("a size");
        fd
    };
    match kind {
        FdKind::Memory(size) => memory(size),
        FdKind::SealedMemory(size) => {
            let fd = memory(size);
            rustix::fs::fcntl_add_seals(&fd, rustix::fs::SealFlags::WRITE).expect("a seal");
            fd
        }
        FdKind::Socket => {
            let (one, _other) = UnixStream::pair().expect("a socket pair");
            OwnedFd::from(one)
        }
    }
}

/// Sends `bytes`, with `fd` if any, in one message. Never raises SIGPIPE, which libFuzzer's
/// process does not ignore. Returns whether all of it went.
fn send(socket: &UnixStream, bytes: &[u8], fd: Option<&OwnedFd>) -> bool {
    let fds: Vec<_> = fd.iter().map(|fd| fd.as_fd()).collect();
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = rustix::net::SendAncillaryBuffer::new(&mut space);
    if !fds.is_empty() && !control.push(rustix::net::SendAncillaryMessage::ScmRights(&fds)) {
        return false;
    }
    let mut written = 0;
    while written < bytes.len() {
        let sent = rustix::net::sendmsg(
            socket,
            &[IoSlice::new(&bytes[written..])],
            &mut control,
            rustix::net::SendFlags::NOSIGNAL,
        );
        match sent {
            Ok(count) => {
                written += count;
                // The file went with the first bytes.
                control.clear();
            }
            Err(rustix::io::Errno::INTR) => {}
            Err(_) => return false,
        }
    }
    true
}
