// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The relay's server side against a fake mpv that sends what the fuzzer gives: Wayland messages
//! to any object, with any arguments, some with a file descriptor, between changes of the
//! interface's (place, show, hide, commit).
//!
//! Each input runs a whole session: the fake compositor of the player's tests
//! (`tests/common/host.rs`), with a colour manager, and an interface connected to it, the relay on
//! the interface's window, and mpv's end of the relay's socket. A prelude first makes what mpv
//! makes (every global bound, a toplevel configured, a frame on the plane, a viewport, a colour
//! description set on the plane), so that the input starts from mpv's usual state; the input's
//! messages follow.
//!
//! What must hold, whatever the input: nothing panics, in any thread (libFuzzer aborts the
//! process on a panic); the relay reads mpv's socket to its end and closes it, or closes it
//! before; and the interface's connection lives on, the fake compositor having raised no error on
//! it, during the session and after the plane is closed. The prelude must work too: a prelude the
//! relay refuses, as when the relay's globals change their order, would leave every input to a
//! closed connection, so the session fails at once if its colour description never becomes
//! ready.

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
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use aerith_player::{Plane, Window};
use host::{ColourList, FakeHost, Interface};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| session(input));

/// One session: the prelude, then each step of `input`, then the end of mpv's socket.
fn session(input: &[u8]) {
    let host = FakeHost::start_with_colour(ColourList::default());
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
    let (reader, ready) = read_to_end(&mpv_end, DESCRIPTION);
    let mut mpv = Mpv { socket: mpv_end };
    let mut sent = mpv.prelude();
    assert!(
        ready.recv_timeout(PRELUDE_WAIT).is_ok(),
        "the prelude's colour description never became ready: the relay refused the prelude"
    );
    sent &= mpv.set_description();
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

/// How long the prelude waits for its colour description: the relay and the fake compositor
/// answer within milliseconds.
const PRELUDE_WAIT: Duration = Duration::from_secs(5);

/// Reads what the relay sends mpv until it closes mpv's end, and tells, once, when the
/// description `watched` is ready (`ready` or `ready2`, opcodes 1 and 2).
fn read_to_end(socket: &UnixStream, watched: u32) -> (JoinHandle<()>, Receiver<()>) {
    let mut reader = socket.try_clone().expect("a second handle on mpv's end");
    let (tell, ready) = mpsc::channel();
    let thread = thread::spawn(move || {
        let mut bytes = [0; 65536];
        // What was read of the message under way.
        let mut stream = Vec::new();
        while let Ok(count @ 1..) = reader.read(&mut bytes) {
            stream.extend_from_slice(&bytes[..count]);
            // Each message: the object's id, then its size, header included, over its opcode.
            while let Some(&[a, b, c, d, e, f, g, h]) = stream.first_chunk::<8>() {
                let object = u32::from_ne_bytes([a, b, c, d]);
                let word = u32::from_ne_bytes([e, f, g, h]);
                let size = usize::try_from(word >> 16).expect("a 16-bit size");
                if size < 8 {
                    // Not a message: the rest is only drained.
                    stream.clear();
                    break;
                }
                if stream.len() < size {
                    break;
                }
                if object == watched && matches!(word & 0xffff, 1 | 2) {
                    let _ = tell.send(());
                }
                stream.drain(..size);
            }
        }
    });
    (thread, ready)
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

/// The ids of the prelude's colour objects: the colour-management surface of mpv's surface, its
/// feedback, a parametric creator, and the description it creates.
const COLOUR_SURFACE: u32 = 20;
const FEEDBACK: u32 = 21;
const CREATOR: u32 = 22;
const DESCRIPTION: u32 = 23;

/// mpv's surface.
const SURFACE: u32 = 14;

/// The first id the prelude leaves free.
const FIRST_FREE_ID: u32 = 24;

/// The steps an input holds, each a header of four bytes and the words it announces:
/// - byte 0: the object, 1 to 47, or 0 for a change of the interface's; the prelude made 1 to 23;
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
    /// of 64x48 on it, a viewport, and a colour-management surface, a feedback and an HDR
    /// description, as mpv 0.42 makes them. Returns whether the socket took it all.
    fn prelude(&mut self) -> bool {
        use Arg::{Text, Word};
        let mut sent = self.raw(1, 1, &[Word(2)], None); // wl_display.get_registry
        // The relay's globals, by their names, in the order it makes them (`offer_globals`),
        // bound from id 3: the output first, as the compositor tells of it while the relay
        // starts.
        let globals = [
            ("wl_output", 4),
            ("wl_compositor", 6),
            ("wl_shm", 1),
            ("zwp_linux_dmabuf_v1", 4),
            ("wp_viewporter", 1),
            ("wp_presentation", 2),
            ("wp_fractional_scale_manager_v1", 1),
            ("wp_content_type_manager_v1", 1),
            ("wp_fifo_manager_v1", 1),
            ("wp_color_manager_v1", 2),
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
        let (compositor, shm, viewporter, colour, shell) = (4, 5, 7, 12, 13);
        let surface = SURFACE;
        let (xdg_surface, toplevel, pool, buffer, viewport) = (15, 16, 17, 18, 19);
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
        sent &= self.raw(colour, 2, &[Word(COLOUR_SURFACE), Word(surface)], None); // get_surface
        // get_surface_feedback
        sent &= self.raw(colour, 3, &[Word(FEEDBACK), Word(surface)], None);
        sent &= self.raw(colour, 5, &[Word(CREATOR)], None); // create_parametric_creator
        // PQ, BT.2020, 0 to 10,000 cd/m² with a reference of 203, the mastering display, MaxCLL
        // and MaxFALL.
        sent &= self.raw(CREATOR, 1, &[Word(11)], None); // set_tf_named
        sent &= self.raw(CREATOR, 3, &[Word(6)], None); // set_primaries_named
        sent &= self.raw(CREATOR, 5, &[0, 10_000, 203].map(Word), None); // set_luminances
        let bt2020 = [
            708_000, 292_000, 170_000, 797_000, 131_000, 46_000, 312_700, 329_000,
        ];
        // set_mastering_display_primaries
        sent &= self.raw(CREATOR, 6, &bt2020.map(Word), None);
        sent &= self.raw(CREATOR, 7, &[50, 1_000].map(Word), None); // set_mastering_luminance
        sent &= self.raw(CREATOR, 8, &[Word(1_000)], None); // set_max_cll
        sent &= self.raw(CREATOR, 9, &[Word(400)], None); // set_max_fall
        sent &= self.raw(CREATOR, 0, &[Word(DESCRIPTION)], None); // create
        sent
    }

    /// Sets the prelude's description, ready, on mpv's surface, and commits it. Returns whether
    /// the socket took it.
    fn set_description(&mut self) -> bool {
        use Arg::Word;
        // set_image_description, perceptual
        let mut sent = self.raw(COLOUR_SURFACE, 1, &[Word(DESCRIPTION), Word(0)], None);
        sent &= self.raw(SURFACE, 6, &[], None); // commit
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
