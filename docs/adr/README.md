# Architecture decision records

An ADR records one decision: its context, the decision, the alternatives rejected and the
consequences. Once accepted, only its status changes. A later decision that replaces it whole makes
it `Superseded by NNNN`; one that changes part of it makes it `Accepted; amended by NNNN`, and the
newer one `Accepted; amends NNNN (what)`.

New ADRs copy [`0000-template.md`](0000-template.md) and take the next free number.

| ADR | Title | Status |
|---|---|---|
| [0001](0001-what-aerith-is.md) | What Aerith is | Accepted |
| [0002](0002-mpl-2-0.md) | MPL-2.0 | Accepted; amended by 0010 |
| [0003](0003-mpv-in-its-own-process.md) | On the desktop, mpv in its own process, its image inside the window | Accepted |
| [0004](0004-a-player-engine-per-platform.md) | A player engine per platform: mpv, Media3, AVFoundation | Accepted |
| [0005](0005-the-interface-on-uwui-a-later-client.md) | The interface on uwui, as a later client | Accepted |
| [0006](0006-seven-platforms.md) | Seven platforms, and what each needs | Accepted |
| [0007](0007-a-neutral-client-in-two-variants.md) | A neutral client, in two variants | Accepted |
| [0008](0008-the-quality-bar.md) | The quality bar | Accepted |
| [0009](0009-the-order-of-the-stages.md) | The order of the stages | Accepted |
| [0010](0010-the-logo.md) | The logo | Accepted; amends 0002 |
| [0011](0011-a-pinned-stable-toolchain.md) | A pinned stable toolchain; a nightly only for the checks that need one | Accepted |
