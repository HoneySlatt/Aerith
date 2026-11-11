// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The controls, drawn by wgpu through Vulkan as shapes (`shapes.wgsl`): no text, no image. Each
//! frame starts transparent; the surface's alpha is premultiplied, so that the compositor blends
//! it over the video.

use std::ffi::c_void;
use std::future::Future;
use std::pin::pin;
use std::ptr::NonNull;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use wgpu::{
    Backends, BlendState, BufferUsages, Color, ColorTargetState, ColorWrites, CompositeAlphaMode,
    CurrentSurfaceTexture, DeviceDescriptor, FragmentState, InstanceDescriptor, Limits, LoadOp,
    Operations, PresentMode, PrimitiveState, PrimitiveTopology, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipelineDescriptor, RequestAdapterOptions, StoreOp,
    SurfaceConfiguration, SurfaceTargetUnsafe, TextureFormat, TextureUsages, VertexAttribute,
    VertexBufferLayout, VertexFormat, VertexState, VertexStepMode,
};

use crate::layout::{Controls, PANEL_RADIUS, Rect};

/// A shape to draw, in the surface's pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub rect: Rect,
    /// Its colour, premultiplied by its alpha.
    pub colour: [f32; 4],
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// A rectangle with corners of this radius; a disc when the radius is half its side.
    Rounded(f32),
    /// A triangle pointing right, its base on the rectangle's left side.
    Triangle,
}

/// The most shapes a frame draws.
const MOST_SHAPES: usize = 8;
/// What the shader reads of a shape: three vectors of four floats.
const SHAPE_FLOATS: usize = 12;
const SHAPE_BYTES: u64 = (SHAPE_FLOATS * size_of::<f32>()) as u64;

/// A colour from 8-bit sRGB components and an opacity, premultiplied.
fn colour(red: u8, green: u8, blue: u8, alpha: f32) -> [f32; 4] {
    let channel = |value: u8| f32::from(value) / 255.0 * alpha;
    [channel(red), channel(green), channel(blue), alpha]
}

/// The controls' shapes at `scale` pixels to the logical pixel: the panel, the play/pause button
/// with the sign of what it does, and the seek bar with `progress` (0 to 1) played.
pub fn controls_shapes(controls: &Controls, scale: f32, paused: bool, progress: f32) -> Vec<Shape> {
    let panel = controls.panel.scaled(scale);
    let button = controls.button.scaled(scale);
    let bar = controls.bar.scaled(scale);
    let light = colour(245, 245, 250, 0.92);
    let dark = colour(20, 20, 28, 0.95);
    let mut shapes = vec![
        Shape {
            rect: panel,
            colour: colour(14, 14, 22, 0.55),
            kind: Kind::Rounded(PANEL_RADIUS * scale),
        },
        Shape {
            rect: button,
            colour: light,
            kind: Kind::Rounded(button.width() / 2.0),
        },
    ];
    // The sign, in the button's middle third and a half.
    let sign = Rect {
        left: button.left + button.width() * 0.34,
        top: button.top + button.height() * 0.3,
        right: button.right - button.width() * 0.26,
        bottom: button.bottom - button.height() * 0.3,
    };
    if paused {
        shapes.push(Shape {
            rect: sign,
            colour: dark,
            kind: Kind::Triangle,
        });
    } else {
        let stroke = sign.width() * 0.36;
        let left = sign.left - sign.width() * 0.06;
        let right = sign.right - sign.width() * 0.14;
        for (from, to) in [(left, left + stroke), (right - stroke, right)] {
            shapes.push(Shape {
                rect: Rect {
                    left: from,
                    right: to,
                    ..sign
                },
                colour: dark,
                kind: Kind::Rounded(stroke * 0.25),
            });
        }
    }
    let thickness = bar.height();
    let played = bar.left + bar.width() * progress.clamp(0.0, 1.0);
    let knob = 8.0 * scale;
    let middle = (bar.top + bar.bottom) / 2.0;
    shapes.extend([
        Shape {
            rect: bar,
            colour: colour(255, 255, 255, 0.28),
            kind: Kind::Rounded(thickness / 2.0),
        },
        Shape {
            rect: Rect {
                right: played,
                ..bar
            },
            colour: light,
            kind: Kind::Rounded(thickness / 2.0),
        },
        Shape {
            rect: Rect {
                left: played - knob,
                top: middle - knob,
                right: played + knob,
                bottom: middle + knob,
            },
            colour: light,
            kind: Kind::Rounded(knob),
        },
    ]);
    shapes
}

/// wgpu on one Wayland surface, and what draws the shapes.
pub struct Gpu {
    // Declared first, so that it goes before the device it was made with.
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    viewport: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    shapes: wgpu::Buffer,
    /// The adapter and the present mode, for the person running the testbed.
    pub description: String,
}

