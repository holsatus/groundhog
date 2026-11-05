//! Renders the globe on this machine's GPU. Ignored by default, as CI has none:
//!
//! ```sh
//! GROUNDHOG_GLOBE_SNAPSHOT=/tmp/globe.png cargo test -p groundhog-globe --release -- --ignored
//! ```

use std::time::{Duration, Instant};

use groundhog_globe::{Globe, Options};

#[test]
#[ignore = "needs a GPU, and the network for the tiles"]
fn frames_arrive_at_the_size_asked_for() {
    let cache = std::env::temp_dir().join("groundhog-globe-test-tiles");
    let globe = Globe::start(Options {
        cache_dir: Some(cache),
        ..Options::default()
    })
    .unwrap();
    globe.controls().resize(640, 360);

    let started = Instant::now();
    let deadline = started + Duration::from_secs(60);
    let mut first = None;
    let mut sized = 0;
    // Give the tiles a while to stream in before taking the snapshot
    // Asking for frames as a display would, at 100 Hz
    while Instant::now() < deadline && sized < 300 {
        globe.controls().frame();
        std::thread::sleep(Duration::from_millis(10));
        let Some(frame) = globe.latest() else {
            continue;
        };
        first.get_or_insert_with(|| started.elapsed());
        if (frame.width, frame.height) == (640, 360) {
            sized += 1;
        }
    }
    let frame = globe.latest().expect("a frame");
    eprintln!(
        "first frame after {:?}, frame {} after {:?}",
        first,
        frame.number,
        started.elapsed()
    );
    assert_eq!((frame.width, frame.height), (640, 360));
    assert_eq!(frame.stride, 640 * 4);
    assert!(frame.pixels.chunks(4).any(|pixel| pixel[..3] != [0, 0, 0]));

    if let Some(path) = std::env::var_os("GROUNDHOG_GLOBE_SNAPSHOT") {
        let rows = frame
            .pixels
            .chunks(frame.stride as usize)
            .flat_map(|row| &row[..frame.width as usize * 4])
            .copied()
            .collect();
        image::RgbaImage::from_raw(frame.width, frame.height, rows)
            .unwrap()
            .save(path)
            .unwrap();
    }
}
