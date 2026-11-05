//! A video stream's newest frame, drawn on the GPU.
//!
//! The widget reads its stream's [`FrameSlot`] each time the view is built,
//! so the application only has to rebuild the view when a frame arrives.
//! Its pipeline keeps a set of textures per stream, uploads a frame's planes
//! only when the frame is new, recreates the textures only when the frame's
//! size or layout changes, and converts YUV to RGB in the fragment shader
//! (`video.wgsl`): no colour conversion on the CPU, and a third of an RGBA
//! frame's upload for NV12. The frame is letterboxed: it keeps its aspect
//! ratio, centred in the widget, and the widget's background shows around
//! it.
//!
//! Textures of a stream not drawn in a frame are freed at its end.

use std::{collections::HashMap, sync::Arc};

use groundhog_video::{Colour, Frame, FrameSlot, Matrix, PixelLayout, Plane, Range};
use iced::{
    Length, Point, Rectangle, Size, Widget, mouse, wgpu,
    widget::shader::{self, Viewport},
};

/// The newest frame of `slot`, filling the space it is given.
pub fn video<M>(slot: &FrameSlot) -> impl Widget<M> + use<M> {
    iced::widget::Shader::new(Video { slot: slot.clone() })
        .width(Length::Fill)
        .height(Length::Fill)
}

struct Video {
    slot: FrameSlot,
}

impl<M> shader::Program<M> for Video {
    type State = ();
    type Primitive = Primitive;

    fn draw(&self, _state: &(), _cursor: mouse::Cursor, _bounds: Rectangle) -> Primitive {
        Primitive {
            stream: self.slot.id(),
            frame: self.slot.latest(),
        }
    }
}

/// A stream's frame to draw, with its generation.
#[derive(Debug)]
pub struct Primitive {
    stream: u64,
    frame: Option<(u64, Arc<Frame>)>,
}

impl shader::Primitive for Primitive {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        _viewport: &Viewport,
    ) {
        let Some((generation, frame)) = &self.frame else {
            return;
        };

        let Pipeline {
            layout,
            sampler,
            blank,
            streams,
            ..
        } = pipeline;
        let stream = streams
            .entry(self.stream)
            .or_insert_with(|| Stream::new(device));
        stream.used = true;

        // A texture larger than the device allows fails wgpu's validation,
        // which panics. The last frame that fit stays on screen.
        if !fits_device(frame, device.limits().max_texture_dimension_2d) {
            return;
        }

        if stream.generation != Some(*generation) {
            let fits = stream
                .textures
                .as_ref()
                .is_some_and(|textures| textures.fit(frame));
            if !fits {
                stream.textures = Some(Textures::new(
                    device,
                    layout,
                    sampler,
                    blank,
                    &stream.uniforms,
                    frame,
                ));
            }
            if let Some(textures) = &stream.textures {
                textures.upload(queue, frame);
            }
            stream.generation = Some(*generation);
        }

        queue.write_buffer(&stream.uniforms, 0, &uniforms(frame, *bounds));
    }

    fn draw(&self, pipeline: &Pipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        if self.frame.is_none() {
            return true;
        }
        let Some(textures) = pipeline
            .streams
            .get(&self.stream)
            .and_then(|stream| stream.textures.as_ref())
        else {
            return true;
        };

        render_pass.set_pipeline(&pipeline.pipeline);
        render_pass.set_bind_group(0, &textures.bind_group, &[]);
        render_pass.draw(0..4, 0..1);
        true
    }
}

/// The video pipeline, shared by every video on screen, and the textures
/// of each stream.
pub struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Bound in place of the third plane an NV12 frame does not have.
    blank: wgpu::TextureView,
    streams: HashMap<u64, Stream>,
}

impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("video bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                texture(2),
                texture(3),
                texture(4),
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("video pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("video shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("video.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("video pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("video sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let blank = plane_texture(device, 1, 1, wgpu::TextureFormat::R8Unorm)
            .create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            pipeline,
            layout,
            sampler,
            blank,
            streams: HashMap::new(),
        }
    }

    /// Free the textures of the streams not drawn in this frame.
    fn trim(&mut self) {
        self.streams.retain(|_, stream| stream.used);
        for stream in self.streams.values_mut() {
            stream.used = false;
        }
    }
}

/// The GPU side of one stream.
struct Stream {
    uniforms: wgpu::Buffer,
    textures: Option<Textures>,
    /// The generation of the frame in the textures.
    generation: Option<u64>,
    /// Whether the stream was drawn since the last trim.
    used: bool,
}

impl Stream {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            uniforms: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("video uniforms"),
                size: UNIFORMS_SIZE,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            textures: None,
            generation: None,
            used: true,
        }
    }
}

/// A texture per plane, at one frame size and layout.
struct Textures {
    width: u32,
    height: u32,
    layout: PixelLayout,
    planes: Vec<wgpu::Texture>,
    bind_group: wgpu::BindGroup,
}

impl Textures {
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        blank: &wgpu::TextureView,
        uniforms: &wgpu::Buffer,
        frame: &Frame,
    ) -> Self {
        let (chroma_width, chroma_height) = frame.chroma_size();
        let planes = match frame.layout {
            PixelLayout::Nv12 => vec![
                plane_texture(
                    device,
                    frame.width,
                    frame.height,
                    wgpu::TextureFormat::R8Unorm,
                ),
                plane_texture(
                    device,
                    chroma_width,
                    chroma_height,
                    wgpu::TextureFormat::Rg8Unorm,
                ),
            ],
            PixelLayout::I420 => vec![
                plane_texture(
                    device,
                    frame.width,
                    frame.height,
                    wgpu::TextureFormat::R8Unorm,
                ),
                plane_texture(
                    device,
                    chroma_width,
                    chroma_height,
                    wgpu::TextureFormat::R8Unorm,
                ),
                plane_texture(
                    device,
                    chroma_width,
                    chroma_height,
                    wgpu::TextureFormat::R8Unorm,
                ),
            ],
        };
        let views: Vec<wgpu::TextureView> = planes
            .iter()
            .map(|plane| plane.create_view(&wgpu::TextureViewDescriptor::default()))
            .collect();
        let view = |index: usize| views.get(index).unwrap_or(blank);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("video bind group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(view(0)),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(view(1)),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(view(2)),
                },
            ],
        });

        Self {
            width: frame.width,
            height: frame.height,
            layout: frame.layout,
            planes,
            bind_group,
        }
    }

    /// Whether `frame` goes in these textures as they are.
    fn fit(&self, frame: &Frame) -> bool {
        self.width == frame.width && self.height == frame.height && self.layout == frame.layout
    }

    fn upload(&self, queue: &wgpu::Queue, frame: &Frame) {
        for (texture, plane) in self.planes.iter().zip(&frame.planes) {
            let size = texture.size();
            // A plane that does not fill its texture is a broken frame; skip
            // it rather than read past it, or fail wgpu's validation.
            let bytes_per_texel = texture.format().block_copy_size(None).unwrap_or(1);
            if !plane_fills(plane, size.width, size.height, bytes_per_texel) {
                continue;
            }

            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &plane.data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(plane.stride),
                    rows_per_image: Some(plane.rows),
                },
                size,
            );
        }
    }
}

/// Whether every plane of `frame` fits in a texture at most `max_dimension`
/// texels wide and tall. The chroma planes are no larger than the luma.
fn fits_device(frame: &Frame, max_dimension: u32) -> bool {
    frame.width <= max_dimension && frame.height <= max_dimension
}

/// Whether `plane` fills a texture `width` by `height` texels of
/// `bytes_per_texel`: its rows hold a row of texels, there are as many as
/// the texture is tall, and its data holds all of them.
fn plane_fills(plane: &Plane, width: u32, height: u32, bytes_per_texel: u32) -> bool {
    let Some(row) = width.checked_mul(bytes_per_texel) else {
        return false;
    };
    // At most (2^32 - 1)^2 + 2^32 - 1, which a u64 holds.
    let needed = u64::from(plane.stride) * u64::from(height.saturating_sub(1)) + u64::from(row);
    plane.stride >= row
        && plane.rows >= height
        && u64::try_from(plane.data.len()).is_ok_and(|length| length >= needed)
}

