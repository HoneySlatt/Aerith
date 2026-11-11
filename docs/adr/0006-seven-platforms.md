# 0006. Seven platforms, and what each needs

- Status: Accepted
- Date: 2026-11-11

## Context

The owner wants Aerith at a desk and on a couch, on computers, phones and TVs. uwui's scope
(SlimeEngine ADR-0181) aims at Linux, Windows, Android phones and the web; macOS and iOS are "within
reach, without a port"; Android TV and Apple TV are out of scope. The owner opened a design session
in uwui to reconsider its targets.

Facts that bound the Apple platforms ([survey](../survey.md#apple-platforms)): Xcode 27 runs only on
Apple silicon and macOS 26 is the last release for Intel Macs; building and signing for iPhone and
Apple TV need Xcode; GitHub's macOS runners on Apple silicon are free for public repositories;
Apple's licence allows macOS in a virtual machine only on Apple-branded computers.

## Decision

**Aerith targets Linux, Windows, macOS, iPhone, Apple TV, Android phones and Android TV**, in the
order of the [ROADMAP](../ROADMAP.md#4-stages).

**Each platform's interface is uwui's.** What Aerith needs of uwui on Android TV and Apple TV (the
remote's keys in the focus model, a surface inside a native activity or a UIKit shell, the tvOS focus
engine) goes to [`uwui-needs.md`](../uwui-needs.md); uwui decides its targets in its own session.

**Hardware each platform needs**, named in the [quality bar](../ROADMAP.md#hardware):

- Linux and Windows: the owner's desktop and the lowest laptop.
- Android: the owner's Pixel and mid-range phone; an Android TV box, to be named and acquired before
  the Android TV stage.
- Apple: the owner's Apple TV; an Apple-silicon Mac and an iPhone, to be acquired before the Apple
  stage, and the paid developer programme.

## Alternatives considered

- **No Android TV**, the couch through a desktop in the living room: simpler, but a platform the
  owner wants is lost.
- **A separate TV interface in Compose for TV**: independent of uwui, but two interfaces to maintain
  for ever, and a look that diverges.

## Consequences

- Android TV and Apple TV depend on uwui taking them; if it does not, this ADR is reopened.
- The Apple stage needs money and hardware the owner does not have yet.
- A tier with no device is stated as unmeasured, never assumed.
