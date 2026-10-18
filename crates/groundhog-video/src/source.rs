//! Where a stream comes from, and the GStreamer pipeline that receives it.
//!
//! The URIs are QGroundControl's, so a camera set up for it works here:
//!
//! | URI | Stream |
//! |---|---|
//! | `rtsp://host[:port]/path`, `rtspt://…` | RTSP (`rtspt` over TCP only) |
//! | `udp://[address]:port` | H.264 over RTP/UDP (QGroundControl's default port is 5600) |
//! | `udp265://[address]:port` | H.265 over RTP/UDP |
//! | `mpegts://[address]:port` | MPEG-TS over UDP |
//! | `tcp://host:port` | MPEG-TS over TCP |
//! | `test://` | Scrolling colour bars, to check the video path (and its colours) without a camera |
//!
//! An empty address listens on every interface.

/// A video codec carried over RTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264,
    H265,
}

/// A stream's source, from its URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Rtsp {
        location: String,
    },
    Rtp {
        address: String,
        port: u16,
        codec: Codec,
    },
    MpegTsUdp {
        address: String,
        port: u16,
    },
    MpegTsTcp {
        host: String,
        port: u16,
    },
    TestPattern,
}

impl Source {
    /// The source `uri` names, or why it names none.
    pub fn parse(uri: &str) -> Result<Self, String> {
        let uri = uri.trim();
        let Some((scheme, rest)) = uri.split_once("://") else {
            return Err(format!("\"{uri}\" is not a URI (such as udp://:5600)"));
        };

        match scheme.to_ascii_lowercase().as_str() {
            "rtsp" | "rtspt" => {
                if rest.is_empty() {
                    return Err("An RTSP URI needs a host".to_string());
                }
                if uri.contains(['"', ' ']) {
                    return Err("An RTSP URI cannot contain spaces or quotes".to_string());
                }
                Ok(Source::Rtsp {
                    location: uri.to_string(),
                })
            }
            "udp" | "udp265" => {
                let (address, port) = address_and_port(rest, true)?;
                let codec = if scheme.eq_ignore_ascii_case("udp265") {
                    Codec::H265
                } else {
                    Codec::H264
                };
                Ok(Source::Rtp {
                    address,
                    port,
                    codec,
                })
            }
            "mpegts" => {
                let (address, port) = address_and_port(rest, true)?;
                Ok(Source::MpegTsUdp { address, port })
            }
            "tcp" => {
                let (host, port) = address_and_port(rest, false)?;
                Ok(Source::MpegTsTcp { host, port })
            }
            "test" => Ok(Source::TestPattern),
            other => Err(format!("Unknown video source scheme \"{other}://\"")),
        }
    }
}

/// `[address]:port`, the address defaulting to every interface if
/// `listening`, and required otherwise.
fn address_and_port(rest: &str, listening: bool) -> Result<(String, u16), String> {
    let rest = rest.trim_end_matches('/');
    let Some((address, port)) = rest.rsplit_once(':') else {
        return Err(format!("\"{rest}\" has no port (such as :5600)"));
    };
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| format!("\"{port}\" is not a port"))?;
    let address = match address {
        "" if listening => "0.0.0.0".to_string(),
        "" => return Err("A TCP source needs a host".to_string()),
        address if address.contains(['"', ' ', '/']) => {
            return Err(format!("\"{address}\" is not an address"));
        }
        address => address.to_string(),
    };
    Ok((address, port))
}

/// The UDP receive buffer: room for the burst of a keyframe, which the
/// kernel's default would partly drop.
#[cfg(not(target_os = "android"))]
const UDP_BUFFER_BYTES: u32 = 8 * 1024 * 1024;

/// How long a TCP-interleaved RTSP connection may stay silent.
#[cfg(not(target_os = "android"))]
const RTSP_TCP_TIMEOUT_US: u64 = 5_000_000;

/// The appsink the frames are taken from, in every pipeline.
#[cfg(not(target_os = "android"))]
pub(crate) const SINK: &str = "sink";

/// The RTSP source, in an RTSP pipeline, so its streams can be chosen.
#[cfg(not(target_os = "android"))]
pub(crate) const RTSP_SOURCE: &str = "source";

