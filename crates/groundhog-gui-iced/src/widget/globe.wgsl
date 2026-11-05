// The globe's newest frame, stretched over the widget's bounds. The frame
// is rendered at the widget's size, so it is stretched only for the frame or
// two after a resize.

@group(0) @binding(0) var samples: sampler;
@group(0) @binding(1) var picture: texture_2d<f32>;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) coordinates: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> Varyings {
    // A triangle strip: top-left, top-right, bottom-left, bottom-right.
    let x = f32(index & 1u);
    let y = f32((index >> 1u) & 1u);

    var out: Varyings;
    out.position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.coordinates = vec2<f32>(x, y);
    return out;
}

@fragment
fn fs_main(in: Varyings) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(picture, samples, in.coordinates).rgb, 1.0);
}
