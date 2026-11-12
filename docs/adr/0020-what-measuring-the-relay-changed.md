# 0020. What measuring the relay changed: mpv's opaque region and the compositor's fifo

- Status: Accepted; amends 0015 (what mpv is offered, what of mpv's state reaches the plane)
- Date: 2026-11-12

## Context

[ADR-0015](0015-a-relay-for-mpvs-wayland-surface.md) lists what the relay offers mpv, and says
mpv's state reaches the plane through the relay's own requests. Measuring the plane at M1.4b
([figures](../measurements.md#m14b--inside-the-window-measured)) found two gaps:

- **mpv's buffers carry alpha.** libplacebo asks the Vulkan driver for premultiplied alpha, which
  Mesa offers on Wayland, so Mesa picks formats with alpha (`ARGB2101010` and the like). mpv says
  its picture is opaque through its surface's opaque region instead
  (`video/out/wayland_common.c`, `vo_wayland_set_opaque_region`). The relay dropped that region:
  neither Umbriel nor KWin could cull what lay under the plane, and neither ever scanned the video
  out.
- **mpv paces itself without fifo.** mpv 0.42 presents in FIFO when the compositor offers
  `wp_fifo_v1`, and in MAILBOX, timing each frame itself, otherwise
  (`video/out/vulkan/context_wayland.c`). The relay did not offer it, so on a compositor that has
  it (KWin 6.8), mpv inside Aerith would have paced worse than mpv alone. Under KWin, with fifo,
  a 25 fps film then kept its cadence at 240 Hz (0 to 2 gaps off of 1,500); without, on Umbriel,
  51 to 75. The two runs differ by their compositor too: no run withheld fifo on KWin.

## Decision

**The plane is opaque when mpv's opaque region covers mpv's whole surface.** The relay keeps each
of mpv's regions as far as it needs: empty, one rectangle, or anything else. At mpv's commit, a
region of one rectangle that covers mpv's whole surface makes the plane's whole surface opaque;
any other region makes it not opaque, which costs the compositor some drawing and never shows a
wrong picture. Nothing of mpv's region is issued as mpv sent it.

**mpv is offered `wp_fifo_manager_v1` when the compositor offers it.** One fifo a surface, as the
protocol says; a request on a fifo whose surface is gone ends mpv's connection, never the
window's. mpv's barriers go with the commit they came with, issued on the plane's own fifo. The
compositor ignores the wait while the plane is synchronized, as the protocol requires; the barrier
still applies.

## Alternatives considered

- **Forwarding mpv's opaque region as it is**: exact for any region, but a region of mpv's
  rectangles, issued on a surface the relay scales to the plane, would need scaling too; mpv only
  ever sends its whole surface or nothing.
- **Asking mpv for an opaque format**: libplacebo picks the composite alpha by a fixed order of
  preference, premultiplied before opaque (`src/vulkan/swapchain.c`), whatever mpv asks.
- **Not offering fifo**: mpv then paces in MAILBOX on every compositor, worse than mpv alone on
  those that have fifo.

## Consequences

- The list of what mpv is offered grows by one global, `wp_fifo_manager_v1`, when the compositor
  has it. Colour management still joins at M1.5.
- Fifo mends the cadence where the display's rate over the clip's is far from a whole number, not
  where it is near one (23.976 and 59.94 fps on 240 Hz). Display sync is decided apart.
- Umbriel offers no fifo, and wlroots 0.20 provides none: there, mpv paces itself, inside Aerith
  as alone.
