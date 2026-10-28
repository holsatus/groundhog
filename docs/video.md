# Video from the vehicle

How Groundhog shows a drone's video stream, with latency low enough to fly
by: what QGroundControl does, where latency comes from, and how a stream
reaches an iced view on the desktop and on Android. Steps 1 to 3 of the
[order of work](#order-of-work) are built: video on the desktop, in the
Video panel and, in the floating shell, in Fly's corner.

## What QGroundControl does

QGroundControl receives video with **GStreamer** (its current tree:
`src/VideoManager/VideoReceiver/GStreamer/`). A Qt Multimedia receiver
exists only for builds without GStreamer. The pipeline is

```
source ─► tee ─┬─► queue (leaky) ─► decodebin3 ─► video sink   (display)
               └─► queue ─► parser ─► splitmuxsink              (recording)
```

**Sources** (`GstSourceFactory.cc`), chosen by the URI's scheme:

| URI | Elements |
|---|---|
| `rtsp://`, `rtspt://` | `rtspsrc`, video stream only (other SDP streams are not set up) |
| `udp://` (H.264), `udp265://` (H.265) | `udpsrc` with RTP caps (`application/x-rtp, media=video, clock-rate=90000, encoding-name=H264`), then `rtph264depay`/`rtph265depay` and the parser |
| `mpegts://` | `udpsrc` and `tsdemux` |
| `tcp://` | `tcpclientsrc`, MPEG-TS |

UDP on port 5600 is QGroundControl's default; that is what companion
computers and most "RTP over UDP" camera setups send to.

**The settings that keep latency low:**

- **The jitter buffer is the main dial.** QGC's *Low Latency Mode* setting
  (`lowLatencyMode`, off by default) removes it: for RTSP it sets `rtspsrc
  latency=0` (rtspsrc always has an internal jitter buffer), and for UDP it
  links `udpsrc` straight to the depayloader. Otherwise the buffer holds
  `rtpJitterLatencyMs` (default 80 ms), with `drop-on-latency=true` and
  `do-lost=true`, and retransmission (RFC 4588, `rtx-delay=25 ms`, one
  retry) only when that budget is at least 40 ms.
- **RTSP:** `do-rtcp=true`, `udp-reconnect=true`, `drop-on-latency=true`,
  `tcp-timeout=5 s`, `retry=3`, and UDP and TCP allowed as transports, so a
  firewall that blocks UDP falls back to TCP-interleaved instead of hanging.
- **UDP:** `udpsrc buffer-size` of 8 MB (clamped to `net.core.rmem_max`),
  so bursts of a keyframe are not lost in the kernel.
- **Display branch:** a `queue` with `leaky=downstream` and
  `max-size-buffers=2` (bytes and time 0): when the display falls behind,
  old frames are dropped instead of the stream stalling.
- **Decoding:** `decodebin3`, which picks a hardware decoder where one is
  ranked (VA-API, NVDEC, D3D11/12, VideoToolbox, Android MediaCodec) and
  falls back to software. QGC turns **QoS off** on every decoder it plugs:
  on a live stream, a decoder with QoS on starts dropping every frame after
  the first late one.
- **The sink does not wait for the clock** in low-latency mode (`sync=false`),
  and QGC's own sink (`qgcqvideosink`) keeps only the newest frame for the
  GUI, so a stalled GUI holds at most one frame.
- **Parameter sets** are repeated at every IDR (`h264parse
  config-interval=-1`), so a recording or a decoder joining mid-stream can
  start.
- Hardware frames reach Qt without a copy where the platform allows
  (DMABuf, GL, D3D, IOSurface, AHardwareBuffer), with a CPU copy as the
  fallback.

On Android QGC runs the GStreamer Android SDK, linked statically. It calls
`gst_init` from C++, but still ships the plugins' Java sources in the APK:
the MediaCodec plugin (`androidmedia`) needs its Java helper classes and
the application's class loader.

## Where latency comes from

From the camera's sensor to the operator's screen:

| Stage | Typical cost | What sets it |
|---|---|---|
| Encoder | 1 frame to several | B-frames, lookahead, rate control. Ask for `tune=zerolatency`-style settings: no B-frames, no lookahead, a short GOP or intra refresh. Not ours to set, but worth documenting for operators. |
| Network | ms on a direct link | Radio; the UDP receive buffer only absorbs bursts |
| **Jitter buffer** | 0 to 80+ ms | Our setting: 0 in low-latency mode |
| Depayloading, parsing | under 1 ms | — |
| **Decoder** | under 1 ms to a few frames | Hardware decoders are fast. Software decoders with *frame* threading hold one frame per thread; slice threading or one thread does not |
| Queues | 0 when leaky, small | Our setting |
| **Display** | up to one refresh | Waking the UI as soon as a frame arrives, not on a timer |

A quick check on this machine (720p30 H.264 over loopback UDP, software
decoder, GStreamer's latency tracer): depayloading, parsing, decoding and a
leaky queue add **about 1.1 ms per frame together** (the decoder 0.8 ms).
The receiving side is cheap. The budget is spent in the camera's encoder,
the jitter buffer and the wait for the display, and those are what this
design controls or documents.

## Getting frames into iced

The decoder produces frames on its own thread; iced draws on the UI thread.
Between them:

1. **A latest-frame slot.** The receiver writes each decoded frame into a
   shared slot (`Arc<Mutex<Option<Frame>>>` plus a generation counter),
   replacing any frame the UI has not drawn yet. A slow UI drops frames and
   never queues them, which is exactly QGC's sink's behaviour.
2. **A wake-up, not a poll.** After writing, the receiver sends a
   notification through a bounded channel of one (`try_send`, so a pending
   wake-up is not doubled), and a `Subscription` turns it into a
   `Message::VideoFrame`. iced then builds the view and draws at the next
   refresh. Nothing redraws while the stream is idle. The alternative, the
   widget requesting a redraw on every frame like `iced_video_player` does,
   avoids rebuilding the view but redraws at the display's rate while video
   shows.
3. **A `shader` widget** (iced's `widget::shader`, with the wgpu renderer
   the application already uses, on the desktop and on Android). Its
   primitive carries the frame (`Arc`, so building the view copies
   nothing) and its generation. Its `Pipeline` keeps the textures:
   `prepare` uploads the planes with `queue.write_texture` only when the
   generation changed, and recreates the textures only when the size does.
   `draw` renders a quad, letterboxed to the frame's aspect ratio, and
   converts YUV to RGB in WGSL.
4. **YUV stays YUV until the GPU.** Hardware decoders produce NV12 (a luma
   plane and an interleaved chroma plane), software decoders often I420
   (three planes). The shader takes both, and the colour matrix (BT.601 or
   BT.709, limited or full range) comes from the stream's caps. A 1080p
   NV12 frame is 3 MB to upload, against 8 MB as RGBA, and no CPU
   conversion runs.

The frame type, the slot and the widget are the same whichever backend
decodes. Above them, the video is a panel (`Panel::Video`): a pane in the
tiled shell, and in the floating shell Fly's background, with the map in a
corner, swapped by a tap, as QGroundControl does. Recording, when it comes,
taps the encoded stream before decoding, as QGC's tee does.

## Backends

| | GStreamer (QGC's choice) | FFmpeg (libavformat, libavcodec) | Rust transport and platform decoders |
|---|---|---|---|
| Transports | RTSP, RTP/UDP, MPEG-TS, TCP, all mature | RTSP, MPEG-TS, TCP; raw RTP needs an SDP | RTSP with `retina` (TCP solid, UDP experimental); RTP/UDP depayloading written by us |
| Hardware decoding | `decodebin3` ranks it automatically, every platform | Per-platform `hwaccel` code (VA-API, D3D11VA, VideoToolbox, MediaCodec) | One decoder per platform, written by us |
| Desktop | System GStreamer on Linux, SDK on Windows and macOS; `gstreamer-rs` 0.25 bindings | System or bundled FFmpeg; `ffmpeg-next` | A software or hardware decoder is still needed |
| Android | SDK linked statically; `androidmedia` needs Java helper classes and the class loader, which our Java-less APK lacks; `gstreamer-rs` expects separate libraries, the SDK ships one | Cross-compile FFmpeg with `--enable-mediacodec`; `av_jni_set_java_vm` with the VM we already have; no Java of our own | NDK `AMediaCodec` (C, no Java), `ndk` crate; `low-latency` format key on Android 11+ |
| Low latency | The settings above | `fflags=nobuffer`, `flags=low_delay`, `max_delay=0`, `probesize`/`analyzeduration` small, slice threads | Ours to keep: no jitter buffer, latest frame wins |
| Licence | LGPL (dynamic linking on the desktop; statically linked on Android, relinking must stay possible) | LGPL without the GPL parts | MIT/Apache |

Distribution codec packaging matters on Linux. This machine (Fedora) has
`openh264dec` but neither `avdec_h264` nor a VA-API H.264 decoder, without
extra repositories. Any backend that relies on the system's decoders has to
say which packages it needs.

## Recommendation

**Desktop: GStreamer, configured as QGroundControl configures it.** It
gives every transport QGC supports, hardware decoding through
`decodebin3`, and a low-latency path QGroundControl's users fly with. We
would be reusing settings tested in the field rather than discovering
them.

```
low latency:  udpsrc buffer-size=8388608 caps=application/x-rtp,…
                ! rtph264depay ! h264parse ! decodebin3 (qos=false on the decoder)
                ! appsink sync=false max-buffers=1 drop=true caps=video/x-raw,format={NV12,I420}
default:      the same, with rtpjitterbuffer latency=80 drop-on-latency=true do-lost=true
RTSP:         rtspsrc latency=0|80 drop-on-latency=true do-rtcp=true protocols=udp+tcp …
```

The `appsink` with `max-buffers=1 drop=true` is the latest-frame slot's
first half, so frames never wait in GStreamer either.

**Android: decide after the desktop works.** Both routes are real work:

- *GStreamer Android SDK*: one pipeline on every platform. It needs a Java
  build step in `android/build.sh` (`javac` and `d8`, for the
  `androidmedia` helper classes), a way to link `gstreamer-rs` against the
  SDK's single combined library, and the LGPL obligations of static
  linking.
- *NDK MediaCodec*: no Java, hardware decoding with Android's own
  low-latency mode. It needs RTSP from `retina` and an RTP/UDP H.264/H.265
  depayloader of our own (single NAL units, STAP-A and FU-A; a few hundred
  lines), and it is a second decoding path to maintain.

My leaning is MediaCodec, because the APK deliberately has no Java and the
GStreamer route's two blockers are both unproven. The frame type, the slot
and the widget do not care which route is chosen, so the choice can wait
until the desktop video works.

## Where it lives

- **A `groundhog-video` crate**: the receiver, which turns a URI and
  settings into decoded frames in a slot, and reports its state (connecting,
  streaming, stalled, failed) and statistics. It depends on GStreamer (or,
  on Android, `ndk`), which `groundhog-logic` should not carry. Another
  frontend can use it the same way.
- **The frontend**: `widget/video.rs` (the shader widget and its WGSL),
  `features/video.rs` (the panel: source, status, the low-latency switch),
  and the settings (URI, low-latency mode, jitter buffer).
- **Later, from the vehicle:** the MAVLink camera protocol's
  `VIDEO_STREAM_INFORMATION` names a camera's stream URI. Asked for with
  `MAV_CMD_REQUEST_MESSAGE`, it lets the vehicle configure the video, as
  QGroundControl does with cameras that support it. That belongs in the
  logic crate, as vehicle state.

## Order of work

1. **The widget, with a test source** (done: `widget/video.rs`, and
   `test://`, scrolling colour bars). `widget/video.rs` drawing NV12 and
   I420 frames from a slot, fed by a GStreamer `videotestsrc` pipeline into
   an `appsink`. This settles the iced side: textures, letterboxing, colour,
   wake-ups, and the cost per frame in the gallery and on the device.
2. **The receiver** (done). `groundhog-video` with the sources above, the
   low-latency and default jitter settings, QoS off, reconnecting a
   second after a stall, and state for the panel. Tested against a
   `videotestsrc ! x264enc tune=zerolatency ! rtph264pay ! udpsink`
   sender on loopback, as QGC's tests do.
3. **The panel** (done), in both shells, and the settings: the Video
   panel holds the source, the low-latency switch and the picture (the
   floating shell lists it under More). While the stream is on, the
   picture is the whole panel: the stream's state, changing the source and
   stopping float over its corner, and the details (URI, size, decoder,
   latency) show on hovering the state or pressing it. The chip also puts
   the picture alone in a full-screen window (the notices still show
   over it), left with Escape, back, or its own button. F11 puts the whole
   application in full screen; the two are kept apart, so leaving the
   video does not take the application out of its full screen. Fly
   shows it in the map's corner, a tap swapping the two. The source, the
   switch and whether the stream was on are kept in the configuration.
4. **Android**, by the route chosen then.
5. **Recording and camera discovery** (`VIDEO_STREAM_INFORMATION`).

## Sources

- QGroundControl's GStreamer receiver: [README](https://github.com/mavlink/qgroundcontrol/blob/master/src/VideoManager/VideoReceiver/GStreamer/README.md), [`GstSourceFactory.cc`](https://github.com/mavlink/qgroundcontrol/blob/master/src/VideoManager/VideoReceiver/GStreamer/GstSourceFactory.cc), [`DecodingBranch.cc`](https://github.com/mavlink/qgroundcontrol/blob/master/src/VideoManager/VideoReceiver/GStreamer/DecodingBranch.cc), [`GstVideoReceiver.cc`](https://github.com/mavlink/qgroundcontrol/blob/master/src/VideoManager/VideoReceiver/GStreamer/GstVideoReceiver.cc), [`GStreamerEnvironment.cc`](https://github.com/mavlink/qgroundcontrol/blob/master/src/VideoManager/VideoReceiver/GStreamer/GStreamerEnvironment.cc), [video settings](https://github.com/mavlink/qgroundcontrol/blob/master/src/Settings/Video.SettingsGroup.json)
- [`iced_video_player`](https://github.com/jazzfool/iced_video_player): GStreamer `appsink` with NV12 into an iced shader widget (iced 0.14)
- [`retina`](https://github.com/scottlamb/retina): RTSP client in Rust
- GStreamer on Android: [the `androidmedia` class-loader issue](https://gitlab.freedesktop.org/gstreamer/gstreamer/-/issues/3133), [linking `gstreamer-rs` against the combined library](https://gitlab.freedesktop.org/gstreamer/gstreamer/-/issues/2999), [Android tutorial](https://gstreamer.freedesktop.org/documentation/tutorials/android/a-running-pipeline.html)
