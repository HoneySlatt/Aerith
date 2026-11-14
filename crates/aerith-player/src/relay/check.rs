// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The checks the relay makes on mpv's values before it issues anything on the window's
//! connection: each refuses what wlroots 0.20 (the base of Umbriel, the owner's compositor) would
//! raise as a protocol error, which would close the whole connection, and some more that the relay
//! can tell cheaply. Colour management's checks are the union of what wlroots 0.20, Umbriel,
//! KWin 6.8 and Yuna refuse ([ADR-0024]). A refusal ends mpv's connection, never the window's.
//!
//! Each function names the code it mirrors. They take plain values and file descriptors, so that
//! they are tested apart from any connection. The relay's fuzz target (`fuzz/`) reaches them
//! through mpv's requests.
//!
//! [ADR-0024]: https://github.com/HoneySlatt/Aerith/blob/main/docs/adr/0024-the-relay-passes-mpvs-colour-description.md

use std::os::fd::{AsFd, BorrowedFd};

use rustix::fs::{FileType, OFlags, SealFlags, SeekFrom};

/// Why a request was refused: the protocol's error code on the object that sent it, and a message
/// for mpv's log and the player's report.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Refusal {
    pub(super) code: u32,
    pub(super) message: String,
}

impl Refusal {
    pub(super) fn new(code: impl Into<u32>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// A viewport's source rectangle, in the surface's coordinates before the viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Source {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) width: f64,
    pub(super) height: f64,
}

/// `wp_viewport.set_source`: all four -1 unsets it; otherwise x and y are at least 0, width and
/// height above 0 (wlroots types/wlr_viewporter.c, `viewport_handle_set_source`).
pub(super) fn viewport_source(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    bad_value: u32,
) -> Result<Option<Source>, Refusal> {
    if [x, y, width, height] == [-1.0; 4] {
        return Ok(None);
    }
    // A wl_fixed is always finite; the test for it costs nothing and keeps NaN out of the sums
    // below whatever the caller passes.
    let finite = [x, y, width, height].iter().all(|v| v.is_finite());
    if !finite || x < 0.0 || y < 0.0 || width <= 0.0 || height <= 0.0 {
        return Err(Refusal::new(
            bad_value,
            format!("wp_viewport.set_source({x}, {y}, {width}, {height}): invalid values"),
        ));
    }
    Ok(Some(Source {
        x,
        y,
        width,
        height,
    }))
}

/// `wp_viewport.set_destination`: both -1 unsets it; otherwise both above 0 (wlroots
/// `viewport_handle_set_destination`).
pub(super) fn viewport_destination(
    width: i32,
    height: i32,
    bad_value: u32,
) -> Result<Option<(i32, i32)>, Refusal> {
    match (width, height) {
        (-1, -1) => Ok(None),
        (1.., 1..) => Ok(Some((width, height))),
        _ => Err(Refusal::new(
            bad_value,
            format!("wp_viewport.set_destination({width}, {height}): invalid values"),
        )),
    }
}

/// `wl_surface.set_buffer_scale`: above 0 (wlroots types/wlr_compositor.c,
/// `surface_handle_set_buffer_scale`).
pub(super) fn buffer_scale(scale: i32, invalid_scale: u32) -> Result<i32, Refusal> {
    if scale > 0 {
        Ok(scale)
    } else {
        Err(Refusal::new(
            invalid_scale,
            format!("wl_surface.set_buffer_scale({scale}): the scale is not positive"),
        ))
    }
}

/// What a commit of a surface is checked against, once mpv's pending state is merged into its
/// last one.
#[derive(Clone, Copy, Debug)]
pub(super) struct CommitState {
    /// The size in pixels of the buffer the surface holds, `(0, 0)` for none. As in wlroots, it is
    /// the size of the buffer last committed, kept when that buffer is destroyed afterwards: the
    /// compositor still holds its picture, and checks every later commit against its size.
    pub(super) buffer: (i32, i32),
    pub(super) scale: i32,
    /// Whether the transform turns the buffer by 90 or 270 degrees.
    pub(super) transposed: bool,
    pub(super) source: Option<Source>,
    pub(super) destination: Option<(i32, i32)>,
}

impl CommitState {
    /// The surface's size in its own coordinates, as the compositor computes it: the viewport's
    /// destination, else its source's size, else the buffer's size divided by its scale and turned
    /// by its transform. `(0, 0)` for no buffer.
    pub(super) fn surface_size(&self) -> (i32, i32) {
        if let Some(destination) = self.destination {
            return destination;
        }
        if let Some(source) = self.source {
            // Whole when no destination is set: `commit` refuses it otherwise.
            return (source.width as i32, source.height as i32);
        }
        let (width, height) = (self.buffer.0 / self.scale, self.buffer.1 / self.scale);
        if self.transposed {
            (height, width)
        } else {
            (width, height)
        }
    }
}

/// The checks wlroots makes at a commit (types/wlr_compositor.c, `surface_finalize_pending`, and
/// types/wlr_viewporter.c, `viewport_handle_surface_client_commit`): the buffer's size is a
/// multiple of its scale when no source is set; the source's size is whole when no destination is
/// set; the source lies inside the buffer, when there is one. Each refusal names its code:
/// `wl_surface.invalid_size`, `wp_viewport.bad_size`, `wp_viewport.out_of_buffer`.
pub(super) fn commit(
    state: CommitState,
    invalid_size: u32,
    bad_size: u32,
    out_of_buffer: u32,
) -> Result<(), Refusal> {
    let (width, height) = state.buffer;
    if state.source.is_none() && (width % state.scale != 0 || height % state.scale != 0) {
        return Err(Refusal::new(
            invalid_size,
            format!(
                "a buffer of {width}x{height} is not divisible by the scale {}",
                state.scale
            ),
        ));
    }
    if let Some(source) = state.source {
        if state.destination.is_none()
            && (source.width.fract() != 0.0 || source.height.fract() != 0.0)
        {
            return Err(Refusal::new(
                bad_size,
                "the viewport's source size is not whole and no destination is set",
            ));
        }
        // wlroots' `wlr_surface_state_has_buffer`: a size of zero is no buffer.
        if width > 0 && height > 0 {
            let (mut width, mut height) = (width / state.scale, height / state.scale);
            if state.transposed {
                std::mem::swap(&mut width, &mut height);
            }
            if source.x + source.width > f64::from(width)
                || source.y + source.height > f64::from(height)
            {
                return Err(Refusal::new(
                    out_of_buffer,
                    "the viewport's source lies outside the buffer",
                ));
            }
        }
    }
    Ok(())
}

/// The most file descriptors mpv may have the relay hold at once: one for each shared-memory pool
/// alive. Mesa destroys its pools as soon as it made their buffer; mpv keeps one for its cursor.
pub(super) const MOST_POOL_FDS: usize = 64;

/// The file system a shared-memory pool may live on: memory, as `memfd_create` makes it (tmpfs).
/// Any other is refused: the compositor maps the pool, and a file it cannot map is a protocol error
/// on the window's connection. A file on disk, in /proc or /sys may not map; a huge-page memfd
/// (hugetlbfs) maps only while the system has huge pages reserved, which it has not by default.
const TMPFS_MAGIC: i128 = 0x0102_1994;

/// `wl_shm.create_pool`: the size is above 0 (wlroots types/wlr_shm.c, `shm_handle_create_pool`),
/// and the compositor can map `size` bytes of the file read and write, as wlroots does
/// (`mapping_create`). The relay never maps it itself: it reads the file's kind, its size, how it
/// was opened and its seals.
pub(super) fn shm_pool(
    fd: impl AsFd,
    size: i32,
    invalid_fd: u32,
    invalid_stride: u32,
) -> Result<(), Refusal> {
    if size <= 0 {
        return Err(Refusal::new(
            invalid_stride,
            format!("wl_shm.create_pool: invalid size {size}"),
        ));
    }
    shm_file(fd, size, invalid_fd)
}

/// `wl_shm_pool.resize`: never smaller (wlroots `pool_handle_resize`), and the file covers it.
pub(super) fn shm_resize(
    fd: impl AsFd,
    size: i32,
    current: i32,
    invalid_fd: u32,
    invalid_stride: u32,
) -> Result<(), Refusal> {
    if size < current {
        return Err(Refusal::new(
            invalid_stride,
            format!("wl_shm_pool.resize: shrinking the pool from {current} to {size}"),
        ));
    }
    shm_file(fd, size, invalid_fd)
}

/// A file the compositor can map: a regular file in memory, opened for reading and writing,
/// without a seal against writing, at least `size` bytes long.
///
/// mpv keeps the file and could seal it once checked, so that the compositor's mapping of it
/// fails: the relay first seals it against shrinking and against any new seal, then reads its
/// seals and its size, which can no longer change in a way that matters.
fn shm_file(fd: impl AsFd, size: i32, invalid_fd: u32) -> Result<(), Refusal> {
    let refuse = |why: &str| {
        Err(Refusal::new(
            invalid_fd,
            format!("a shared-memory pool's file {why}"),
        ))
    };
    let fd = fd.as_fd();
    let Ok(stat) = rustix::fs::fstat(fd) else {
        return refuse("cannot be read");
    };
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return refuse("is not a regular file");
    }
    if !file_system(fd).is_ok_and(shm_file_system_is_memory) {
        return refuse("is not in memory (memfd or tmpfs)");
    }
    let Ok(flags) = rustix::fs::fcntl_getfl(fd) else {
        return refuse("cannot be read");
    };
    if flags & OFlags::RWMODE != OFlags::RDWR {
        return refuse("is not open for reading and writing");
    }
    if !freeze_seals(fd) {
        return refuse("cannot be sealed");
    }
    let Ok(seals) = rustix::fs::fcntl_get_seals(fd) else {
        return refuse("cannot be read");
    };
    if !seals.contains(SealFlags::SEAL) {
        return refuse("cannot be sealed");
    }
    // A file that can still shrink, as a memfd made without `MFD_ALLOW_SEALING`, would let mpv cut
    // the pool under the compositor: wlroots reads zeroes past its end, but compositors built on
    // libwayland's `wl_shm` raise a protocol error, on the window's connection.
    if !seals.contains(SealFlags::SHRINK) {
        return refuse("may still shrink");
    }
    if seals.intersects(SealFlags::WRITE | SealFlags::FUTURE_WRITE) {
        return refuse("is sealed against writing");
    }
    // Read after the seals: a file sealed against shrinking keeps at least this size.
    let Ok(stat) = rustix::fs::fstat(fd) else {
        return refuse("cannot be read");
    };
    if stat.st_size < i64::from(size) {
        return refuse(&format!(
            "holds {} bytes, fewer than the pool's {size}",
            stat.st_size
        ));
    }
    Ok(())
}

