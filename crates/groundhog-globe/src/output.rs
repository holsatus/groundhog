//! The picture: the camera renders into an image, which is read back from the GPU every frame
//! and published to the frontend as a [`Frame`].
//!
//! The image follows the size the frontend asks for. Each size gets an image and a readback of
//! its own, so a readback that completes after a resize is still read at the size it was made
//! with.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use bevy::{
    asset::RenderAssetUsages,
    camera::RenderTarget,
    prelude::*,
    render::{
        gpu_readback::{Readback, ReadbackComplete},
        render_resource::{TextureFormat, TextureUsages},
    },
};
use tokio::sync::watch;

use crate::camera::{MainCamera, camera_bundle};

/// Rows of a read back image are padded to whole multiples of this many bytes.
const ROW_ALIGNMENT: u32 = 256;
/// The largest picture rendered, on each side: larger requests are clamped to it.
const MAX_SIDE: u32 = 4096;

/// A rendered picture.
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    /// Counts up from one with each frame published, so a viewer can tell a frame it has
    /// already uploaded.
    pub number: u64,
    pub width: u32,
    pub height: u32,
    /// Bytes per row, a multiple of 256: the rows are padded at their end.
    pub stride: u32,
    /// RGBA, 8 bits per channel, sRGB encoded, row by row from the top.
    pub pixels: Vec<u8>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({}: {}×{})", self.number, self.width, self.height)
    }
}

pub(crate) struct OutputPlugin {
    pub frames: watch::Sender<Option<Arc<Frame>>>,
}

impl Plugin for OutputPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Publisher {
            frames: self.frames.clone(),
            published: Arc::new(AtomicU64::new(0)),
        })
        .insert_resource(Target {
            size: UVec2::new(16, 16),
            wanted: UVec2::new(16, 16),
        })
        .add_systems(Startup, spawn_camera)
        .add_systems(PostUpdate, resize);
    }
}

#[derive(Resource)]
struct Publisher {
    frames: watch::Sender<Option<Arc<Frame>>>,
    published: Arc<AtomicU64>,
}

/// The size of the picture, in physical pixels.
#[derive(Resource)]
pub(crate) struct Target {
    size: UVec2,
    wanted: UVec2,
}

impl Target {
    pub(crate) fn request(&mut self, width: u32, height: u32) {
        self.wanted = UVec2::new(width.clamp(1, MAX_SIDE), height.clamp(1, MAX_SIDE));
    }
}

/// The readback of one image, and its size.
#[derive(Component)]
struct Reading(UVec2);

fn target_image(images: &mut Assets<Image>, size: UVec2) -> Handle<Image> {
    let mut image = Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    image.asset_usage = RenderAssetUsages::RENDER_WORLD;
    images.add(image)
}

fn spawn_camera(mut commands: Commands, target: Res<Target>, mut images: ResMut<Assets<Image>>) {
    let image = target_image(&mut images, target.size);
    commands.spawn((camera_bundle(), RenderTarget::Image(image.clone().into())));
    commands
        .spawn((Readback::texture(image), Reading(target.size)))
        .observe(publish);
}

fn resize(
    mut commands: Commands,
    mut target: ResMut<Target>,
    mut images: ResMut<Assets<Image>>,
    camera: Single<Entity, With<MainCamera>>,
    readings: Query<Entity, With<Reading>>,
) {
    if target.size == target.wanted {
        return;
    }
    target.size = target.wanted;
    let image = target_image(&mut images, target.size);
    commands
        .entity(*camera)
        .insert(RenderTarget::Image(image.clone().into()));
    for reading in &readings {
        commands.entity(reading).despawn();
    }
    commands
        .spawn((Readback::texture(image), Reading(target.size)))
        .observe(publish);
}

fn publish(readback: On<ReadbackComplete>, readings: Query<&Reading>, publisher: Res<Publisher>) {
    let Ok(Reading(size)) = readings.get(readback.entity) else {
        return;
    };
    let stride = (size.x * 4).next_multiple_of(ROW_ALIGNMENT);
    let Some(length) = usize::try_from(u64::from(stride) * u64::from(size.y)).ok() else {
        return;
    };
    if readback.data.len() < length {
        log::warn!(
            "A globe frame of {}×{} came back with {} bytes",
            size.x,
            size.y,
            readback.data.len()
        );
        return;
    }
    let number = publisher.published.fetch_add(1, Ordering::Relaxed) + 1;
    let frame = Frame {
        number,
        width: size.x,
        height: size.y,
        stride,
        pixels: readback.data.get(..length).unwrap_or_default().to_vec(),
    };
    publisher.frames.send_replace(Some(Arc::new(frame)));
}
