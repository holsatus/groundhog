use num_enum::FromPrimitive;

/// https://github.com/ArduPilot/ardupilot/blob/master/ArduCopter/mode.h
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, FromPrimitive)]
pub enum Copter {
    Stabilize = 0,
    Acro = 1,
    AltHold = 2,
    Auto = 3,
    Guided = 4,
    Loiter = 5,
    Rtl = 6,
    Circle = 7,
    Land = 9,
    Drift = 11,
    Sport = 13,
    Flip = 14,
    Autotune = 15,
    Poshold = 16,
    Brake = 17,
    Thriw = 18,
    AvoidAdsb = 19,
    GuidedNogps = 20,
    SmartRtl = 21,
    Flowhold = 22,
    Follow = 23,
    Zigzag = 24,
    Systemid = 25,
    Autorotate = 26,
    AutoRtl = 27,
    Turtle = 28,
    #[num_enum(default)]
    Unknown = 255,
}

/// https://github.com/ArduPilot/ardupilot/blob/master/ArduCopter/mode.h
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, FromPrimitive)]
pub enum Plane {
    Manual = 0,
    Circle = 1,
    Stabilize = 2,
    Training = 3,
    Acro = 4,
    FlyByWireA = 5,
    FlyByWireB = 6,
    Cruise = 7,
    Autotune = 8,
    Auto = 10,
    Rtl = 11,
    Loiter = 12,
    Takeoff = 13,
    AvoidAdsb = 14,
    Guided = 15,
    Initialising = 16,
    Qstabilise = 17,
    QHover = 18,
    Qloiter = 19,
    Qland = 20,
    Qrtl = 21,
    Qautotune = 22,
    Qacro = 23,
    Thermal = 24,
    LoiterAltQland = 25,
    Autoland = 26,
    #[num_enum(default)]
    Unknown = 255,
}
