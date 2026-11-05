//! The sun, the haze, and the sky around the camera.

use bevy::{
    camera::{Exposure, visibility::NoFrustumCulling},
    light::NotShadowCaster,
    math::DVec2,
    prelude::*,
};

use crate::{
    camera::{CameraSystems, View},
    geo,
    material::SkyMaterial,
    scattering::{ScatteringLuts, ScatteringParams, ScatteringPlugin, SkyLight},
};

/// Radius of the sphere the sky is drawn on, which must enclose all of the visible terrain.
const SKY_RADIUS: f32 = 1e9;
/// Average reflectance of the ground below objects, as in `scattering.wgsl`.
const GROUND_ALBEDO: f32 = 0.3;

pub(crate) struct AtmospherePlugin;

impl Plugin for AtmospherePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ScatteringPlugin)
            .insert_resource(Sun::now())
            .init_resource::<Haze>()
            .insert_resource(ClearColor(Color::BLACK))
            .add_systems(Startup, spawn)
            .add_systems(
                Update,
                (follow_clock, update, light_objects)
                    .chain()
                    .after(CameraSystems),
            );
    }
}

/// Where the sun is straight overhead, in radians.
#[derive(Resource)]
pub(crate) struct Sun {
    pub lon: f64,
    pub lat: f64,
}

impl Sun {
    /// Where the sun is overhead at the moment.
    pub(crate) fn now() -> Self {
        let since_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        Self::at(since_epoch.as_secs_f64())
    }

    /// Where the sun is overhead `unix_s` seconds after 1970 began in UTC, to within a degree
    /// or two: the declination follows the seasons as a cosine, and the equation of time is
    /// left out, which is plenty for lighting a view.
    pub(crate) fn at(unix_s: f64) -> Self {
        const DAY_S: f64 = 86_400.0;
        const YEAR_DAYS: f64 = 365.2422;
        // The December solstice of 2000, 21 December 07:37 UTC
        const SOLSTICE_UNIX_S: f64 = 977_384_220.0;
        const OBLIQUITY_DEG: f64 = 23.44;

        let days = (unix_s - SOLSTICE_UNIX_S) / DAY_S;
        let season = std::f64::consts::TAU * days.rem_euclid(YEAR_DAYS) / YEAR_DAYS;
        let lat = -OBLIQUITY_DEG.to_radians() * season.cos();
        // At noon UTC the sun is over the prime meridian, and it moves west
        let day_fraction = (unix_s / DAY_S).rem_euclid(1.0);
        let lon = std::f64::consts::PI - std::f64::consts::TAU * day_fraction;
        Self { lon, lat }
    }

    pub(crate) fn direction(&self) -> Vec3 {
        geo::up(DVec2::new(self.lon, self.lat)).as_vec3()
    }
}

/// The aerosols in the air, as their vertical optical depth at 550 nm from sea level: about
/// 0.05 on a clear day, 0.1 to 0.2 on a typical one, and 0.5 and more in thick haze.
#[derive(Resource)]
pub(crate) struct Haze(pub f32);

impl Default for Haze {
    fn default() -> Self {
        Self(0.1)
    }
}

/// Objects drawn with Bevy's materials, such as a vehicle, are lit by the sun and the sky as
/// they are where the entity with this component is, or at the camera without one.
#[derive(Component)]
pub(crate) struct LightProbe;

#[derive(Component)]
struct Sky(Handle<SkyMaterial>);

