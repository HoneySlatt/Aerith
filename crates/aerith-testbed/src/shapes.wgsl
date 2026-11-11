// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

// The controls' shapes, one instance each: a rounded rectangle, or a triangle pointing right (the
// play sign). Each fragment's coverage comes from its distance to the shape's edge, which smooths
// the edge over one pixel. Colours are premultiplied, as the surface is.

struct Viewport {
    // The surface's size, in pixels.
    size: vec2<f32>,
    unused: vec2<f32>,
};

@group(0) @binding(0) var<uniform> viewport: Viewport;

struct Shape {
    // Left, top, right, bottom, in pixels.
    @location(0) rect: vec4<f32>,
    @location(1) colour: vec4<f32>,
    // The corners' radius in pixels, then the kind: 0 a rounded rectangle, 1 a triangle.
    @location(2) params: vec4<f32>,
};

struct Fragment {
    @builtin(position) position: vec4<f32>,
    @location(0) pixel: vec2<f32>,
    @location(1) @interpolate(flat) rect: vec4<f32>,
    @location(2) @interpolate(flat) colour: vec4<f32>,
    @location(3) @interpolate(flat) params: vec4<f32>,
};

// Four vertices, a triangle strip: the shape's rectangle grown by a pixel, room for the smoothed
// edge.
@vertex
fn vertex(@builtin(vertex_index) index: u32, shape: Shape) -> Fragment {
    let corner = vec2<f32>(f32(index & 1u), f32(index >> 1u));
    let pixel = mix(shape.rect.xy - vec2<f32>(1.0), shape.rect.zw + vec2<f32>(1.0), corner);
    let clip = vec2<f32>(
        pixel.x / viewport.size.x * 2.0 - 1.0,
        1.0 - pixel.y / viewport.size.y * 2.0,
    );
    var out: Fragment;
    out.position = vec4<f32>(clip, 0.0, 1.0);
    out.pixel = pixel;
    out.rect = shape.rect;
    out.colour = shape.colour;
    out.params = shape.params;
    return out;
}

// The signed distance from `p` to a rectangle with rounded corners: negative inside.
fn rounded_rectangle(p: vec2<f32>, rect: vec4<f32>, radius: f32) -> f32 {
    let centre = (rect.xy + rect.zw) * 0.5;
    let half = (rect.zw - rect.xy) * 0.5;
    let r = min(radius, min(half.x, half.y));
    let q = abs(p - centre) - half + vec2<f32>(r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

// The distance from `p` to the line through `a` and `b`, positive on its right, looking from `a`
// to `b` with y pointing down: outside a triangle whose corners turn clockwise on screen.
fn side(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let along = b - a;
    return dot(p - a, normalize(vec2<f32>(along.y, -along.x)));
}

// The triangle with its base on the rectangle's left side and its tip at the middle of its right
// side: exact inside and along the sides, a little short of the true distance past the corners.
fn triangle(p: vec2<f32>, rect: vec4<f32>) -> f32 {
    let top = rect.xy;
    let tip = vec2<f32>(rect.z, (rect.y + rect.w) * 0.5);
    let bottom = rect.xw;
    return max(max(side(p, top, tip), side(p, tip, bottom)), side(p, bottom, top));
}

@fragment
fn fragment(in: Fragment) -> @location(0) vec4<f32> {
    var distance: f32;
    if in.params.y > 0.5 {
        distance = triangle(in.pixel, in.rect);
    } else {
        distance = rounded_rectangle(in.pixel, in.rect, in.params.x);
    }
    return in.colour * clamp(0.5 - distance, 0.0, 1.0);
}
