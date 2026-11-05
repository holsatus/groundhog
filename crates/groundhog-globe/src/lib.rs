//! A 3D globe: satellite or map imagery draped over real elevation on a spherical earth,
//! streamed from the web, under an atmosphere.
//!
//! It is rendered by Bevy, offscreen, on a thread of its own: a frontend starts a [`Globe`],
//! tells it the size of the picture and the gestures made on it through [`Controls`], and draws
//! the newest [`Frame`]. The globe renders a frame only when asked ([`Controls::frame`]), so a
//! frontend that asks once for each frame it shows paces the globe by its display: no frames
//! rendered that are never shown, none shown twice while the next is due, and none rendered
//! while nothing shows the globe. The frontend and the globe share no GPU device, so they need not agree
//! on a wgpu version; each frame crosses through memory instead.
//!
//! The rendering is vendored from bevy-worldmap: the globe, the terrain, the tiles and the
//! atmosphere, without its flight simulator.

#![warn(unreachable_pub)]

mod atmosphere;
mod camera;
mod fetch;
mod geo;
mod material;
mod mesh;
mod output;
mod post;
mod ride;
mod scattering;
mod sources;
mod terrain;
mod tiles;

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use bevy::{
    app::{PluginsState, TerminalCtrlCHandlerPlugin},
    prelude::*,
    window::ExitCondition,
};
use crossbeam_channel::{Receiver, Sender};
use tokio::sync::watch;

pub use camera::{Gesture, Overhead, Placement, StartView};
pub use output::Frame;
pub use ride::{Fix, Orientation};

/// The imagery draped over the terrain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Imagery {
    #[default]
    Satellite,
    /// OpenStreetMap's map.
    Map,
}

impl Imagery {
    fn source(self) -> sources::Source {
        match self {
            Imagery::Satellite => sources::Source::Satellite,
            Imagery::Map => sources::Source::OpenStreetMap,
        }
    }

    /// Whom the imagery and the elevation are credited to, to show with the picture.
    pub fn attribution(self) -> [&'static str; 2] {
        [
            self.source().attribution(),
            sources::Source::Terrain.attribution(),
        ]
    }
}

/// How a globe starts.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Where fetched tiles are cached, or nowhere.
    pub cache_dir: Option<PathBuf>,
    pub start: Placement,
    pub imagery: Imagery,
}

/// A running globe. Dropping it stops the globe, and its thread ends shortly after.
#[derive(Debug)]
pub struct Globe {
    controls: Controls,
    frames: watch::Receiver<Option<Arc<Frame>>>,
    focus: watch::Receiver<Option<Overhead>>,
}

impl Globe {
    /// Start rendering on a thread of its own.
    ///
    /// # Errors
    ///
    /// When the thread cannot be started.
    pub fn start(options: Options) -> std::io::Result<Self> {
        static STARTED: AtomicU64 = AtomicU64::new(0);

        let (commands, receiver) = crossbeam_channel::unbounded();
        let (frames, frames_rx) = watch::channel(None);
        let (focus, focus_rx) = watch::channel(None);
        thread::Builder::new()
            .name("globe".into())
            .spawn(move || run(options, receiver, frames, focus))?;
        Ok(Self {
            controls: Controls {
                globe: STARTED.fetch_add(1, Ordering::Relaxed),
                commands,
            },
            frames: frames_rx,
            focus: focus_rx,
        })
    }

    /// What moves the globe, to hand to whatever the gestures are made on.
    pub fn controls(&self) -> &Controls {
        &self.controls
    }

    /// The newest frame, if one has been rendered.
    pub fn latest(&self) -> Option<Arc<Frame>> {
        self.frames.borrow().clone()
    }

    /// What the camera looks at, once it has been placed: to show the same place on a map.
    pub fn focus(&self) -> Option<Overhead> {
        *self.focus.borrow()
    }

    /// Changes with each new frame, and closes if the globe stops on its own (it found no
    /// GPU to render with, say).
    pub fn frames(&self) -> watch::Receiver<Option<Arc<Frame>>> {
        self.frames.clone()
    }
}

/// Sends the globe the size of its picture and the gestures made on it. Cheap to clone; the
/// globe stops once the [`Globe`] and every clone of these are gone.
#[derive(Clone, Debug)]
pub struct Controls {
    globe: u64,
    commands: Sender<Command>,
}

impl Controls {
    /// An identity unique to the globe these control, for a viewer that keeps state about the
    /// globe it shows: each globe starts at a size of its own, and numbers its frames from one.
    pub fn globe(&self) -> u64 {
        self.globe
    }

    /// Render at `width` × `height` physical pixels.
    pub fn resize(&self, width: u32, height: u32) {
        self.send(Command::Resize { width, height });
    }