/// The GStreamer pipeline (in `gst-launch` syntax) receiving `source` as
/// `config` asks.
#[cfg(not(target_os = "android"))]
pub(crate) fn pipeline_description(source: &Source, config: &crate::Config) -> String {
    use crate::MIN_RETRANSMISSION_LATENCY_MS;

    // The jitter buffer is the latency dial: none in low-latency mode.
    let latency_ms = if config.low_latency {
        0
    } else {
        config.jitter_buffer_ms
    };
    // Frames show as decoded in low-latency mode, without waiting for
    // their presentation time; one frame at most waits for the viewer,
    // replaced by the next.
    let sink = format!(
        "videoconvert ! appsink name={SINK} sync={} max-buffers=1 drop=true \
         caps=\"video/x-raw,format={{NV12,I420}}\"",
        !config.low_latency
    );
    let decode = format!("decodebin3 ! {sink}");

    match source {
        Source::Rtsp { location } => {
            let retransmission = latency_ms >= MIN_RETRANSMISSION_LATENCY_MS;
            format!(
                "rtspsrc name={RTSP_SOURCE} location=\"{location}\" latency={latency_ms} \
                 drop-on-latency=true do-rtcp=true do-retransmission={retransmission} \
                 tcp-timeout={RTSP_TCP_TIMEOUT_US} udp-reconnect=true retry=3 \
                 protocols=udp+tcp ! {decode}"
            )
        }
        Source::Rtp {
            address,
            port,
            codec,
        } => {
            let (encoding, depay, parse) = match codec {
                Codec::H264 => ("H264", "rtph264depay", "h264parse"),
                Codec::H265 => ("H265", "rtph265depay", "h265parse"),
            };
            let jitter = if latency_ms > 0 {
                format!("rtpjitterbuffer latency={latency_ms} do-lost=true drop-on-latency=true ! ")
            } else {
                String::new()
            };
            format!(
                "udpsrc address={address} port={port} buffer-size={UDP_BUFFER_BYTES} \
                 caps=\"application/x-rtp,media=video,clock-rate=90000,encoding-name={encoding}\" \
                 ! {jitter}{depay} ! {parse} config-interval=-1 ! {decode}"
            )
        }
        Source::MpegTsUdp { address, port } => format!(
            "udpsrc address={address} port={port} buffer-size={UDP_BUFFER_BYTES} \
             caps=\"video/mpegts,systemstream=true\" ! {decode}"
        ),
        Source::MpegTsTcp { host, port } => {
            format!("tcpclientsrc host={host} port={port} ! {decode}")
        }
        Source::TestPattern => format!(
            "videotestsrc is-live=true pattern=smpte horizontal-speed=4 \
             ! video/x-raw,width=1280,height=720,framerate=30/1 ! {sink}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qgroundcontrol_uris_parse() {
        assert_eq!(
            Source::parse("udp://:5600"),
            Ok(Source::Rtp {
                address: "0.0.0.0".to_string(),
                port: 5600,
                codec: Codec::H264
            })
        );
        assert_eq!(
            Source::parse("udp265://192.168.1.10:5601"),
            Ok(Source::Rtp {
                address: "192.168.1.10".to_string(),
                port: 5601,
                codec: Codec::H265
            })
        );
        assert_eq!(
            Source::parse("rtsp://192.168.144.25:8554/main.264"),
            Ok(Source::Rtsp {
                location: "rtsp://192.168.144.25:8554/main.264".to_string()
            })
        );
        assert_eq!(
            Source::parse("tcp://10.0.0.2:5000"),
            Ok(Source::MpegTsTcp {
                host: "10.0.0.2".to_string(),
                port: 5000
            })
        );
        assert_eq!(Source::parse(" test:// "), Ok(Source::TestPattern));
    }

    #[test]
    fn malformed_uris_say_why() {
        assert!(Source::parse("5600").unwrap_err().contains("not a URI"));
        assert!(
            Source::parse("udp://0.0.0.0")
                .unwrap_err()
                .contains("no port")
        );
        assert!(
            Source::parse("udp://:0")
                .unwrap_err()
                .contains("not a port")
        );
        assert!(Source::parse("tcp://:5000").unwrap_err().contains("host"));
        assert!(Source::parse("http://x").unwrap_err().contains("Unknown"));
        assert!(Source::parse("rtsp://a b").unwrap_err().contains("spaces"));
    }

    #[cfg(not(target_os = "android"))]
    fn description(uri: &str, low_latency: bool) -> String {
        let config = crate::Config {
            low_latency,
            ..crate::Config::new(uri)
        };
        pipeline_description(&Source::parse(uri).unwrap(), &config)
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn low_latency_drops_the_jitter_buffer_and_the_clock() {
        let low = description("udp://:5600", true);
        assert!(!low.contains("rtpjitterbuffer"), "{low}");
        assert!(low.contains("sync=false max-buffers=1 drop=true"), "{low}");

        let buffered = description("udp://:5600", false);
        assert!(
            buffered.contains("rtpjitterbuffer latency=80 do-lost=true drop-on-latency=true"),
            "{buffered}"
        );
        assert!(buffered.contains("sync=true"), "{buffered}");

        let rtsp = description("rtsp://camera/main", true);
        assert!(rtsp.contains("latency=0 "), "{rtsp}");
        assert!(rtsp.contains("do-retransmission=false"), "{rtsp}");
        assert!(rtsp.contains("protocols=udp+tcp"), "{rtsp}");
        assert!(
            description("rtsp://camera/main", false).contains("do-retransmission=true"),
            "80 ms leaves time to ask for a lost packet again"
        );
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn rtp_is_depayloaded_for_its_codec() {
        let h265 = description("udp265://:5600", true);
        assert!(h265.contains("encoding-name=H265"), "{h265}");
        assert!(h265.contains("rtph265depay ! h265parse"), "{h265}");
    }
}