/// Whether a file system, by its magic number, is one a pool may live on.
fn shm_file_system_is_memory(magic: i128) -> bool {
    magic == TMPFS_MAGIC
}

/// Seals a memory file against shrinking and against any new seal. Returns whether its seals can
/// no longer change: they could be added, or the file was sealed against new seals already, as a
/// memfd made without `MFD_ALLOW_SEALING` or a tmpfs file opened by its path are from the start.
/// The caller then refuses a file that is not sealed against shrinking: Mesa and mpv make their
/// pools with memfds that take seals.
fn freeze_seals(fd: BorrowedFd<'_>) -> bool {
    match rustix::fs::fcntl_add_seals(fd, SealFlags::SHRINK | SealFlags::SEAL) {
        Ok(()) => true,
        // Sealed against new seals already; the caller reads which seals it has.
        Err(rustix::io::Errno::PERM) => true,
        // Not a file that takes seals.
        Err(_) => false,
    }
}

/// The shared-memory formats the relay passes on, when the compositor offers them, with the bytes
/// a pixel takes: the formats Mesa's software drivers and mpv draw in. wlroots refuses a stride
/// that is not a whole number of pixels, or too short for the width (render/pixel_format.c,
/// `pixel_format_info_check_stride`), so the relay knows each format's size.
pub(super) const SHM_FORMATS: [(u32, i32); 11] = [
    (0, 4), // wl_shm's argb8888
    (1, 4), // wl_shm's xrgb8888
    (fourcc(*b"AB24"), 4),
    (fourcc(*b"XB24"), 4),
    (fourcc(*b"AR30"), 4),
    (fourcc(*b"XR30"), 4),
    (fourcc(*b"AB30"), 4),
    (fourcc(*b"XB30"), 4),
    (fourcc(*b"AB4H"), 8),
    (fourcc(*b"XB4H"), 8),
    (fourcc(*b"RG16"), 2),
];

/// A DRM format code from its four letters.
const fn fourcc(code: [u8; 4]) -> u32 {
    u32::from_le_bytes(code)
}

/// The bytes a pixel of `format` takes, if the relay knows the format.
pub(super) fn shm_pixel_size(format: u32) -> Option<i32> {
    SHM_FORMATS
        .iter()
        .find(|(known, _)| *known == format)
        .map(|(_, size)| *size)
}

/// A buffer of a shared-memory pool, as `wl_shm_pool.create_buffer` asks for it.
#[derive(Clone, Copy, Debug)]
pub(super) struct ShmBuffer {
    pub(super) offset: i32,
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) stride: i32,
    pub(super) format: u32,
}

/// The error codes of `wl_shm`, by name.
#[derive(Clone, Copy)]
pub(super) struct ShmErrors {
    pub(super) invalid_format: u32,
    pub(super) invalid_stride: u32,
}

