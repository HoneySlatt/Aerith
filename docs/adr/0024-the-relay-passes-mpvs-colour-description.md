# 0024. The relay passes mpv's colour description to the plane; the interface declares none

- Status: Accepted; amends 0015 (what mpv is offered)
- Date: 2026-11-14

## Context

[ADR-0015](0015-a-relay-for-mpvs-wayland-surface.md) left colour management out of what the relay
offers mpv, "to join at M1.5". Without it, mpv inside Aerith tone-maps every HDR film to SDR, and
dithers to 8 bits where mpv alone does not: the two pictures differ by one step of 8 bits
([M1.4b](../measurements.md#a-first-pass-on-the-desktop)). What bears on it, read on 2026-11-14 in
the pinned sources (mpv 0.42.0, libplacebo 7.387, Mesa 26.2.6, wlroots 0.20.2, Umbriel `d171ccb`,
KWin 6.8.2, niri 26.04, and Yuna's ADR-0047 and ADR-0048):

- **The protocol is `wp_color_manager_v1`**, version 3 in wayland-protocols 1.49. A client
  describes a surface with an *image description*: named transfer function and primaries,
  luminances, the mastering display, MaxCLL and MaxFALL. A compositor tells the surface the
  description it prefers, its output's. Many wrong values are protocol errors, fatal to the whole
  connection.
- **mpv sets its own surface's description** (`video/out/wayland_common.c`), on gpu-next with
  Vulkan. It binds version 2 at most, asks for the preferred description first and waits for each
  description to be `ready` before setting it. It forces its swapchain to `PASS_THROUGH`, so that
  Mesa does not describe the surface too (`video/out/vulkan/context.c`). Given a colour manager, it
  no longer dithers to 8 bits (`video/out/vulkan/context_wayland.c`).
- **What mpv sets depends on the preferred description.** On an SDR output, mpv tone-maps the film
  itself and describes an SDR picture. On an HDR output, it describes BT.2020 with PQ, 0 to
  10,000 cd/m² with a reference of 203, the film's mastering display and MaxCLL, and leaves the
  display's own range to the compositor.
- **Mesa binds the manager too, at version 1**, in mpv's process. It describes a surface only
  when its swapchain is not `PASS_THROUGH`; for an sRGB swapchain it does so only when the
  compositor lists the `srgb` transfer function. mpv's first swapchain is sRGB, before mpv's first
  hint. A second description object on one surface is a fatal error (`surface_exists`).
- **Compositors differ in what they refuse.** wlroots 0.20 offers version 2 and refuses
  `set_luminances`; KWin 6.8 refuses a MaxCLL or MaxFALL set twice; Umbriel accepts the render
  intent *perceptual* only; Yuna follows KWin's list. niri, on the lowest laptop, offers no colour
  management.
- **On Umbriel, a description with luminances ends the plane's scanout.** mpv sends
  `set_luminances` whenever the compositor offers it, for SDR as for HDR, and Umbriel then
  composites the surface (`umbrielfx/types/scene/wlr_scene.c`, `color_management_is_scanout_allowed`).
  mpv alone shows it today: `off (color_conversion)` on the HDR clip. Under display sync, Aerith's
  plane scanned out costs 0.9 to 3.1 W less than mpv alone composited
  ([M1.4c](../measurements.md#the-rule-on-the-desktop-at-240-hz)).
- **The interface's surfaces** (the black root and the controls' surface,
  [ADR-0019](0019-the-video-above-a-black-root.md)) are SDR. A surface that declares nothing is
  sRGB to the compositor, which places its white at the output's SDR white, a setting of the
  person's (Umbriel's `sdr_white`, KWin's reference luminance).
- **No compositor without a screen offers an HDR output** today: wlroots' headless outputs and
  KWin's virtual ones take no image description. Yuna's virtual outputs announce one, but Yuna
  draws in HDR only from its Stage 5.

## Decision

The owner's decisions of 2026-11-14.

**The relay offers mpv `wp_color_manager_v1`, checked and issued again, as it does every other
protocol.** mpv's descriptions are created again on the window's connection, under the relay's
checks; `ready` and `failed` go back to mpv; the plane's surface gets mpv's description with the
commit it came with. The plane's preferred description, which the compositor sends the relay,
goes back to mpv with its information. What is offered is listed:

- offered: the parametric creator (named transfer function and primaries, luminances, the
  mastering display, MaxCLL, MaxFALL), surface objects and their feedback, the intents, transfer
  functions and primaries the window's compositor advertises;
- not offered: ICC profiles, which pass a file descriptor the relay cannot check (as sync objects,
  [ADR-0018](0018-what-building-the-relay-changed.md)); custom primaries, power transfer
  functions, Windows' scRGB and BT.2100, an output's own description, and
  `wp_color_representation_v1`, which mpv 0.42 does not need on this path;
- never the `srgb` transfer function, so that Mesa never describes mpv's surface behind mpv;
  `compound_power_2_4` only to version-2 bindings. Values and events (`ready` and `ready2`,
  `preferred_changed` and its second form) are translated to the version each binding of mpv's
  asked for;
- at most version 2, the highest mpv binds.

**The checks are the union of the compositors' refusals**: whatever wlroots 0.20, Umbriel, KWin
6.8 or Yuna would refuse ends mpv's connection, never the window's. A description that fails on
the compositor goes back to mpv as `failed`, and is counted for the bench: mpv then keeps its
target, and its picture may be wrong.

**Whether `set_luminances` is offered is decided by a measure.** The relay is built with it; on
Umbriel at 240 Hz under display sync, it is measured with and without (scanout, GPU power, frames
against mpv alone), and the owner chooses on the figures. This ADR is amended with the choice.

**The interface's surfaces declare no description.** The compositor treats them as sRGB at the
person's SDR white. A description for each of the interface's surfaces is uwui's need for Stage 3
([`uwui-needs.md`](../uwui-needs.md)).

**Dolby Vision is reported as mpv applies it**: `video-params/colormatrix` reads `dolbyvision`
when mpv reshapes a Dolby Vision picture. The profile and level the container declares are
reported as the container's claim, apart.

**Tested without a screen against the relay's fake compositor**, which gains a colour manager with
an SDR and a PQ preferred description: the real mpv plays each HDR and Dolby Vision clip of the
corpus, and the tests check the description set on the plane, for bindings at versions 1 and 2.
The HDR path is checked by hand on the owner's display, under Umbriel with HDR on, then under
KWin. Yuna joins Aerith's tests when it draws HDR, by a decision of its own; Yuna's weekly job
plays Aerith meanwhile ([Yuna's ADR-0050](https://github.com/HoneySlatt/Yuna/blob/main/docs/adr/0050-mpv-in-the-applications-tests.md)).

## Alternatives considered

- **The player tags the plane itself**, mpv told its target by options and offered no colour
  manager: no new protocol facing mpv, but mpv then dithers to 8 bits, PQ in 8 bits bands, and a
  change of description no longer goes with the frame it belongs to.
- **Not passing it**: mpv tone-maps every film to SDR; HDR never reaches an HDR display, and the
  image floor is missed by one step of 8 bits.
- **Offering everything the compositor offers, unchecked against other compositors**: simpler,
  but a value one compositor accepts and another refuses would close the window there.
- **The interface declaring sRGB with a fixed reference luminance**: the same look on every
  compositor, but against the person's own setting, and colour code in a test window that goes at
  Stage 3.
- **The interface drawing in PQ**: it could dim its controls against the film's peak, but needs
  HDR rendering in the interface, which uwui does not have.
- **Yuna in Aerith's tests now**: an HDR output without a screen, but on a colour handler decided
  the same day, a young interface, and two repositories pinning each other.

## Consequences

- The largest protocol the relay writes by hand. The fuzz target covers it.
- mpv inside Aerith shows what mpv alone shows wherever the compositor offers colour management,
  dithering included. Where it does not (niri, on the lowest laptop), both tone-map to SDR and
  dither to 8 bits alike.
- The relay's checks follow four compositors now; another one (Mutter, Weston) may refuse what
  they accept, as the relay's other checks may.
- On Umbriel, mpv alone's HDR plane is never scanned out; Aerith's may be, depending on the
  measure above.
- uwui, when it describes its own surfaces, must present its swapchain in `PASS_THROUGH`, or Mesa
  takes the surface's description first ([`uwui-needs.md`](../uwui-needs.md)).
- The HDR path has no test on a real compositor without the owner's screen until Yuna draws HDR.
