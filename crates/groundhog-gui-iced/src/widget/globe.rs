//! The 3D globe's newest frame, drawn on the GPU, and the gestures made on
//! it.
//!
//! The globe renders on a thread of its own (`groundhog_globe`), at the size
//! this widget reports from each draw, in physical pixels, and one frame for
//! each draw: the display paces it, and it rests while it is not shown.
//! Frames come through memory, not a shared GPU device, so each is uploaded
//! here as it arrives: the widget redraws every display frame while it is
//! shown and uploads only a frame it has not uploaded yet. That costs no
//! view rebuilds, which a message per frame would.
//!
//! Gestures go straight to the globe's [`Controls`]: dragging with the left
//! button or a finger pans, dragging with the right or middle button turns
//! the camera, and the wheel zooms. They are the globe's view state, as
//! panning is the map's, so the application never sees them.

use std::sync::Arc;

use groundhog_globe::{Controls, Frame, Gesture, Globe};
use iced::{
    Event, Length, Point, Rectangle, Size, Widget, mouse, touch, wgpu,
    widget::shader::{self, Action, Viewport},
    window,
};

/// What `globe` renders, filling the space it is given.
pub fn globe<M>(globe: &Globe) -> impl Widget<M> + use<M> {
    iced::widget::Shader::new(GlobeView {
        controls: globe.controls().clone(),
        frames: globe.frames(),
    })
    .width(Length::Fill)
    .height(Length::Fill)
}

struct GlobeView {
    controls: Controls,
    /// Read at each draw, as frames arrive between views.
    frames: tokio::sync::watch::Receiver<Option<Arc<Frame>>>,
}

/// A drag under way: what it does, and where the pointer was last.
#[derive(Debug, Clone, Copy)]
struct Drag {
    kind: DragKind,
    last: Point,
    /// The finger making it, or `None` for the mouse.
    finger: Option<touch::Finger>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragKind {
    Pan,
    Look,
}

impl DragKind {
    fn gesture(self, dx: f64, dy: f64) -> Gesture {
        match self {
            DragKind::Pan => Gesture::Pan { dx, dy },
            DragKind::Look => Gesture::Look { dx, dy },
        }
    }
}

/// Wheel pixels per step of zoom, for touchpads.
const PIXELS_PER_STEP: f32 = 60.0;

impl<M> shader::Program<M> for GlobeView {
    type State = Option<Drag>;
    type Primitive = Primitive;