/// `wl_shm_pool.create_buffer` (wlroots `pool_handle_create_buffer`): the buffer lies inside the
/// pool of `pool_size` bytes, its format is one the relay `offered`, its stride a whole number of
/// pixels and at least its width.
pub(super) fn shm_buffer(
    buffer: ShmBuffer,
    pool_size: i32,
    offered: &[u32],
    errors: ShmErrors,
) -> Result<(), Refusal> {
    let ShmBuffer {
        offset,
        width,
        height,
        stride,
        format,
    } = buffer;
    let outside = offset < 0
        || width <= 0
        || height <= 0
        || stride < width
        || i64::from(offset) + i64::from(stride) * i64::from(height) > i64::from(pool_size);
    if outside {
        return Err(Refusal::new(
            errors.invalid_stride,
            format!(
                "wl_shm_pool.create_buffer: {width}x{height}, stride {stride} at {offset} does not \
                 fit a pool of {pool_size} bytes"
            ),
        ));
    }
    let Some(pixel) = shm_pixel_size(format).filter(|_| offered.contains(&format)) else {
        return Err(Refusal::new(
            errors.invalid_format,
            format!("wl_shm_pool.create_buffer: format {format:#x} was not offered"),
        ));
    };
    if stride % pixel != 0 || i64::from(stride) < i64::from(width) * i64::from(pixel) {
        return Err(Refusal::new(
            errors.invalid_stride,
            format!(
                "wl_shm_pool.create_buffer: stride {stride} for {width} pixels of {pixel} bytes"
            ),
        ));
    }
    Ok(())
}

/// What a dmabuf plane's fd is: a dma-buf, and how many bytes it holds, when the kernel can tell.
/// The file system of every dma-buf since Linux 5.3 carries this magic number.
const DMA_BUF_MAGIC: i128 = 0x444d_4142;

/// The magic number of the file system `fd` lives on. The kernel's type for it is the platform's
/// `long`; widened, it compares the same everywhere.
fn file_system(fd: BorrowedFd<'_>) -> rustix::io::Result<i128> {
    rustix::fs::fstatfs(fd).map(|file_system| i128::from(file_system.f_type))
}

/// The size of a dma-buf, from `lseek(SEEK_END)`, as wlroots reads it; `None` when the kernel
/// cannot seek in it, which wlroots takes as unknown. Anything but a dma-buf is refused: the relay
/// passes nothing else to the compositor as one. `memory_stands_for_dmabuf` lets a memory file
/// pass too: only the tests and the fuzz target set it, as no machine without a GPU can make a
/// dma-buf.
pub(super) fn dmabuf_size(
    fd: impl AsFd,
    out_of_bounds: u32,
    memory_stands_for_dmabuf: bool,
) -> Result<Option<u64>, Refusal> {
    let fd = fd.as_fd();
    let kind = file_system(fd);
    let accepted =
        kind == Ok(DMA_BUF_MAGIC) || (memory_stands_for_dmabuf && kind == Ok(TMPFS_MAGIC));
    if !accepted {
        return Err(Refusal::new(
            out_of_bounds,
            "a dmabuf plane's fd is not a dma-buf",
        ));
    }
    Ok(rustix::fs::seek(fd, SeekFrom::End(0)).ok())
}

/// One plane given to `zwp_linux_buffer_params_v1.add`.
#[derive(Clone, Copy, Debug)]
pub(super) struct Plane {
    pub(super) offset: u32,
    pub(super) stride: u32,
    /// The dma-buf's size, when the kernel tells it.
    pub(super) size: Option<u64>,
}

/// The most planes a dmabuf has (wlroots `WLR_DMABUF_MAX_PLANES`).
pub(super) const MOST_PLANES: usize = 4;

/// The error codes of `zwp_linux_buffer_params_v1`, by name.
#[derive(Clone, Copy)]
pub(super) struct ParamsErrors {
    pub(super) plane_idx: u32,
    pub(super) plane_set: u32,
    pub(super) incomplete: u32,
    pub(super) invalid_format: u32,
    pub(super) invalid_dimensions: u32,
    pub(super) out_of_bounds: u32,
}

/// `zwp_linux_buffer_params_v1.add` (wlroots types/wlr_linux_dmabuf_v1.c, `params_add`): a plane
/// index below 4, not set yet, with the modifier of the planes before it.
pub(super) fn dmabuf_add(
    planes: &[Option<Plane>; MOST_PLANES],
    modifier: Option<u64>,
    index: u32,
    new_modifier: u64,
    errors: ParamsErrors,
) -> Result<usize, Refusal> {
    let Some(slot) = usize::try_from(index).ok().filter(|i| *i < MOST_PLANES) else {
        return Err(Refusal::new(
            errors.plane_idx,
            format!("plane index {index} is not below {MOST_PLANES}"),
        ));
    };
    if planes[slot].is_some() {
        return Err(Refusal::new(
            errors.plane_set,
            format!("plane {index} was set already"),
        ));
    }
    if modifier.is_some_and(|known| known != new_modifier) {
        return Err(Refusal::new(
            errors.invalid_format,
            format!("plane {index}'s modifier differs from the other planes'"),
        ));
    }
    Ok(slot)
}

/// The flags of `zwp_linux_buffer_params_v1.create` up to version 4: y_invert, interlaced,
/// bottom_first.
const KNOWN_FLAGS: u32 = 1 | 2 | 4;

/// `zwp_linux_buffer_params_v1.create` and `create_immed` (wlroots `params_create_common`):
/// planes from 0 without a gap, known flags, a size of at least one pixel, and each plane inside
/// its dma-buf.
pub(super) fn dmabuf_create(
    planes: &[Option<Plane>; MOST_PLANES],
    width: i32,
    height: i32,
    flags: u32,
    errors: ParamsErrors,
) -> Result<(), Refusal> {
    let count = planes.iter().take_while(|plane| plane.is_some()).count();
    if count == 0 {
        return Err(Refusal::new(errors.incomplete, "no plane 0 was added"));
    }
    if planes[count..].iter().any(Option::is_some) {
        return Err(Refusal::new(errors.incomplete, "the planes have a gap"));
    }
    if flags & !KNOWN_FLAGS != 0 {
        return Err(Refusal::new(
            errors.invalid_format,
            format!("unknown flags {flags:#x}"),
        ));
    }
    if width < 1 || height < 1 {
        return Err(Refusal::new(
            errors.invalid_dimensions,
            format!("invalid size {width}x{height}"),
        ));
    }
    let height = u64::try_from(height).unwrap_or_default();
    for (index, plane) in planes.iter().flatten().enumerate() {
        let (offset, stride) = (u64::from(plane.offset), u64::from(plane.stride));
        let end_of_first_row = offset + stride;
        let end = offset + stride * height;
        let out = |what: &str| {
            Err(Refusal::new(
                errors.out_of_bounds,
                format!("plane {index}: {what}"),
            ))
        };
        if end_of_first_row > u64::from(u32::MAX) || end > u64::from(u32::MAX) {
            return out("its size overflows");
        }
        let Some(size) = plane.size else { continue };
        if offset > size {
            return out("its offset lies past the dma-buf's end");
        }
        if end_of_first_row > size || stride == 0 {
            return out("its stride is zero or reaches past the dma-buf's end");
        }
        // Only the first plane's height is known: the others may be subsampled.
        if index == 0 && end > size {
            return out("its rows reach past the dma-buf's end");
        }
    }
    Ok(())
}