impl Gpu {
    /// wgpu on the Wayland `surface` of `display`, drawn at `size` pixels.
    ///
    /// # Safety
    ///
    /// `display` is a live `wl_display` and `surface` a live `wl_surface` on it, both alive until
    /// this value is dropped.
    pub unsafe fn new(
        display: NonNull<c_void>,
        surface: NonNull<c_void>,
        size: (u32, u32),
    ) -> Result<Self, String> {
        let instance = wgpu::Instance::new(InstanceDescriptor {
            backends: Backends::VULKAN,
            ..InstanceDescriptor::new_without_display_handle()
        });
        let target = SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
                display,
            ))),
            raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(surface)),
        };
        // SAFETY: the caller keeps the display and the surface alive while this value, which owns
        // the wgpu surface, lives.
        let surface = unsafe { instance.create_surface_unsafe(target) }
            .map_err(|error| format!("wgpu's surface: {error}"))?;
        let adapter = block_on(instance.request_adapter(&RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|error| format!("no GPU that draws on this surface through Vulkan: {error}"))?;
        let (device, queue) = block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("aerith-testbed"),
            // The lowest limits that draw on a surface as large as the GPU allows.
            required_limits: Limits::downlevel_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .map_err(|error| format!("wgpu's device: {error}"))?;

        let capabilities = surface.get_capabilities(&adapter);
        if !capabilities
            .alpha_modes
            .contains(&CompositeAlphaMode::PreMultiplied)
        {
            return Err(format!(
                "the surface cannot be translucent: it offers {:?}",
                capabilities.alpha_modes
            ));
        }
        // Colours are blended as they are written, in sRGB, as most interfaces do.
        let format = [TextureFormat::Bgra8Unorm, TextureFormat::Rgba8Unorm]
            .into_iter()
            .find(|format| capabilities.formats.contains(format))
            .ok_or_else(|| format!("no 8-bit surface format in {:?}", capabilities.formats))?;
        // The testbed paces itself on frame callbacks; a mode that waits on them too would block
        // the window's thread, and forever on a surface that shows nothing.
        let present_mode = [PresentMode::Mailbox, PresentMode::Immediate]
            .into_iter()
            .find(|mode| capabilities.present_modes.contains(mode))
            .unwrap_or(PresentMode::Fifo);
        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: Default::default(),
            width: size.0.max(1),
            height: size.1.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: CompositeAlphaMode::PreMultiplied,
            view_formats: Vec::new(),
        };
        surface.configure(&device, &config);

        let module = device.create_shader_module(wgpu::include_wgsl!("shapes.wgsl"));
        let viewport = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport"),
            size: 16,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shapes = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shapes"),
            size: SHAPE_BYTES * MOST_SHAPES as u64,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: viewport.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shapes"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = [
            (0, 0),
            (1, 4 * size_of::<f32>() as u64),
            (2, 8 * size_of::<f32>() as u64),
        ]
        .map(|(location, offset)| VertexAttribute {
            format: VertexFormat::Float32x4,
            offset,
            shader_location: location,
        });
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("shapes"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &module,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[Some(VertexBufferLayout {
                    array_stride: SHAPE_BYTES,
                    step_mode: VertexStepMode::Instance,
                    attributes: &attributes,
                })],
            },
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(FragmentState {
                module: &module,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(ColorTargetState {
                    format,
                    blend: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let info = adapter.get_info();
        let gpu = Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            viewport,
            bind_group,
            shapes,
            description: format!(
                "{} ({:?}, {:?}), {format:?}, {present_mode:?}",
                info.name, info.backend, info.device_type
            ),
        };
        gpu.write_viewport();
        Ok(gpu)
    }

    /// Draws at `size` pixels from the next frame on.
    pub fn resize(&mut self, size: (u32, u32)) {
        let size = (size.0.max(1), size.1.max(1));
        if (self.config.width, self.config.height) == size {
            return;
        }
        (self.config.width, self.config.height) = size;
        self.surface.configure(&self.device, &self.config);
        self.write_viewport();
    }

    fn write_viewport(&self) {
        let size = [
            self.config.width as f32,
            self.config.height as f32,
            0.0,
            0.0,
        ];
        self.queue
            .write_buffer(&self.viewport, 0, &floats_as_bytes(&size));
    }

    /// Draws `shapes` (the first [`MOST_SHAPES`]) on a transparent frame and presents it. Returns
    /// whether a frame was presented: none is while the surface is being made again, and the
    /// caller draws later.
    pub fn draw(&mut self, shapes: &[Shape]) -> Result<bool, String> {
        let (frame, suboptimal) = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) => (frame, false),
            // Presented as it is; the surface is made again once it is, as wgpu refuses to while a
            // frame of it lives.
            CurrentSurfaceTexture::Suboptimal(frame) => (frame, true),
            CurrentSurfaceTexture::Timeout | CurrentSurfaceTexture::Occluded => return Ok(false),
            CurrentSurfaceTexture::Outdated | CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return Ok(false);
            }
            CurrentSurfaceTexture::Validation => {
                return Err("wgpu refused the surface's next frame".to_owned());
            }
        };
        let shapes = &shapes[..shapes.len().min(MOST_SHAPES)];
        let floats: Vec<f32> = shapes.iter().flat_map(shape_floats).collect();
        self.queue
            .write_buffer(&self.shapes, 0, &floats_as_bytes(&floats));
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("controls"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color::TRANSPARENT),
                        store: StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            if !shapes.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.shapes.slice(..));
                pass.draw(0..4, 0..shapes.len() as u32);
            }
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        if suboptimal {
            self.surface.configure(&self.device, &self.config);
        }
        Ok(true)
    }
}

/// What the shader reads of a shape (`shapes.wgsl`).
fn shape_floats(shape: &Shape) -> [f32; SHAPE_FLOATS] {
    let Rect {
        left,
        top,
        right,
        bottom,
    } = shape.rect;
    let (radius, kind) = match shape.kind {
        Kind::Rounded(radius) => (radius, 0.0),
        Kind::Triangle => (0.0, 1.0),
    };
    let [red, green, blue, alpha] = shape.colour;
    [
        left, top, right, bottom, red, green, blue, alpha, radius, kind, 0.0, 0.0,
    ]
}

fn floats_as_bytes(floats: &[f32]) -> Vec<u8> {
    floats
        .iter()
        .flat_map(|float| float.to_ne_bytes())
        .collect()
}

/// Runs a future to its end on this thread. wgpu's on Vulkan are ready at once; this waits all the
/// same if one is not.
fn block_on<F: Future>(future: F) -> F::Output {
    struct Unpark(Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => thread::park(),
        }
    }
}
