# 0005. The interface on uwui, as a later client

- Status: Accepted
- Date: 2026-11-11

## Context

uwui, the owner's UI framework in Rust drawn through wgpu, is not usable yet and has no date. Its
code lives in SlimeEngine until Stage 6d moves it out. Applications move onto it when its API is
declared stable, at the end of its first stage, whose programs are Opium, Slatty and Pluto's client;
the DAW Magnolia is a later client. uwui places "video decoded by the GPU, HDR" in the work before
its 1.0, while SlimeEngine's ADR-0190 places HDR from Stage 6d: its documents disagree.

Aerith's first usable version needs video inside the interface, HDR, translation and media keys.

## Decision

**Aerith's interface is written on uwui. Aerith is a later client**, not a program of uwui's first
stage.

**Until uwui allows it, Aerith builds what needs no interface:** the player
([ADR-0003](0003-mpv-in-its-own-process.md)) and the core (addons, accounts, storage, sources). They
are driven by tests and a small test bench; no temporary interface is built in another toolkit.

**Aerith pushes its needs to uwui** in [`uwui-needs.md`](../uwui-needs.md), each marked
indispensable or wished, and asks for a bounded trial before uwui's API is declared stable: video
frames in an element at the display's rate, an HDR video plane under an SDR interface, and a couch
screen driven by a remote. The need for a surface from another process is filed jointly with
Magnolia, whose plugin windows need it too. uwui decides these in its own sessions; Aerith never
changes uwui from here.

## Alternatives considered

- **A program of uwui's first stage**: video, HDR and TV in uwui sooner, but a heavier first stage
  that delays the owner's other applications.
- **Android first on Kotlin and Compose for TV**, as Opium and Pluto do meanwhile: a couch app much
  sooner, but a whole interface written then thrown away.

## Consequences

- Nothing is seen on screen but the test bench until the interface stage; that stage starts when uwui
  holds Aerith's indispensable needs.
- The date of the first usable version depends on uwui's.