/// What one of mpv's bindings of `wp_color_manager_v1` was offered: the values its requests may
/// use. Each set holds a bit for each value of the protocol's enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ColourOffer {
    /// The version mpv bound.
    pub(super) version: u32,
    pub(super) intents: u32,
    pub(super) transfer_functions: u32,
    pub(super) primaries: u32,
    /// Whether the features `set_luminances` and `set_mastering_display_primaries` were offered;
    /// the second offers `set_mastering_luminance` too.
    pub(super) set_luminances: bool,
    pub(super) mastering: bool,
}

/// Whether a set of values, a bit for each, holds `value`. No value of the protocol's enums
/// reaches 32.
pub(super) fn holds(set: u32, value: u32) -> bool {
    value < 32 && set & (1 << value) != 0
}

/// A parametric image description as mpv sets it on its creator, until it creates it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Parametric {
    pub(super) transfer_function: Option<u32>,
    pub(super) primaries: Option<u32>,
    /// The primary colour volume's minimum in 1/10,000 cd/m², its maximum and reference white in
    /// cd/m², as the protocol carries them.
    pub(super) luminances: Option<(u32, u32, u32)>,
    /// Red, green, blue and white, x then y, each times 1,000,000.
    pub(super) mastering_primaries: Option<[i32; 8]>,
    /// The minimum in 1/10,000 cd/m², the maximum in cd/m².
    pub(super) mastering_luminance: Option<(u32, u32)>,
    pub(super) max_cll: Option<u32>,
    pub(super) max_fall: Option<u32>,
}

/// One of mpv's requests on a parametric creator, `create` apart, with its values.
#[derive(Clone, Copy, Debug)]
pub(super) enum ParametricSet {
    TransferFunction(u32),
    /// `set_tf_power`, never offered.
    TransferPower,
    Primaries(u32),
    /// `set_primaries`, never offered.
    PrimaryCoordinates,
    Luminances {
        min: u32,
        max: u32,
        reference: u32,
    },
    MasteringPrimaries([i32; 8]),
    MasteringLuminance {
        min: u32,
        max: u32,
    },
    MaxCll(u32),
    MaxFall(u32),
}

/// The error codes of `wp_image_description_creator_params_v1`, by name.
#[derive(Clone, Copy)]
pub(super) struct CreatorErrors {
    pub(super) incomplete_set: u32,
    pub(super) already_set: u32,
    pub(super) unsupported_feature: u32,
    pub(super) invalid_tf: u32,
    pub(super) invalid_primaries_named: u32,
    pub(super) invalid_luminance: u32,
}

/// The largest chromaticity coordinate, 1.0, times 1,000,000.
const MOST_COORDINATE: i32 = 1_000_000;

/// One request of mpv's on a parametric creator, `create` apart, checked against what its binding
/// was `offer`ed and what the creator holds, then kept in `description`. Each refusal is a
/// compositor's, quoted from wlroots 0.20.2's colour manager, Umbriel `d171ccb`'s
/// (`src/server/color_manager.cpp`) and KWin 6.8.2's (`src/wayland/colormanagement_v1.cpp`):
/// - any property set twice: `already_set` (wlroots "transfer function already set", and the
///   same for the primaries and the mastering display; KWin also for `set_max_cll` and
///   `set_max_fall`, "max_cll is already set", "max_fall is already set");
/// - a transfer function or named primaries not offered: `invalid_tf`, `invalid_primaries_named`
///   (wlroots "invalid transfer function", "invalid primaries"; KWin "unsupported named transfer
///   function" for `srgb`);
/// - `set_tf_power`, `set_primaries`, and `set_luminances` or the mastering display unless
///   offered: `unsupported_feature` (wlroots "set_tf_power is not supported", "set_primaries is
///   not supported", "set_luminances is not supported", and the mastering requests while the
///   feature is off);
/// - luminances whose maximum or reference is at or below the minimum: `invalid_luminance` (the
///   protocol's `set_luminances`: "If 'max_lum' or 'reference_lum' are less than or equal to
///   'min_lum', the protocol error invalid_luminance is raised"; Umbriel "maximum and reference
///   luminance must exceed minimum luminance");
/// - a mastering luminance whose maximum is at or below its minimum, which counts in 1/10,000
///   cd/m²: `invalid_luminance` (wlroots "max luminance must be greater than min luminance";
///   Umbriel "maximum mastering luminance must exceed minimum mastering luminance");
/// - mastering primaries outside 0.0 to 1.0, the range of a CIE 1931 chromaticity coordinate,
///   which the protocol carries times 1,000,000: no compositor read was found to refuse them, but
///   the relay sends none it cannot place; the protocol names no code, and
///   `invalid_primaries_named` is the closest.
///
/// Luminances are compared twice, and refused when either comparison refuses them ([`at_or_below`]):
/// exactly, in 64 bits, where no product of the protocol's 32-bit values overflows, and as wlroots
/// and Umbriel compare them, in `f32`.
pub(super) fn parametric_set(
    description: &mut Parametric,
    set: ParametricSet,
    offer: &ColourOffer,
    errors: CreatorErrors,
) -> Result<(), Refusal> {
    let refuse = |code: u32, message: String| Err(Refusal::new(code, message));
    let twice = |what: &str| {
        refuse(
            errors.already_set,
            format!("wp_image_description_creator_params_v1: {what} set twice"),
        )
    };
    let not_offered = |what: &str| {
        refuse(
            errors.unsupported_feature,
            format!("wp_image_description_creator_params_v1.{what} was not offered"),
        )
    };
    match set {
        ParametricSet::TransferFunction(tf) => {
            if description.transfer_function.is_some() {
                return twice("the transfer function");
            }
            if !holds(offer.transfer_functions, tf) {
                return refuse(
                    errors.invalid_tf,
                    format!("transfer function {tf} was not offered"),
                );
            }
            description.transfer_function = Some(tf);
        }
        ParametricSet::TransferPower => return not_offered("set_tf_power"),
        ParametricSet::Primaries(primaries) => {
            if description.primaries.is_some() {
                return twice("the primaries");
            }
            if !holds(offer.primaries, primaries) {
                return refuse(
                    errors.invalid_primaries_named,
                    format!("primaries {primaries} were not offered"),
                );
            }
            description.primaries = Some(primaries);
        }
        ParametricSet::PrimaryCoordinates => return not_offered("set_primaries"),
        ParametricSet::Luminances {
            min,
            max,
            reference,
        } => {
            if !offer.set_luminances {
                return not_offered("set_luminances");
            }
            if description.luminances.is_some() {
                return twice("the luminances");
            }
            if at_or_below(max, min) || at_or_below(reference, min) {
                return refuse(
                    errors.invalid_luminance,
                    format!(
                        "luminances: maximum {max} and reference {reference} cd/m² must lie \
                         above the minimum {min} × 0.0001 cd/m²"
                    ),
                );
            }
            description.luminances = Some((min, max, reference));
        }
        ParametricSet::MasteringPrimaries(coordinates) => {
            if !offer.mastering {
                return not_offered("set_mastering_display_primaries");
            }
            if description.mastering_primaries.is_some() {
                return twice("the mastering display's primaries");
            }
            if coordinates
                .iter()
                .any(|coordinate| !(0..=MOST_COORDINATE).contains(coordinate))
            {
                return refuse(
                    errors.invalid_primaries_named,
                    format!(
                        "the mastering display's primaries {coordinates:?} lie outside 0 to \
                         {MOST_COORDINATE}"
                    ),
                );
            }
            description.mastering_primaries = Some(coordinates);
        }
        ParametricSet::MasteringLuminance { min, max } => {
            if !offer.mastering {
                return not_offered("set_mastering_luminance");
            }
            if description.mastering_luminance.is_some() {
                return twice("the mastering luminance");
            }
            if at_or_below(max, min) {
                return refuse(
                    errors.invalid_luminance,
                    format!(
                        "mastering luminance: maximum {max} cd/m² must lie above the minimum \
                         {min} × 0.0001 cd/m²"
                    ),
                );
            }
            description.mastering_luminance = Some((min, max));
        }
        ParametricSet::MaxCll(max_cll) => {
            if description.max_cll.is_some() {
                return twice("max_cll");
            }
            description.max_cll = Some(max_cll);
        }
        ParametricSet::MaxFall(max_fall) => {
            if description.max_fall.is_some() {
                return twice("max_fall");
            }
            description.max_fall = Some(max_fall);
        }
    }
    Ok(())
}