    fn update(
        &self,
        drag: &mut Option<Drag>,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<M>> {
        let send = |kind: DragKind, from: Point, to: Point| {
            let height = f64::from(bounds.height.max(1.0));
            let (dx, dy) = (f64::from(to.x - from.x), f64::from(to.y - from.y));
            self.controls
                .gesture(kind.gesture(dx / height, dy / height));
        };

        match event {
            // Keep drawing while shown: frames arrive on their own
            Event::Window(window::Event::RedrawRequested(_)) => Some(Action::request_redraw()),

            Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                let at = cursor.position_over(bounds)?;
                let kind = match button {
                    mouse::Button::Left => DragKind::Pan,
                    mouse::Button::Right | mouse::Button::Middle => DragKind::Look,
                    _ => return None,
                };
                *drag = Some(Drag {
                    kind,
                    last: at,
                    finger: None,
                });
                Some(Action::capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let current = drag.as_mut().filter(|drag| drag.finger.is_none())?;
                send(current.kind, current.last, *position);
                current.last = *position;
                Some(Action::capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(_)) => {
                drag.take().filter(|drag| drag.finger.is_none())?;
                Some(Action::capture())
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                cursor.position_over(bounds)?;
                let steps = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / PIXELS_PER_STEP,
                };
                self.controls.gesture(Gesture::Zoom {
                    steps: f64::from(steps),
                });
                Some(Action::capture())
            }

            Event::Touch(touch::Event::FingerPressed { id, position }) => {
                if drag.is_some() || !bounds.contains(*position) {
                    return None;
                }
                *drag = Some(Drag {
                    kind: DragKind::Pan,
                    last: *position,
                    finger: Some(*id),
                });
                Some(Action::capture())
            }
            Event::Touch(touch::Event::FingerMoved { id, position }) => {
                let current = drag.as_mut().filter(|drag| drag.finger == Some(*id))?;
                send(current.kind, current.last, *position);
                current.last = *position;
                Some(Action::capture())
            }
            Event::Touch(
                touch::Event::FingerLifted { id, .. } | touch::Event::FingerLost { id, .. },
            ) => {
                drag.take_if(|drag| drag.finger == Some(*id))?;
                Some(Action::capture())
            }
            _ => None,
        }
    }

    fn draw(&self, _drag: &Option<Drag>, _cursor: mouse::Cursor, _bounds: Rectangle) -> Primitive {
        Primitive {
            frame: self.frames.borrow().clone(),
            controls: self.controls.clone(),
        }
    }

    fn mouse_interaction(
        &self,
        drag: &Option<Drag>,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        match drag {
            Some(_) => mouse::Interaction::Grabbing,
            None if cursor.is_over(bounds) => mouse::Interaction::Grab,
            None => mouse::Interaction::default(),
        }
    }
}

/// The frame to draw, and where to report the size it should have.
#[derive(Debug)]
pub struct Primitive {
    frame: Option<Arc<Frame>>,
    controls: Controls,
}

impl shader::Primitive for Primitive {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        let globe = self.controls.globe();
        if !pipeline.exchange.is_with(globe) {
            // Not a picture of this globe
            pipeline.picture = None;
        }
        let size = physical_size(bounds.size(), viewport.scale_factor());
        if pipeline.exchange.ask(globe, size) {
            self.controls.resize(size.width, size.height);
        }
        // The next frame, for a later draw: this one shows the newest yet
        self.controls.frame();

        let Some(frame) = &self.frame else {
            return;
        };
        if pipeline.exchange.has(frame.number) {
            return;
        }
        // A texture larger than the device allows fails wgpu's validation,
        // which panics. The last frame that fit stays on screen.
        let max = device.limits().max_texture_dimension_2d;
        if frame.width > max || frame.height > max || !fills(frame) {
            return;
        }

        let fits = pipeline
            .picture
            .as_ref()
            .is_some_and(|picture| picture.size == (frame.width, frame.height));
        if !fits {
            pipeline.picture = Some(Texture::new(device, pipeline, frame));
        }
        if let Some(picture) = &pipeline.picture {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &picture.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &frame.pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(frame.stride),
                    rows_per_image: Some(frame.height),
                },
                picture.texture.size(),
            );
            pipeline.exchange.uploaded = Some(frame.number);
        }
    }

    fn draw(&self, pipeline: &Pipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        let Some(picture) = &pipeline.picture else {
            return true;
        };
        render_pass.set_pipeline(&pipeline.pipeline);
        render_pass.set_bind_group(0, &picture.bind_group, &[]);
        render_pass.draw(0..4, 0..1);
        true
    }
}

/// The size of `bounds` in physical pixels, as the globe renders it.
fn physical_size(bounds: Size, scale_factor: f32) -> Size<u32> {
    let side = |logical: f32| (logical * scale_factor).round().max(1.0) as u32;
    Size::new(side(bounds.width), side(bounds.height))
}

/// Whether `frame` holds all the bytes its size and stride say, so an
/// upload cannot read past it.
fn fills(frame: &Frame) -> bool {
    frame.stride >= frame.width.saturating_mul(4)
        && u64::from(frame.stride) * u64::from(frame.height) <= frame.pixels.len() as u64
}

/// The globe's pipeline and the texture of its picture.
pub struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The format the picture is viewed as: the frame's bytes are sRGB
    /// already, so they pass through unchanged into a target of either kind.
    view_format: wgpu::TextureFormat,
    picture: Option<Texture>,
    exchange: Exchange,
}

