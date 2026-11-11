# AGENTS.md

Rules for AI agents working in this repository. Read this file fully at the start of every session.

## Project

Aerith is an open-source video client for films and series, in Rust: Stremio addons, the person's
own files and Jellyfin servers; Nuvio, Trakt and a sync of its own. It aims at Linux, Windows,
macOS, iPhone, Apple TV, Android phones and Android TV, at a desk and on a couch. Its interface will
be written on uwui, the owner's UI framework, which is not ready yet. It is licensed under MPL-2.0
([ADR-0002](docs/adr/0002-mpl-2-0.md)).

Its real state: the design is done; the Cargo workspace and its checks exist (M1.1); the bench,
`aerith-bench`, measures the rivals (M1.2); `aerith-player` drives mpv on Linux, without a window of
Aerith's own yet (M1.3). Stage 1, the player on the desktop, is under way.

Agents develop it; the owner sets its direction and validates its decisions. Anyone must be able to
understand it from the repository alone. Documentation is part of every deliverable.

## Read before working

- `docs/PLAN.md`, its *Current* section and the current milestone: what to do now.
- When planning, or when the task touches a decision: `docs/ROADMAP.md` (direction, stages, the
  quality bar), `docs/adr/`, and `docs/ARCHITECTURE.md`.
- `docs/uwui-needs.md` when the work touches the interface; `docs/survey.md` for the field's facts
  and their sources.

You remember nothing from earlier sessions. What is not written in these files is lost.

## Scope

- Work on the current milestone of `docs/PLAN.md`, or on what the owner asks. A request that changes
  the plan is written into the PLAN.
- Do not start later stages, nor prepare abstractions for them.
- When experience contradicts the roadmap, propose a change; once accepted, record it in an ADR.
- Never change uwui or SlimeEngine from here. What Aerith asks of uwui goes into
  `docs/uwui-needs.md`, for the owner to carry.

## Commands

```sh
nix develop          # the development shell: every tool the checks use
scripts/check.sh     # every check of the definition of done; what CI runs
nix develop .#bench  # the bench's shell: the corpus's tools and the rivals
```

`CONTRIBUTING.md` holds the commands to build, test and run each check alone, and to run the bench.

Never on your own:

- sign in to the owner's Nuvio, Trakt, Jellyfin or other accounts, or use their tokens: tests use
  local fakes and pinned self-hosted backends;
- play from the owner's library or NAS: the bench plays the test corpus;
- install an addon or plugin that points at unlicensed content;
- start a virtual machine, or reach the owner's desktop outside `owner-desktop`.

## Definition of done

- `scripts/check.sh` passes.
- The program was run and behaved as expected.
- New behaviour is tested where a test is practical.
- The budgets the milestone touches are measured and met; the figures go to
  `docs/measurements.md`.
- `docs/PLAN.md` gives the new status.
- Every document the change affects is updated and reread as a newcomer would.
- Every decision a newcomer would question is an ADR.

Never report as working what was not verified. Say exactly what was not, and why. Never weaken a
check to make it pass (a skipped test, a raised budget, a silenced lint) without the owner's go.

## Architecture rules

- The rest of Aerith reaches playback only through `aerith-player`; no engine detail leaks out.
- Decoders written in C (mpv, FFmpeg, libass) and JavaScript plugins never run in the process that
  holds tokens.
- The core runs without a window, a GPU or the network, so that it is tested with local fakes.
- Platform code (Kotlin, Swift, Windows) lives in its own folders, as thin as the platform allows.

## Rules the code depends on

- Report what the engine observed, never what a file name or an addon claims: Dolby Vision, HDR,
  Atmos and hardware decoding are shown only when the engine reports them.
- Tokens and secrets live only in the system's keyring, never in files, logs, command lines or
  crash reports.
- Every network access goes through the I/O layer and its rules: allowed hosts, sizes, times.
  Addon URLs may point at the person's local network; requests to it are deliberate.
- Untrusted input (addon responses, subtitles, file names, plugin output) is bounded in size and
  parsed by fuzzed code.
- Store builds contain neither the torrent engine nor JavaScript plugins
  ([ADR-0007](docs/adr/0007-a-neutral-client-in-two-variants.md)).
- Aerith installs no content source by default.
- Every file starts with the MPL-2.0 header, but the few that cannot carry it, which
  `scripts/check.sh` names. Nothing under the GPL enters Aerith's own binaries; mpv ships as a
  program apart.

## Code rules

- No abstraction without a second concrete use.
- No reimplementation of the standard library without a measurement.
- A new dependency needs a reason, an alternative weighed and a license checked; ask the owner.
- Every `unsafe` block has a `// SAFETY:` comment.
- Code reads plainly to someone who has never seen it: names that say what things are, one job per
  function, the obvious solution over the clever one.
- Comments say why, not what. No commented-out code.
- A bug is fixed at its cause, with the test that failed first; look for the same bug elsewhere.
- Edge cases are thought through and tested: empty, huge, malformed, concurrent, interrupted.
- The user's data is never lost: versioned formats, atomic writes, a backup before a destructive
  step.
- Search for an existing helper before writing one. One way to do a thing.
- Delete dead code and unused parameters.
- Report drift from `docs/ARCHITECTURE.md` or an ADR; do not work around it.

## Documentation

- It is written for a newcomer: plain words, short sentences, the essential first, as short as
  complete.
- It describes the code as it is now. Wrong documentation is worse than none.
- No reference to conversations the reader cannot see. Coined terms go in `docs/glossary.md`.
- Limits and debt are stated plainly.
- Each fact has one home; other documents link to it:
  - the code's documentation: contracts, limits, figures;
  - `docs/ARCHITECTURE.md`: flows across parts;
  - ADRs: why a decision was taken, what was rejected; only their status changes later;
  - `docs/PLAN.md`: status and known debt;
  - `CHANGELOG.md`: what changed for users, from the first release.
- A change that leaves a document wrong is not finished.

## Commits

- One logical change per commit, in English, its subject in the imperative.
- Never commit code that fails the checks. A check that fails on the main branch is fixed before
  any new work.

## When to stop and ask

Before a decision no ADR covers, a new dependency, a request against the roadmap or an ADR, a plan
that turns out wrong, or deleting or rewriting a large part of the code. Give the options, their
trade-offs in plain terms and a recommendation for the long term, one decision at a time.

## Reporting

A stage ends with a stage audit, and a release is preceded by a release audit; their findings go to
`docs/audits/`. Ideas outside the current work go to the *Ideas* of `docs/PLAN.md`.

At the end of each milestone, after a reviewing agent on the strongest model has checked it against
this file, the ADRs and the quality bar, report: what was done; the choices, why, and the
alternatives rejected; what to read first; the figures measured; shortcuts and debt; the next step,
on the owner's go. The next milestone starts in a new session, unless the owner says to continue.

## Working with several agents

- One milestone per agent, as `docs/PLAN.md` assigns it.
- Shared files (`docs/PLAN.md`, `docs/ARCHITECTURE.md`, the build manifests): one agent at a time.
- Commit your files by their paths, never with `git add -A`.
- Delegate broad searches to subagents; keep their conclusions, not the files they read.

## Language

Everything in the repository is in English.
