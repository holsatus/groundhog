// A video frame's YUV planes, drawn as a quad letterboxed in the widget's
// bounds and converted to RGB here, on the GPU.

struct Uniforms {
    // rgb = matrix * (yuv - offset), one row per channel.
    row_r: vec4<f32>,
    row_g: vec4<f32>,
    row_b: vec4<f32>,
    offset: vec4<f32>,
    // xy: the quad's scale in clip space (letterboxing); z: 0 for NV12
    // (interleaved chroma in `chroma_u`), 1 for I420 (separate planes).
    shape: vec4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var samples: sampler;
@group(0) @binding(2) var luma: texture_2d<f32>;
@group(0) @binding(3) var chroma_u: texture_2d<f32>;
@group(0) @binding(4) var chroma_v: texture_2d<f32>;

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
    out.position = vec4<f32>(
        (x * 2.0 - 1.0) * uniforms.shape.x,
        (1.0 - y * 2.0) * uniforms.shape.y,
        0.0,
        1.0,
    );
    out.coordinates = vec2<f32>(x, y);
    return out;
}

@fragment
fn fs_main(in: Varyings) -> @location(0) vec4<f32> {
    let y = textureSample(luma, samples, in.coordinates).r;
    let u_sample = textureSample(chroma_u, samples, in.coordinates);
    let v_sample = textureSample(chroma_v, samples, in.coordinates);

    var chroma = u_sample.rg;
    if uniforms.shape.z > 0.5 {
        chroma = vec2<f32>(u_sample.r, v_sample.r);
    }

    let yuv = vec3<f32>(y, chroma) - uniforms.offset.xyz;
    let rgb = vec3<f32>(
        dot(uniforms.row_r.xyz, yuv),
        dot(uniforms.row_g.xyz, yuv),
        dot(uniforms.row_b.xyz, yuv),
    );
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