fn spawn(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<SkyMaterial>>,
    luts: Res<ScatteringLuts>,
) {
    // The shaders only read the direction of the light
    commands.spawn((
        DirectionalLight {
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::default(),
    ));

    let material = materials.add(SkyMaterial::new(&luts));
    commands.spawn((
        Mesh3d(meshes.add(Sphere::new(1.0).mesh().uv(64, 32))),
        MeshMaterial3d(material.clone()),
        Transform::from_scale(Vec3::splat(SKY_RADIUS)),
        NoFrustumCulling,
        NotShadowCaster,
        Sky(material),
    ));
}

/// Keep the sun where it is at the moment, checked once a second.
fn follow_clock(time: Res<Time>, mut since: Local<f32>, mut sun: ResMut<Sun>) {
    *since += time.delta_secs();
    if *since >= 1.0 {
        *since = 0.0;
        *sun = Sun::now();
    }
}

fn update(
    sun: Res<Sun>,
    haze: Res<Haze>,
    view: Res<View>,
    mut params: ResMut<ScatteringParams>,
    sky: Single<&Sky>,
    mut light: Single<&mut Transform, With<DirectionalLight>>,
    mut materials: ResMut<Assets<SkyMaterial>>,
) {
    let direction = sun.direction();
    // The light shines along its forward direction, away from the sun
    **light = Transform::default().looking_to(-direction, Vec3::Z);

    if let Some(mut material) = materials.get_mut(&sky.0) {
        material.camera = view.position.as_vec3().extend(0.0);
    }
    *params = ScatteringParams {
        radius: view.position.length() as f32,
        sun_mu: view.up().as_vec3().dot(direction),
        aerosols: haze.0,
    };
}

/// Lights Bevy's materials like the terrain: the directional light is the sunlight as it reaches
/// the probe, and the ambient light averages the light of the sky from above and that reflected
/// by the ground from below.
fn light_objects(
    sky_light: Res<SkyLight>,
    sun: Res<Sun>,
    view: Res<View>,
    probe: Option<Single<&Transform, With<LightProbe>>>,
    mut light: Single<&mut DirectionalLight>,
    mut ambient: ResMut<GlobalAmbientLight>,
) {
    // Positions are relative to the camera
    let offset = probe.map_or(Vec3::ZERO, |transform| transform.translation);
    let position = (view.position + offset.as_dvec3()).as_vec3();
    let direction = sun.direction();
    let Some((sunlight, sky)) = sky_light.at(position, direction) else {
        return;
    };

    // Bevy scales lights by the exposure of the camera, where the shaders of the atmosphere
    // output their light directly
    let exposure = Exposure::default().exposure();
    light.color = Color::linear_rgb(sunlight.x, sunlight.y, sunlight.z);
    light.illuminance = 1.0 / exposure;

    let up = position.normalize();
    let ground = GROUND_ALBEDO * (sunlight * up.dot(direction).max(0.0) + sky);
    // Bevy's ambient light is radiance, where the tables hold irradiance
    let radiance = (sky + ground) * 0.5 / std::f32::consts::PI;
    ambient.color = Color::linear_rgb(radiance.x, radiance.y, radiance.z);
    ambient.brightness = 1.0 / exposure;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn degrees(sun: &Sun) -> (f64, f64) {
        let lon = sun.lon.to_degrees();
        (lon - 360.0 * (lon / 360.0).round(), sun.lat.to_degrees())
    }

    #[test]
    fn the_sun_is_over_the_prime_meridian_at_noon_utc() {
        // 2024-06-20 12:00 UTC, near the June solstice
        let (lon, lat) = degrees(&Sun::at(1_718_884_800.0));
        assert!(lon.abs() < 1e-6, "{lon}");
        assert!((lat - 23.44).abs() < 0.5, "{lat}");
    }

    #[test]
    fn the_sun_moves_west_and_follows_the_seasons() {
        // 2024-12-21 18:00 UTC: a quarter turn west of noon, over the Tropic of Capricorn
        let (lon, lat) = degrees(&Sun::at(1_734_804_000.0));
        assert!((lon + 90.0).abs() < 1e-6, "{lon}");
        assert!((lat + 23.44).abs() < 0.5, "{lat}");

        // 2024-03-20, the equinox
        let (_, lat) = degrees(&Sun::at(1_710_936_000.0));
        assert!(lat.abs() < 1.0, "{lat}");
    }
}
