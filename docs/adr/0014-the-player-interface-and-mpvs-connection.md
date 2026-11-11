# 0014. The player's interface: commands and numbered events; mpv over its standard input

- Status: Accepted; amended by 0016 (the clock)
- Date: 2026-11-11

## Context

Milestone M1.3 drives mpv, a program apart ([ADR-0003](0003-mpv-in-its-own-process.md)), through
`aerith-player`, whose contract [ADR-0004](0004-a-player-engine-per-platform.md) sets. What bears on
its shape:

- The interface's thread never waits on an engine ([ARCHITECTURE](../ARCHITECTURE.md)). At rest,
  Aerith wakes nothing up: 0 wake-ups a second of its own ([ROADMAP](../ROADMAP.md#budgets)).
- Tokens never reach a command line or a log ([AGENTS](../../AGENTS.md#rules-the-code-depends-on)).
  A stream's URL and headers often carry one.
- mpv 0.42 offers its JSON IPC two ways: a socket file it creates (`--input-ipc-server`, which the
  bench and Stremio's and Jellyfin's mpv use), or a connection it inherits
  (`--input-ipc-client=fd://N`). In the second, mpv quits when the connection closes.
- mpv keeps one playlist. A load replaces the entry playing; an entry replaced before it starts
  may end without any event. mpv numbers each entry, and its events say which.
- mpv's options outlive the file they were set for, unless they are given as options of one load.
- What mpv sends to an observer of a property is not complete: when a source replaces one of the
  same sound format, mpv clears the sound's parameters and does not send them again.
- One engine exists. The others (Media3, AVFoundation) come at their stages, one per platform.
- The corpus is 11 GB, made from a 740 MB download ([ADR-0012](0012-the-test-corpus.md)); CI's
  budget is 20 minutes a job.

## Decision

**One concrete `Player`, no engine trait.** Each platform has one engine, chosen when Aerith is
built; a trait would have a single implementation. `Player` exists on Linux; Windows adds its
engine at M1.6.

**Commands return at once; what the engine does comes back as events.** `open`, `play`, `pause`,
`seek` and `stop` hand the command to the player's own thread and return. The events arrive on a
standard channel. Two threads drive mpv, both blocked until something arrives: one reads mpv's
messages, one writes the commands and turns mpv's messages into events.

**Each open is numbered, and the events of a source carry its number** (`Started`, `Clock`,
`Report`, `PictureFailed`, `Ended`). Every open ends with exactly one `Ended`, after one
`Started` if its first frame was shown: an open that a later one replaced, or a stop ended, before
it loaded ends as stopped, as mpv says nothing of it. Opens made from several threads reach the
engine in the order of their numbers. The player matches mpv's number for each entry to the open that made it. An entry that
is no open of Aerith's, such as a playlist's, is not reported; a source that turns out to be a
playlist ends as failed and is stopped, because the player plays one media an open, and because a
playlist's entries would not carry the source's headers.

**mpv reads its IPC on its standard input**, one end of a socket pair
(`--input-ipc-client=fd://0`). No socket file exists, that another program could open: mpv's IPC
can run programs. No loop waits for a file to appear. mpv quits by itself when Aerith ends, however
it ends.

**The position is a clock.** mpv is not asked to send the position, which changes at every frame.
The player reads it when its relation to time changes (the first frame, a seek, a pause, a stream
waiting for data) and sends a `Clock`: a position, the instant it was read, and whether it runs.
The interface computes the position at any moment from it.

**The report is read, not only observed.** mpv sends the properties it is asked to observe when
they change, and the player reports those changes. At each source's first frame, and when mpv
reconfigures its audio or video output, the player reads every property of the report again.
When the source has a picture that mpv dropped, because its video output failed, the player says
so (`PictureFailed`), from the selected state of mpv's tracks.

**Secrets travel over the connection only, and only to their source.** A stream's headers are
options of its own load: mpv sets them for that file and restores them after. mpv's list of
headers escapes a comma, but has no escape for a backslash: a header that holds one ends its open
as failed, before anything is sent. mpv writes no log, and its window's title is fixed, never the
media's name, which for a stream is its URL. The `Debug` of a source shows a stream's scheme and
host only. Only `http` and `https` streams are accepted: mpv knows other protocols (`av://`,
`edl://`) that reach the machine's devices and files.

**mpv runs with nothing of the person's or of its own interface**: no configuration, script, key
binding, terminal or resume file. Its picture is discarded (`--vo=null`) for tests and measures, or
shown through Wayland only (`gpu-next`, `waylandvk` or `wayland`), never by taking the screen
through DRM. Its shader caches, and the GPU driver's, go to a folder Aerith names.

**The tests run in three layers:**

- against a fake mpv on the other end of the socket pair, for every event, error and limit, on
  Linux; Windows runs the crate's other unit tests, of sources and clocks;
- against the real mpv, on clips mpv's own encoder makes in a few milliseconds, in CI: the
  development shell names it (`AERITH_TEST_MPV`, the same build as the bench's mpv). Each mpv runs in
  bubblewrap, the person's home replaced, sound devices and the session's runtime folders out of
  reach, and each test checks that mpv wrote nothing in that home;
- on the whole corpus, by hand, where it exists: the test is ignored unless asked for. It also
  times the first frame against mpv alone, with mpv's defaults and with the player's options.

## Alternatives considered

- **A socket file (`--input-ipc-server`)**: what everyone does, but a file any program of the
  person can open, a loop that waits for it, a path limited to 108 bytes, files left behind by a
  crash, and an mpv that outlives a killed Aerith.
- **A request-and-answer interface**, each command waiting for mpv's answer: simpler to call, but
  the interface's thread would wait on mpv.
- **Callbacks instead of a channel**: no thread to forward events to an interface's loop, but the
  caller's code would run on the player's thread and could stall it. A channel is the standard
  library's; a callback can be added when the interface's loop needs one.
- **Events without the open's number**: a smaller interface, but an event of a source just
  replaced reads as the new one's, and an open replaced before it loads ends without a word.
- **Observing the position**: no arithmetic for the interface, but a wake-up of Aerith at each
  frame, 24 to 60 a second.
- **Reading the position again every 10 to 30 s while playing**: it would bound the clock's drift
  against mpv's own (below), at the cost of a wake-up each time. Not needed while mpv plays at its
  nominal speed; to decide with display sync, at M1.4.
- **The headers as one global option, set before each load**: one command less, but mpv reads the
  option when its file opens, after the load has returned: a load that follows at once can give
  its headers to the source before it. A test of the first version sent a token so, once in 390
  tries of two opens in a row.
- **One mpv per source**: nothing carried from one source to the next, by construction, but mpv's
  start at each open, unless a spare one is started ahead; options of each load give the same
  isolation for the headers.
- **An engine trait now**: ready for the second engine, but one implementation, and the shape of
  the second unknown.
- **The corpus in CI**: the real clips on every change, but 740 MB to fetch and hours to make, or
  a cache to keep, for each run.

## Consequences

- Dropping a player waits up to two seconds for mpv to quit, then kills it: the interface drops it
  off its thread, or at exit.
- An mpv that freezes without exiting is not detected: commands go unanswered. A watchdog needs a
  timer while a command waits; it is recorded as debt.
- The interface's loop needs to wake when an event arrives from another thread
  ([uwui-needs](../uwui-needs.md#general)).
- Codec, format and colour names are mpv's; a second engine will need common ones.
- mpv fetches streams itself, outside the I/O layer of Stage 2. FFmpeg follows redirects to other
  hosts and sends them the headers it was given, but `Authorization`. The core must check a
  stream's host against the I/O layer's rules before it opens it, and give a token only to a
  source whose redirects it trusts, or resolve them first.
- mpv runs as the person, with the person's session: a decoder's exploit would reach the session
  bus, through which the keyring answers, and the home. The separate process isolates crashes,
  not tokens, until mpv runs in a sandbox: recorded as debt, due before Stage 2 stores a token.
- The clock runs at real time from each reading. If mpv changes its speed to match the display
  (`video-sync=display-resample`, up to 1 %), the position drifts by up to 36 s an hour.
- The program that holds the player must ignore `SIGPIPE`, as Rust programs do unless told
  otherwise: a command written as mpv dies would otherwise end it.
- The numbering relies on two orders mpv keeps today but documents only in part: its answer to a
  load comes before that entry's `start-file`, and a reading's answer comes after the events mpv
  sent before it. mpv's manual guarantees only that answers come in the order of the commands. A
  later mpv that breaks them would show as an open that never starts, or a clock read with a stale
  state; the tests against the real mpv would catch the first.
- Between a playlist's `end-file` and the stop the player sends, mpv may start fetching the
  playlist's first entry, without the source's headers. `--access-references=no` would stop mpv
  from following playlists, but also Matroska's ordered chapters and other references: not
  chosen.
- On Windows, mpv takes the connection as an inherited named pipe (`handle://`) with overlapped
  I/O; M1.6 builds it.
- CI's shell downloads mpv: 752 MiB once unpacked, with what it needs, shared in part with the
  rest.
- The corpus test, ignored by CI, is run at each milestone that touches playback, and its result
  recorded in [`measurements.md`](../measurements.md).