/// Whether a luminance in cd/m² lies at or below a minimum in 1/10,000 cd/m², exactly or as the
/// compositors compute it. wlroots 0.20.2 (`types/wlr_color_management_v1.c`,
/// `image_desc_creator_params_handle_set_mastering_luminance`) and Umbriel `d171ccb`
/// (`src/server/color_manager.cpp`, `handleParametricSetLuminances` and
/// `handleParametricSetMasteringLuminance`) decode the minimum in `f32` and compare there:
/// `const float min = static_cast<float>(minimum) / 10000.0F;`
/// `if (static_cast<float>(maximum) <= min || …`. Rounded so, a minimum of 19,999,999 becomes
/// 2,000 cd/m², and a maximum of 2,000 is refused, though it lies above the exact minimum.
fn at_or_below(luminance: u32, minimum: u32) -> bool {
    let exactly = u64::from(luminance) * 10_000 <= u64::from(minimum);
    exactly || as_compositors_decode(luminance) <= decoded_minimum(minimum)
}

/// A luminance in cd/m², in `f32`, as C converts a `uint32_t` to `float`: to the nearest, ties to
/// even, which Rust's cast does too.
fn as_compositors_decode(luminance: u32) -> f32 {
    // The loss of precision is the point: it is the compositors' own.
    luminance as f32
}

/// A minimum in 1/10,000 cd/m², decoded as wlroots and Umbriel decode it: `(float)min / 10000`.
fn decoded_minimum(minimum: u32) -> f32 {
    as_compositors_decode(minimum) / 10_000.0
}