    pub fn gesture(&self, gesture: Gesture) {
        self.send(Command::Gesture(gesture));
    }

    pub fn imagery(&self, imagery: Imagery) {
        self.send(Command::Imagery(imagery));
    }

    /// Ride along with the [`fix`](Self::fix)es and [`orientation`](Self::orientation)s sent
    /// from here on: the camera is carried as they say, looking along the way they point, and
    /// gestures do not move it. Starts the ride over if one is under way; a placement or
    /// [`release`](Self::release) ends it.
    pub fn ride(&self) {
        self.send(Command::Ride);
    }

    /// Stop riding, the camera free where the ride left it.
    pub fn release(&self) {
        self.send(Command::Release);
    }

    /// Where the ridden pose was. Ignored while not riding.
    pub fn fix(&self, fix: Fix) {
        self.send(Command::Fix(fix));
    }

    /// Which way the ridden pose pointed. Ignored while not riding.
    pub fn orientation(&self, orientation: Orientation) {
        self.send(Command::Orientation(orientation));
    }

    /// Render a frame, with every command sent before it. Requests that pile up while a frame
    /// renders are taken as one. The frame arrives a frame or two of requests later, once it
    /// is read back from the GPU.
    pub fn frame(&self) {
        self.send(Command::Frame);
    }

    fn send(&self, command: Command) {
        // The globe has stopped on its own, which its frames tell
        drop(self.commands.send(command));
    }
}

#[derive(Debug)]
enum Command {
    Resize { width: u32, height: u32 },
    Gesture(Gesture),
    Imagery(Imagery),
    Ride,
    Release,
    Fix(Fix),
    Orientation(Orientation),
    Frame,
}

fn run(
    options: Options,
    commands: Receiver<Command>,
    frames: watch::Sender<Option<Arc<Frame>>>,
    focus: watch::Sender<Option<Overhead>>,
) {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                close_when_requested: false,
                ..default()
            })
            // The frontend handles Ctrl-C
            .disable::<TerminalCtrlCHandlerPlugin>(),
    )
    .add_plugins((
        material::MaterialsPlugin,
        tiles::TilesPlugin {
            cache_dir: options.cache_dir,
        },
        camera::CameraPlugin {
            start: options.start,
            focus,
        },
        terrain::TerrainPlugin,
        atmosphere::AtmospherePlugin,
        post::PostProcessPlugin,
        output::OutputPlugin { frames },
    ))
    .set_runner(move |app| run_on_request(app, &commands));
    app.world_mut()
        .resource_mut::<terrain::TerrainSettings>()
        .imagery = options.imagery.source();

    log::info!("The globe starts");
    let exit = app.run();
    log::info!("The globe stopped: {exit:?}");
}

/// Run the app a frame at a time, as the frontend asks, until it lets go of the globe.
fn run_on_request(mut app: App, commands: &Receiver<Command>) -> AppExit {
    // As Bevy's own runners start
    if app.plugins_state() != PluginsState::Cleaned {
        while app.plugins_state() == PluginsState::Adding {
            bevy::tasks::tick_global_task_pools_on_main_thread();
        }
        app.finish();
        app.cleanup();
    }

    // Each wait ends with the first command; the rest sent meanwhile come along
    while let Ok(first) = commands.recv() {
        let mut frame = false;
        for command in std::iter::once(first).chain(commands.try_iter()) {
            frame |= apply(app.world_mut(), command);
        }
        if frame {
            app.update();
            if let Some(exit) = app.should_exit() {
                return exit;
            }
        }
    }
    AppExit::Success
}

/// Take a command into the world, returning whether it asks for a frame.
fn apply(world: &mut World, command: Command) -> bool {
    match command {
        Command::Resize { width, height } => world
            .resource_mut::<output::Target>()
            .request(width, height),
        Command::Gesture(gesture) => world.resource_mut::<camera::View>().apply(gesture),
        Command::Ride => world
            .resource_mut::<camera::View>()
            .ride(std::time::Instant::now()),
        Command::Release => world.resource_mut::<camera::View>().release(),
        // Received now, which is how late it reaches the ride
        Command::Fix(fix) => world
            .resource_mut::<camera::View>()
            .fix(fix, std::time::Instant::now()),
        Command::Orientation(orientation) => world
            .resource_mut::<camera::View>()
            .orientation(orientation, std::time::Instant::now()),
        Command::Imagery(imagery) => {
            let mut settings = world.resource_mut::<terrain::TerrainSettings>();
            if settings.imagery != imagery.source() {
                settings.imagery = imagery.source();
            }
        }
        Command::Frame => return true,
    }
    false
}