/// What the pipeline asked of the globe it shows, and took from it.
///
/// The pipeline outlives a globe: iced keeps one per primitive type. A
/// globe shown again is a new one, which starts at a size of its own and
/// numbers its frames from one, so nothing carries over from the last.
#[derive(Debug, Default)]
struct Exchange {
    globe: Option<u64>,
    /// The size last asked of the globe.
    requested: Option<Size<u32>>,
    /// The number of the frame in the texture.
    uploaded: Option<u64>,
}

impl Exchange {
    fn is_with(&self, globe: u64) -> bool {
        self.globe == Some(globe)
    }

    /// Whether `globe` must be asked for `size`, which it is taken to be
    /// from here on.
    fn ask(&mut self, globe: u64, size: Size<u32>) -> bool {
        if !self.is_with(globe) {
            *self = Self {
                globe: Some(globe),
                ..Self::default()
            };
        }
        let asked = self.requested != Some(size);
        self.requested = Some(size);
        asked
    }

    /// Whether frame `number` of the globe last asked is uploaded.
    fn has(&self, number: u64) -> bool {
        self.uploaded == Some(number)
    }
}

/// The texture holding a frame, at one size.
struct Texture {
    size: (u32, u32),
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

impl Texture {
    fn new(device: &wgpu::Device, pipeline: &Pipeline, frame: &Frame) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("globe picture"),
            size: wgpu::Extent3d {
                width: frame.width.max(1),
                height: frame.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(pipeline.view_format),
            ..wgpu::TextureViewDescriptor::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globe bind group"),
            layout: &pipeline.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(&pipeline.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        Self {
            size: (frame.width, frame.height),
            texture,
            bind_group,
        }
    }
}

impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globe bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("globe pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("globe shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("globe.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("globe pipeline"),
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
            label: Some("globe sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });

        Self {
            pipeline,
            layout,
            sampler,
            view_format: match format.is_srgb() {
                true => wgpu::TextureFormat::Rgba8UnormSrgb,
                false => wgpu::TextureFormat::Rgba8Unorm,
            },
            picture: None,
            exchange: Exchange::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, stride: u32, bytes: usize) -> Frame {
        Frame {
            number: 1,
            width,
            height,
            stride,
            pixels: vec![0; bytes],
        }
    }

    #[test]
    fn a_frame_short_of_its_rows_is_not_uploaded() {
        assert!(fills(&frame(10, 2, 256, 512)));
        assert!(!fills(&frame(10, 2, 256, 511)), "a byte short");
        assert!(
            !fills(&frame(100, 2, 256, 4096)),
            "rows narrower than the width"
        );
    }

    #[test]
    fn a_globe_shown_again_is_asked_for_its_size() {
        let size = Size::new(800, 600);
        let mut exchange = Exchange::default();
        assert!(exchange.ask(1, size));
        assert!(!exchange.ask(1, size), "asked already");
        assert!(exchange.ask(1, Size::new(400, 600)), "resized");

        // Switched away and back: a new globe, at the size the last had
        assert!(exchange.ask(2, Size::new(400, 600)));
    }

    #[test]
    fn a_globe_shown_again_has_its_frames_uploaded() {
        let size = Size::new(800, 600);
        let mut exchange = Exchange::default();
        exchange.ask(1, size);
        exchange.uploaded = Some(1);
        assert!(exchange.has(1));

        // The new globe numbers its frames from one again
        exchange.ask(2, size);
        assert!(!exchange.has(1));
    }

    #[test]
    fn the_globe_renders_at_the_physical_size() {
        assert_eq!(
            physical_size(Size::new(100.4, 50.0), 2.0),
            Size::new(201, 100)
        );
        assert_eq!(physical_size(Size::new(0.0, 0.0), 1.0), Size::new(1, 1));
    }
}