/// A parametric creator's `create`, for a creator of a binding at `version`, as wlroots 0.20.2's
/// colour manager refuses it:
/// - without a transfer function or primaries: `incomplete_set` ("missing transfer function",
///   "missing primaries");
/// - `max_fall` above `max_cll`, both above 0: `invalid_luminance` ("max_fall must be less or
///   equal to max_cll");
/// - at version 1 only, with a mastering luminance set, `max_cll` or `max_fall` above 0 and
///   outside its range, that is not above its minimum or above its maximum: `invalid_luminance`
///   (`if (version < 2 && (!check_mastering_luminance_range(…`, "max_cll must be greater than min
///   L of the mastering luminance range", "…less or equal to max L…"; the protocol's `create`: "In
///   version 1, these following conditions also result in the invalid_luminance protocol
///   error"). Compared exactly and in `f32`, as wlroots compares a `float` value with the decoded
///   range ([`outside_mastering_range`]).
///
/// wlroots is the one compositor with the version-1 rule, and checks nothing without a mastering
/// luminance or for a value of 0: `if (!data->has_mastering_luminance || value == 0) { return
/// true; }`. The relay refuses no more. A value of 0 is unknown (CTA-861-H), and mpv 0.42 sends 0
/// for a value out of that range at version 1 (`cm_metadata_sanitize`), expecting it to pass.
pub(super) fn parametric_create(
    description: &Parametric,
    version: u32,
    errors: CreatorErrors,
) -> Result<(), Refusal> {
    if description.transfer_function.is_none() {
        return Err(Refusal::new(
            errors.incomplete_set,
            "wp_image_description_creator_params_v1.create: no transfer function was set",
        ));
    }
    if description.primaries.is_none() {
        return Err(Refusal::new(
            errors.incomplete_set,
            "wp_image_description_creator_params_v1.create: no primaries were set",
        ));
    }
    let max_cll = description.max_cll.unwrap_or(0);
    let max_fall = description.max_fall.unwrap_or(0);
    if max_cll != 0 && max_fall != 0 && max_fall > max_cll {
        return Err(Refusal::new(
            errors.invalid_luminance,
            format!("max_fall {max_fall} lies above max_cll {max_cll}"),
        ));
    }
    if version < 2
        && let Some((min, max)) = description.mastering_luminance
    {
        for (name, value) in [("max_cll", max_cll), ("max_fall", max_fall)] {
            if value != 0 && outside_mastering_range(value, min, max) {
                return Err(Refusal::new(
                    errors.invalid_luminance,
                    format!(
                        "at version 1, {name} {value} cd/m² lies outside the mastering \
                         luminance range, {min} × 0.0001 to {max} cd/m²"
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Whether a light level in cd/m² lies outside a mastering range whose minimum counts in 1/10,000
/// cd/m² and whose maximum in cd/m²: not above the minimum, or above the maximum. Exactly, or as
/// wlroots computes it, in `f32` (`check_mastering_luminance_range`, with `float value`:
/// `if (value <= data->mastering_luminance.min)`, `if (value > data->mastering_luminance.max)`,
/// the range decoded as `(float)min_lum / 10000` and `max_lum`).
fn outside_mastering_range(value: u32, min: u32, max: u32) -> bool {
    let exactly = u64::from(value) * 10_000 <= u64::from(min) || value > max;
    let level = as_compositors_decode(value);
    exactly || level <= decoded_minimum(min) || level > as_compositors_decode(max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::fs::MemfdFlags;
    use std::os::fd::OwnedFd;

    fn memfd(size: u64) -> OwnedFd {
        let fd = rustix::fs::memfd_create(
            "relay-test",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )
        .unwrap();
        rustix::fs::ftruncate(&fd, size).unwrap();
        fd
    }

    #[test]
    fn a_viewport_takes_minus_one_to_unset_and_refuses_the_rest_out_of_range() {
        assert_eq!(viewport_source(-1.0, -1.0, -1.0, -1.0, 9), Ok(None));
        assert!(viewport_source(0.0, 0.0, 10.5, 2.0, 9).unwrap().is_some());
        for (x, y, w, h) in [
            (-1.0, 0.0, 1.0, 1.0),
            (0.0, -0.5, 1.0, 1.0),
            (0.0, 0.0, 0.0, 1.0),
            (0.0, 0.0, 1.0, -1.0),
            (-1.0, -1.0, -1.0, 2.0),
            (f64::NAN, 0.0, 1.0, 1.0),
        ] {
            assert_eq!(
                viewport_source(x, y, w, h, 9).unwrap_err().code,
                9,
                "{x} {y} {w} {h}"
            );
        }
        assert_eq!(viewport_destination(-1, -1, 9), Ok(None));
        assert_eq!(viewport_destination(1, 2, 9), Ok(Some((1, 2))));
        for (w, h) in [(0, 1), (1, 0), (-1, 5), (-2, -2)] {
            assert!(viewport_destination(w, h, 9).is_err(), "{w} {h}");
        }
    }

    #[test]
    fn a_scale_is_positive() {
        assert_eq!(buffer_scale(1, 3), Ok(1));
        assert_eq!(buffer_scale(0, 3).unwrap_err().code, 3);
        assert_eq!(buffer_scale(-2, 3).unwrap_err().code, 3);
    }

    #[test]
    fn a_commit_is_checked_as_wlroots_checks_it() {
        let base = CommitState {
            buffer: (640, 360),
            scale: 2,
            transposed: false,
            source: None,
            destination: None,
        };
        assert!(commit(base, 1, 2, 3).is_ok());
        let odd = CommitState {
            buffer: (641, 360),
            ..base
        };
        assert_eq!(commit(odd, 1, 2, 3).unwrap_err().code, 1);
        // A source lifts the divisibility; its size must then be whole without a destination.
        let source = Source {
            x: 0.0,
            y: 0.0,
            width: 100.5,
            height: 100.0,
        };
        let fractional = CommitState {
            buffer: (641, 360),
            source: Some(source),
            ..base
        };
        assert_eq!(commit(fractional, 1, 2, 3).unwrap_err().code, 2);
        let with_destination = CommitState {
            destination: Some((100, 100)),
            ..fractional
        };
        assert!(commit(with_destination, 1, 2, 3).is_ok());
        // A source past the buffer, at its scale and turned.
        let outside = CommitState {
            source: Some(Source {
                x: 300.0,
                y: 0.0,
                width: 30.0,
                height: 10.0,
            }),
            destination: Some((10, 10)),
            ..base
        };
        assert_eq!(commit(outside, 1, 2, 3).unwrap_err().code, 3);
        let turned = CommitState {
            transposed: true,
            source: Some(Source {
                x: 0.0,
                y: 0.0,
                width: 180.0,
                height: 320.0,
            }),
            ..outside
        };
        assert!(commit(turned, 1, 2, 3).is_ok());
    }

    #[test]
    fn a_pool_is_a_writable_memory_file_that_covers_its_size() {
        let fd = memfd(4096);
        assert!(shm_pool(&fd, 4096, 7, 8).is_ok());
        assert_eq!(
            shm_pool(&fd, 4097, 7, 8).unwrap_err().code,
            7,
            "larger than its file"
        );
        assert_eq!(shm_pool(&fd, 0, 7, 8).unwrap_err().code, 8);
        assert_eq!(shm_pool(&fd, -1, 7, 8).unwrap_err().code, 8);
        let sealed = memfd(4096);
        rustix::fs::fcntl_add_seals(&sealed, SealFlags::WRITE).unwrap();
        assert_eq!(shm_pool(&sealed, 4096, 7, 8).unwrap_err().code, 7);
        let (socket, _other) = std::os::unix::net::UnixStream::pair().unwrap();
        assert_eq!(shm_pool(&socket, 1, 7, 8).unwrap_err().code, 7, "a socket");
        let on_disk =
            std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
        assert_eq!(
            shm_pool(&on_disk, 1, 7, 8).unwrap_err().code,
            7,
            "a read-only file on disk"
        );
        assert!(shm_resize(&fd, 4096, 4096, 7, 8).is_ok());
        assert_eq!(
            shm_resize(&fd, 2048, 4096, 7, 8).unwrap_err().code,
            8,
            "shrinking"
        );
        assert_eq!(
            shm_resize(&fd, 8192, 4096, 7, 8).unwrap_err().code,
            7,
            "past the file"
        );
    }

    #[test]
    fn a_pool_cannot_gain_a_seal_or_shrink_once_checked() {
        let fd = memfd(4096);
        assert!(shm_pool(&fd, 4096, 7, 8).is_ok());
        // mpv still holds the file: it can neither seal it against writing now, which would make
        // the compositor's mapping fail, nor shrink it under the pool.
        assert_eq!(
            rustix::fs::fcntl_add_seals(&fd, SealFlags::FUTURE_WRITE),
            Err(rustix::io::Errno::PERM)
        );
        assert_eq!(
            rustix::fs::fcntl_add_seals(&fd, SealFlags::WRITE),
            Err(rustix::io::Errno::PERM)
        );
        assert!(rustix::fs::ftruncate(&fd, 1024).is_err());
        // Checked again, as at a resize, it still passes.
        assert!(shm_resize(&fd, 4096, 4096, 7, 8).is_ok());
    }

    #[test]
    fn a_pool_sealed_against_new_seals_passes_only_if_it_cannot_shrink() {
        // As memfd_create makes it without MFD_ALLOW_SEALING: sealed against new seals, but free
        // to shrink, which libwayland's wl_shm answers with a protocol error.
        let unsealable = rustix::fs::memfd_create("relay-test", MemfdFlags::CLOEXEC).unwrap();
        rustix::fs::ftruncate(&unsealable, 4096).unwrap();
        assert!(shm_pool(&unsealable, 4096, 7, 8).is_err());
        // As mpv makes its cursor's pool: sealed against shrinking and new seals.
        let frozen = memfd(4096);
        rustix::fs::fcntl_add_seals(&frozen, SealFlags::SHRINK | SealFlags::SEAL).unwrap();
        assert!(shm_pool(&frozen, 4096, 7, 8).is_ok());
    }

    #[test]
    fn a_huge_page_pool_is_refused() {
        assert!(
            !shm_file_system_is_memory(0x9584_58f6),
            "hugetlbfs's magic number"
        );
        assert!(shm_file_system_is_memory(TMPFS_MAGIC));
        // Where the kernel makes huge-page memfds, one is refused whole: the compositor's mapping
        // of it fails without huge pages reserved.
        if let Ok(huge) =
            rustix::fs::memfd_create("relay-test", MemfdFlags::CLOEXEC | MemfdFlags::HUGETLB)
        {
            assert_eq!(shm_pool(&huge, 1, 7, 8).unwrap_err().code, 7);
        }
    }

    #[test]
    fn a_shared_memory_buffer_fits_its_pool_and_its_format() {
        let offered = [0, 1];
        let errors = ShmErrors {
            invalid_format: 1,
            invalid_stride: 2,
        };
        let buffer = |offset, width, height, stride, format| ShmBuffer {
            offset,
            width,
            height,
            stride,
            format,
        };
        assert!(shm_buffer(buffer(0, 16, 16, 64, 1), 1024, &offered, errors).is_ok());
        for (offset, width, height, stride, format, code) in [
            (-4, 16, 16, 64, 1, 2),
            (0, 0, 16, 64, 1, 2),
            (0, 16, -1, 64, 1, 2),
            (4, 16, 16, 64, 1, 2),
            (0, 16, 16, 63, 1, 2),
            (0, 16, 16, 60, 1, 2),
            (0, i32::MAX, 2, i32::MAX, 1, 2),
            (0, 16, 16, 64, fourcc(*b"AB24"), 1),
            (0, 16, 16, 64, 0xdead, 1),
        ] {
            let refused = shm_buffer(
                buffer(offset, width, height, stride, format),
                1024,
                &offered,
                errors,
            );
            assert_eq!(
                refused.unwrap_err().code,
                code,
                "{offset} {width}x{height} {stride}"
            );
        }
    }

    const ERRORS: ParamsErrors = ParamsErrors {
        plane_idx: 1,
        plane_set: 2,
        incomplete: 3,
        invalid_format: 4,
        invalid_dimensions: 5,
        out_of_bounds: 6,
    };

    fn plane(offset: u32, stride: u32, size: Option<u64>) -> Option<Plane> {
        Some(Plane {
            offset,
            stride,
            size,
        })
    }

    #[test]
    fn a_dmabuf_plane_is_added_once_with_one_modifier() {
        let mut planes = [None; MOST_PLANES];
        assert_eq!(dmabuf_add(&planes, None, 0, 7, ERRORS), Ok(0));
        planes[0] = plane(0, 64, None);
        assert_eq!(
            dmabuf_add(&planes, Some(7), 0, 7, ERRORS).unwrap_err().code,
            2
        );
        assert_eq!(
            dmabuf_add(&planes, Some(7), 4, 7, ERRORS).unwrap_err().code,
            1
        );
        assert_eq!(
            dmabuf_add(&planes, Some(7), 1, 8, ERRORS).unwrap_err().code,
            4
        );
        assert_eq!(dmabuf_add(&planes, Some(7), 1, 7, ERRORS), Ok(1));
    }

    #[test]
    fn a_dmabuf_is_whole_and_inside_its_fds() {
        let one = [plane(0, 64, Some(64 * 16)), None, None, None];
        assert!(dmabuf_create(&one, 16, 16, 0, ERRORS).is_ok());
        assert_eq!(
            dmabuf_create(&[None; 4], 16, 16, 0, ERRORS)
                .unwrap_err()
                .code,
            3
        );
        let gap = [plane(0, 64, None), None, plane(0, 64, None), None];
        assert_eq!(dmabuf_create(&gap, 16, 16, 0, ERRORS).unwrap_err().code, 3);
        assert_eq!(dmabuf_create(&one, 16, 16, 8, ERRORS).unwrap_err().code, 4);
        assert!(
            dmabuf_create(&one, 16, 16, 1, ERRORS).is_ok(),
            "y_invert is known"
        );
        assert_eq!(dmabuf_create(&one, 0, 16, 0, ERRORS).unwrap_err().code, 5);
        assert_eq!(dmabuf_create(&one, 16, 17, 0, ERRORS).unwrap_err().code, 6);
        let overflow = [plane(u32::MAX, 2, None), None, None, None];
        assert_eq!(
            dmabuf_create(&overflow, 16, 16, 0, ERRORS)
                .unwrap_err()
                .code,
            6
        );
        let no_stride = [plane(0, 0, Some(64)), None, None, None];
        assert_eq!(
            dmabuf_create(&no_stride, 16, 16, 0, ERRORS)
                .unwrap_err()
                .code,
            6
        );
        // A size the kernel cannot tell skips the checks against it, as in wlroots.
        let unknown = [plane(0, 64, None), None, None, None];
        assert!(dmabuf_create(&unknown, 16, 1000, 0, ERRORS).is_ok());
    }

    #[test]
    fn only_a_dmabuf_passes_as_one() {
        let fd = memfd(4096);
        assert_eq!(dmabuf_size(&fd, 6, false).unwrap_err().code, 6);
        assert_eq!(
            dmabuf_size(&fd, 6, true),
            Ok(Some(4096)),
            "as the tests have it"
        );
        let (socket, _other) = std::os::unix::net::UnixStream::pair().unwrap();
        assert!(dmabuf_size(&socket, 6, true).is_err());
    }

    const CREATOR: CreatorErrors = CreatorErrors {
        incomplete_set: 10,
        already_set: 11,
        unsupported_feature: 12,
        invalid_tf: 13,
        invalid_primaries_named: 14,
        invalid_luminance: 15,
    };

    /// The transfer function PQ (`st2084_pq`).
    const ST2084_PQ: u32 = 11;

    /// gamma22 and PQ, sRGB and BT.2020 primaries, both features.
    fn offer(version: u32) -> ColourOffer {
        ColourOffer {
            version,
            intents: 1,
            transfer_functions: (1 << 2) | (1 << ST2084_PQ),
            primaries: (1 << 1) | (1 << 6),
            set_luminances: true,
            mastering: true,
        }
    }

    /// The code of the first refusal of `sets`, made in order on one creator, then of its
    /// creation, if all passed.
    fn refusal(sets: &[ParametricSet], offer: &ColourOffer) -> Option<u32> {
        let mut description = Parametric::default();
        for set in sets {
            if let Err(refusal) = parametric_set(&mut description, *set, offer, CREATOR) {
                return Some(refusal.code);
            }
        }
        parametric_create(&description, offer.version, CREATOR)
            .err()
            .map(|refusal| refusal.code)
    }

    use ParametricSet::{
        Luminances, MasteringLuminance, MasteringPrimaries, MaxCll, MaxFall, Primaries,
        TransferFunction,
    };

    const PQ: [ParametricSet; 2] = [TransferFunction(ST2084_PQ), Primaries(6)];

    #[test]
    fn mpvs_hdr_description_passes() {
        let mut sets = PQ.to_vec();
        sets.extend([
            Luminances {
                min: 0,
                max: 10_000,
                reference: 203,
            },
            MasteringPrimaries([
                708_000, 292_000, 170_000, 797_000, 131_000, 46_000, 312_700, 329_000,
            ]),
            MasteringLuminance { min: 50, max: 1000 },
            MaxCll(1000),
            MaxFall(0),
        ]);
        assert_eq!(refusal(&sets, &offer(2)), None);
        assert_eq!(
            refusal(&sets, &offer(1)),
            None,
            "within the mastering range"
        );
    }

    #[test]
    fn each_property_is_set_once() {
        for set in [
            TransferFunction(2),
            Primaries(1),
            Luminances {
                min: 0,
                max: 100,
                reference: 80,
            },
            MasteringPrimaries([0; 8]),
            MasteringLuminance { min: 0, max: 100 },
            MaxCll(100),
            MaxFall(100),
        ] {
            assert_eq!(refusal(&[set, set], &offer(2)), Some(11), "{set:?}");
        }
    }

    #[test]
    fn only_what_was_offered_is_set() {
        assert_eq!(refusal(&[TransferFunction(9)], &offer(2)), Some(13));
        assert_eq!(refusal(&[TransferFunction(0)], &offer(2)), Some(13));
        assert_eq!(refusal(&[TransferFunction(u32::MAX)], &offer(2)), Some(13));
        assert_eq!(refusal(&[Primaries(9)], &offer(2)), Some(14));
        assert_eq!(
            refusal(&[ParametricSet::TransferPower], &offer(2)),
            Some(12)
        );
        assert_eq!(
            refusal(&[ParametricSet::PrimaryCoordinates], &offer(2)),
            Some(12)
        );
        let bare = ColourOffer {
            set_luminances: false,
            mastering: false,
            ..offer(2)
        };
        let luminances = Luminances {
            min: 0,
            max: 100,
            reference: 80,
        };
        assert_eq!(refusal(&[luminances], &bare), Some(12));
        assert_eq!(refusal(&[MasteringPrimaries([0; 8])], &bare), Some(12));
        assert_eq!(
            refusal(&[MasteringLuminance { min: 0, max: 100 }], &bare),
            Some(12)
        );
    }

    #[test]
    fn luminances_lie_above_their_minimum_whatever_their_size() {
        let refused = |min, max, reference| {
            let mut sets = PQ.to_vec();
            sets.push(Luminances {
                min,
                max,
                reference,
            });
            refusal(&sets, &offer(2))
        };
        assert_eq!(refused(10_000, 1, 2), Some(15), "a maximum at the minimum");
        assert_eq!(
            refused(10_000, 2, 1),
            Some(15),
            "a reference at the minimum"
        );
        assert_eq!(refused(10_001, 2, 2), None);
        assert_eq!(refused(u32::MAX, u32::MAX, u32::MAX), None, "no overflow");
        assert_eq!(refused(u32::MAX, 429_496, 429_497), Some(15));
        let mastering = |min, max| {
            let mut sets = PQ.to_vec();
            sets.push(MasteringLuminance { min, max });
            refusal(&sets, &offer(2))
        };
        assert_eq!(mastering(0, 0), Some(15));
        assert_eq!(mastering(10_000, 1), Some(15));
        assert_eq!(mastering(9_999, 1), None);
        assert_eq!(mastering(u32::MAX, u32::MAX), None);
    }

    #[test]
    fn luminances_are_refused_where_the_compositors_rounding_refuses_them() {
        // 2,000 cd/m² lies above 1,999.9999 exactly, but 19,999,999 rounds to 20,000,000 in f32,
        // which wlroots and Umbriel then compare with 2,000.0.
        assert!(at_or_below(2_000, 19_999_999));
        assert!(!at_or_below(2_001, 19_999_999));
        let mut sets = PQ.to_vec();
        sets.push(Luminances {
            min: 19_999_999,
            max: 2_000,
            reference: 2_000,
        });
        assert_eq!(refusal(&sets, &offer(2)), Some(15));
        let mut sets = PQ.to_vec();
        sets.push(MasteringLuminance {
            min: 19_999_999,
            max: 2_000,
        });
        assert_eq!(refusal(&sets, &offer(2)), Some(15));
    }

    #[test]
    fn mastering_primaries_are_chromaticities() {
        let primaries = |coordinates| {
            let mut sets = PQ.to_vec();
            sets.push(MasteringPrimaries(coordinates));
            refusal(&sets, &offer(2))
        };
        assert_eq!(primaries([1_000_000; 8]), None);
        assert_eq!(primaries([0; 8]), None);
        let mut negative = [0; 8];
        negative[7] = -1;
        assert_eq!(primaries(negative), Some(14));
        assert_eq!(primaries([1_000_001; 8]), Some(14));
        assert_eq!(primaries([i32::MIN; 8]), Some(14));
    }

    #[test]
    fn a_creation_needs_its_transfer_function_and_primaries_and_a_max_fall_within_max_cll() {
        assert_eq!(refusal(&[Primaries(1)], &offer(2)), Some(10));
        assert_eq!(refusal(&[TransferFunction(2)], &offer(2)), Some(10));
        let with = |cll, fall| {
            let mut sets = PQ.to_vec();
            sets.extend([MaxCll(cll), MaxFall(fall)]);
            refusal(&sets, &offer(2))
        };
        assert_eq!(with(400, 401), Some(15));
        assert_eq!(with(400, 400), None);
        assert_eq!(with(0, 401), None, "an unknown max_cll bounds nothing");
        assert_eq!(with(400, 0), None);
    }

    #[test]
    fn at_version_1_max_cll_and_max_fall_lie_in_the_mastering_range() {
        let at = |version, sets: &[ParametricSet]| {
            let mut all = PQ.to_vec();
            all.extend_from_slice(sets);
            refusal(&all, &offer(version))
        };
        let mastering = MasteringLuminance { min: 50, max: 1000 };
        assert_eq!(at(1, &[mastering, MaxCll(1001)]), Some(15));
        assert_eq!(
            at(2, &[mastering, MaxCll(1001)]),
            None,
            "version 2 has no such rule"
        );
        assert_eq!(at(1, &[mastering, MaxFall(1001)]), Some(15));
        assert_eq!(at(1, &[mastering, MaxCll(1000), MaxFall(1)]), None);
        // A minimum of 1 cd/m²: a max_cll of 1 does not lie above it.
        let high_floor = MasteringLuminance {
            min: 10_000,
            max: 1000,
        };
        assert_eq!(at(1, &[high_floor, MaxCll(1)]), Some(15));
        assert_eq!(at(1, &[high_floor, MaxCll(0)]), None, "0 is unknown");
        // 2,000 lies above 1,999.9999 cd/m² exactly, not in wlroots' f32.
        let rounded_floor = MasteringLuminance {
            min: 19_999_999,
            max: 3000,
        };
        assert_eq!(at(1, &[rounded_floor, MaxCll(2001)]), None);
        assert_eq!(at(1, &[rounded_floor, MaxCll(2000)]), Some(15));
    }

    #[test]
    fn at_version_1_without_a_mastering_luminance_light_levels_are_free() {
        let luminances = Luminances {
            min: 10_000,
            max: 80,
            reference: 80,
        };
        for sets in [
            vec![MaxCll(u32::MAX)],
            vec![MaxCll(1), MaxFall(1)],
            vec![luminances, MaxCll(10_002), MaxFall(5)],
        ] {
            let mut all = PQ.to_vec();
            all.extend(sets);
            assert_eq!(refusal(&all, &offer(1)), None, "{all:?}");
        }
    }
}