fn plane_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("video plane"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// Five vectors of four floats.
const UNIFORMS_SIZE: u64 = 5 * 4 * 4;

/// The shader's uniforms for `frame` in `bounds`: its colour conversion,
/// and the quad's scale that letterboxes it.
fn uniforms(frame: &Frame, bounds: Rectangle) -> Vec<u8> {
    let (rows, offset) = conversion(frame.colour);
    let (scale_x, scale_y) = letterbox(frame.width, frame.height, bounds.width, bounds.height);
    let i420 = match frame.layout {
        PixelLayout::Nv12 => 0.0,
        PixelLayout::I420 => 1.0,
    };

    let vectors = [
        [rows[0][0], rows[0][1], rows[0][2], 0.0],
        [rows[1][0], rows[1][1], rows[1][2], 0.0],
        [rows[2][0], rows[2][1], rows[2][2], 0.0],
        [offset[0], offset[1], offset[2], 0.0],
        [scale_x, scale_y, i420, 0.0],
    ];
    vectors
        .iter()
        .flatten()
        .flat_map(|value: &f32| value.to_le_bytes())
        .collect()
}

/// The matrix rows and offset that turn sampled YUV (each 0 to 1) into RGB:
/// `rgb = rows * (yuv - offset)`.
fn conversion(colour: Colour) -> ([[f32; 3]; 3], [f32; 3]) {
    // The matrix's chroma weights, for full-range components.
    let (red_v, green_u, green_v, blue_u) = match colour.matrix {
        Matrix::Bt601 => (1.402, -0.344_136, -0.714_136, 1.772),
        Matrix::Bt709 => (1.5748, -0.187_324, -0.468_124, 1.8556),
    };
    // Limited range spreads luma's 16–235 and chroma's 16–240 over 0–1.
    let (luma_gain, chroma_gain, luma_offset) = match colour.range {
        Range::Limited => (255.0 / 219.0, 255.0 / 224.0, 16.0 / 255.0),
        Range::Full => (1.0, 1.0, 0.0),
    };
    let chroma_offset = 128.0 / 255.0;

    (
        [
            [luma_gain, 0.0, red_v * chroma_gain],
            [luma_gain, green_u * chroma_gain, green_v * chroma_gain],
            [luma_gain, blue_u * chroma_gain, 0.0],
        ],
        [luma_offset, chroma_offset, chroma_offset],
    )
}

/// Where a `width` by `height` frame shows in a video widget of `size`,
/// relative to the widget: centred, keeping its aspect ratio, as the widget
/// draws it.
pub fn picture_bounds(width: u32, height: u32, size: Size) -> Rectangle {
    let (scale_x, scale_y) = letterbox(width, height, size.width, size.height);
    let picture = Size::new(size.width * scale_x, size.height * scale_y);
    Rectangle::new(
        Point::new(
            (size.width - picture.width) / 2.0,
            (size.height - picture.height) / 2.0,
        ),
        picture,
    )
}

/// The quad's scale in clip space (1 fills that axis) that keeps a
/// `width` by `height` frame's aspect ratio in a `bounds_width` by
/// `bounds_height` widget.
fn letterbox(width: u32, height: u32, bounds_width: f32, bounds_height: f32) -> (f32, f32) {
    if width == 0 || height == 0 || bounds_width <= 0.0 || bounds_height <= 0.0 {
        return (1.0, 1.0);
    }
    let frame = width as f32 / height as f32;
    let bounds = bounds_width / bounds_height;
    if frame > bounds {
        (1.0, bounds / frame)
    } else {
        (frame / bounds, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_is_centred_between_its_bands() {
        let picture = picture_bounds(1920, 1080, Size::new(400.0, 400.0));
        assert_eq!(picture.x, 0.0);
        assert_eq!(picture.width, 400.0);
        assert!((picture.height - 225.0).abs() < 1e-3);
        assert!((picture.y - 87.5).abs() < 1e-3);
    }

    #[test]
    fn a_frame_keeps_its_aspect_ratio() {
        // 16:9 in a square: full width, bands above and below.
        let (x, y) = letterbox(1920, 1080, 400.0, 400.0);
        assert_eq!(x, 1.0);
        assert!((y - 9.0 / 16.0).abs() < 1e-6);

        // 4:3 in a wide strip: full height, bands at the sides.
        let (x, y) = letterbox(640, 480, 1600.0, 400.0);
        assert!((x - (4.0 / 3.0) / 4.0).abs() < 1e-6);
        assert_eq!(y, 1.0);
    }

    fn frame(width: u32, height: u32) -> Frame {
        Frame {
            width,
            height,
            layout: PixelLayout::Nv12,
            planes: Vec::new(),
            colour: Colour {
                matrix: Matrix::Bt709,
                range: Range::Limited,
            },
        }
    }

    /// wgpu panics on a texture larger than the device allows, so a frame
    /// that would need one is not drawn.
    #[test]
    fn a_frame_larger_than_the_device_allows_does_not_fit() {
        assert!(fits_device(&frame(8192, 8192), 8192));
        assert!(!fits_device(&frame(8193, 1080), 8192));
        assert!(!fits_device(&frame(1920, 8193), 8192));
        assert!(fits_device(&frame(0, 0), 8192), "sized up to one texel");
    }

    fn plane(stride: u32, rows: u32, length: usize) -> Plane {
        Plane {
            data: vec![0; length],
            stride,
            rows,
        }
    }

    /// wgpu panics on a copy whose rows are shorter than the texture's, or
    /// fewer than its height, so only a plane that fills its texture is
    /// uploaded.
    #[test]
    fn only_a_plane_that_fills_its_texture_is_uploaded() {
        assert!(plane_fills(&plane(64, 4, 256), 64, 4, 1));
        assert!(plane_fills(&plane(64, 4, 64 * 3 + 32), 32, 4, 1));
        assert!(
            plane_fills(&plane(128, 2, 256), 64, 2, 2),
            "two bytes a texel"
        );

        assert!(!plane_fills(&plane(64, 4, 255), 64, 4, 1), "data short");
        assert!(!plane_fills(&plane(32, 8, 256), 64, 4, 1), "rows short");
        assert!(!plane_fills(&plane(64, 3, 256), 64, 4, 1), "too few rows");
        assert!(!plane_fills(&plane(0, 4, 256), 64, 4, 1), "no stride");
        assert!(
            !plane_fills(&plane(u32::MAX, u32::MAX, 256), u32::MAX, u32::MAX, 4),
            "sizes that overflow"
        );
    }

    fn rgb(colour: Colour, yuv: [f32; 3]) -> [f32; 3] {
        let (rows, offset) = conversion(colour);
        let centred = [yuv[0] - offset[0], yuv[1] - offset[1], yuv[2] - offset[2]];
        rows.map(|row| row[0] * centred[0] + row[1] * centred[1] + row[2] * centred[2])
    }

    #[test]
    fn limited_range_spans_black_to_white() {
        let colour = Colour {
            matrix: Matrix::Bt709,
            range: Range::Limited,
        };
        let black = rgb(colour, [16.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0]);
        let white = rgb(colour, [235.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0]);

        for channel in black {
            assert!(channel.abs() < 1e-4, "{black:?}");
        }
        for channel in white {
            assert!((channel - 1.0).abs() < 1e-4, "{white:?}");
        }
    }

    #[test]
    fn full_range_red_is_red() {
        // Pure red in BT.601 full range: Y = 0.299, U = -0.1687, V = 0.5.
        let colour = Colour {
            matrix: Matrix::Bt601,
            range: Range::Full,
        };
        let red = rgb(colour, [0.299, 0.5 - 0.168_736, 1.0]);
        assert!((red[0] - 1.0).abs() < 0.01, "{red:?}");
        assert!(red[1].abs() < 0.01, "{red:?}");
        assert!(red[2].abs() < 0.01, "{red:?}");
    }
}
