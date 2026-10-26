# Working on Groundhog

Groundhog is a MAVLink ground control station (GCS) written in Rust with the
[iced](https://github.com/iced-rs/iced) GUI toolkit. It flies real aircraft, so
the code favours being correct and easy to read over being clever.

This file is for **people and coding agents alike**. Read the first two
sections before changing anything; the rest is reference, to be read when you
touch the area it describes.

1. [Quick start](#quick-start)
2. [The architecture](#the-architecture)
3. [The layers](#the-layers)
4. [Design decisions](#design-decisions)
5. [Conventions](#conventions)
6. [Testing](#testing)
7. [Recipes](#recipes)
8. [Pitfalls](#pitfalls)
9. [Glossary](#glossary)

---

## Quick start

```sh
cargo run                                              # the application
cargo test --workspace                                 # every test; a few seconds after building
cargo fmt --all --check                                # CI gate
cargo clippy --workspace --all-targets -- -D warnings  # CI gate: warnings are errors
android/lint.sh                                        # CI gate: the library as built for Android
```

CI (`.github/workflows/ci.yml`) runs exactly the last four. **A change is done
when they pass**, the change has tests, and the docs it touched (module
docs, this file) still tell the truth.

The Android lint is separate because `platform/android.rs` and the
`android_main` export only compile for `target_os = "android"`; the desktop
run never sees them. It needs the NDK and
`rustup target add aarch64-linux-android` (see `android/build.sh` for both).

To try the application without a vehicle, run ArduPilot or PX4 SITL and add a
UDP link listening on the port it sends to (usually 14550). In listen mode a
UDP link serves every vehicle that sends to it.

Logs go to stderr through `env_logger`, at debug level for Groundhog's crates
and error level for dependencies (see `groundhog-gui-iced/src/platform/`).

Video needs GStreamer's development files and plugins (on Fedora,
`gstreamer1-devel gstreamer1-plugins-base-devel` and the plugins packages;
on Debian and Ubuntu, `libgstreamer1.0-dev
libgstreamer-plugins-base1.0-dev` and `gstreamer1.0-plugins-good`,
`-bad` and `-libav`). An H.264 decoder is the part distributions ship
apart: `gstreamer1.0-libav` (`avdec_h264`), a VA-API driver, or OpenH264.
The source `test://` shows colour bars without a camera; the video tests
skip what the installed plugins cannot do.

The frontend also runs on Android: `android/build.sh` builds a debug APK
(see `docs/android-port.md` for the toolchain and the iced fork it needs).
Logs then go to `adb logcat -s groundhog`.

---

## The architecture

The workspace has three crates, under `crates/`:

| Crate | What it is | Depends on |
|---|---|---|
| `groundhog-logic` | The backend: all vehicle state and every rule about it, the middleware, the transports, and the vocabulary frontends name it with (`SystemId`, `MavlinkId`, `GeoPosition`, link descriptions) | `mavio`, `tokio` |
| `groundhog-video` | Video from the vehicle: a stream's URI in, decoded frames out, at low latency (see `docs/video.md`) | GStreamer on the desktop, `tokio` |
| `groundhog-gui-iced` | The frontend, built as the `groundhog` binary on the desktop and as a `cdylib` on Android | `groundhog-logic`, `groundhog-video`, `iced` |

Another frontend (a CLI, say) is another crate next to these, depending on
`groundhog-logic`. Paths in this file are relative to a crate's `src/`;
the crate is named where it is not obvious.

```
 gui-iced  features/  UI features: panels and map plugins             ┐
           ui/        the toolkit: tokens, styles, icons, widgets     │
           map/       map engine: layers, hit-testing, gestures       │
           layout.rs  the tiled shell's workspace                     ├─ UI thread (iced)
           shell.rs   which shell; the floating shell's sheet         │
           app.rs     composition root, message routing, policy       │
           platform/  paths, logging, insets: desktop or Android      ┘

 logic     services/  Backend, fleet runtime, vehicle client, links   ┐
           domain/    vehicle state, missions, parameters, firmware,  │
                      calibration                                     │
           mavlink/   router and transactions (private)               ├─ async runtime (tokio)
           link/      transports: TCP, UDP, serial (private)          │
           wire/      pure codecs (private)                           ┘
           storage/   file formats (missions, parameters)
           persist.rs safe writes of a frontend's own files
           lib.rs     SystemId and MavlinkId, shared by every layer

 video     receiver   a GStreamer pipeline per attempt, supervised    ┐
           frame      the newest frame, for the frontend to draw      ┘ own threads
```

How data moves:

1. **Transports** (`link/`) are byte streams. The router owns them once
   attached.
2. **The router** (`mavlink/router.rs`) parses frames, drops cross-link
   duplicates, routes command ACKs and transaction replies, and hands every
   other frame to one **lossless** channel. Outbound frames go through
   per-link priority queues.
3. **The fleet runtime** (`services/runtime.rs`) is a task that *owns* the
   vehicle state (`domain::fleet::Fleet`). It folds every message in, ages
   vehicles for liveness, and bootstraps new vehicles. At most every 33 ms it
   publishes an immutable `FleetSnapshot`, plus the `FleetEvent`s since the
   last one.
4. **The frontend** (`app.rs`, `features/`) starts one `Backend` at boot,
   keeps the latest snapshot, and holds only *operator* state: mission
   drafts and parameter edits (modelled by the logic crate's `MissionDraft`
   and `ParamEdits`), the selected vehicle, view state. It changes
   vehicles only through operations on `services::VehicleClient`, never by
   mutating state. Results that change what a vehicle holds (a transferred
   mission, a parameter list) flow back through the runtime, so every reader
   sees them.

Vehicle state therefore keeps advancing when the UI is busy or stalled, and the
UI can never observe a half-applied update.

---

## The layers

A layer may use the layers listed below it in the diagram; never one above.
`SystemId` and `MavlinkId` (logic `address.rs`, exported at the crate root)
are shared by all.

| Module | Owns | Depends on | Must not |
|---|---|---|---|
| logic `link/` | Link descriptions (`LinkId`, `LinkConfig`, `LinkBuild`, public); opening transports (private) | `mavio`, `tokio` | know about MAVLink messages |
| logic `wire/` | Pure codecs (parameter encodings, coordinate scales) | `mavio` | hold state |
| logic `mavlink/` | Router, duplicate suppression, transactions, outbound queues | `link`, `wire`, `mavio`, `tokio` | know about vehicles as state, or the UI |
| logic `domain/` | Vehicle state and its folding, positions (`geo`), missions, parameters, firmware adapters, calibration protocols; the operator models that are compared with vehicle state (`MissionDraft`, `ParamEdits`) | `wire`, `mavio` types as vocabulary | do I/O, spawn tasks, call the middleware |
| logic `storage/` | File formats (WPL missions; INI/`.param`/`.params` parameters) | `domain` | touch the filesystem (callers do) |
| logic `persist.rs` | Writing a frontend's own files: atomic replacement, setting unparseable files aside, ordered saves (`SaveQueue`) | `tokio` | know what is stored, or its format |
| logic `services/` | `Backend`, fleet runtime, `FleetHandle`, `VehicleClient`, `LinkManager`, tlog recording | `mavlink`, `domain`, `link` | expose middleware machinery (router, frames, transactions) in its API. Plain data it re-exports, like `TransferProgress` and `LinkStats`, is fine |
| gui `map/` | The map engine: layers, projection, pointer state machine, caching | iced, `slippery`, `domain::geo` | know about vehicles or missions |
| gui `features/` | One module per UI feature | `services`, `domain`, `map`, `storage`, `ui` | each other's state; style raw iced widgets |
| gui `ui/` | The toolkit over iced: design tokens, `Tone`s, every style function, the icon set, and the constructors views build widgets with | iced | know about features or the domain |
| gui `layout.rs`, `shell.rs` | Where panels go: the tiled workspace, which shell is on screen, the floating shell's sheet | `ui`, `widget` | know what a panel shows |
| gui `app.rs`, `app/` | Wiring at boot, message routing, effects, notice policy; each shell's view (`app/tiled.rs`, `app/floating.rs`) over the same `pane_body` | everything | contain feature logic |
| gui `platform/` | What differs between the desktop and Android: file locations, logging, the system bars' insets and icon colour, `TOUCH` | iced, `android-activity`, `jni` | know about the interface |
| gui `widget/` | Reusable widgets (vendored `pane_grid` fork, `virtual_list`, `drag_source`, `surface`, `video`, attitude) | iced | depend on the domain |
| video | `Receiver` (URI to frames, its `State`), `FrameSlot`, `Frame` | GStreamer, `tokio::sync` | know about vehicles or the frontend |

The crate boundary enforces most of the table: `mavlink`, `wire` and the
transports are private to `groundhog-logic`, so no frontend can reach the router, and
the methods that fold messages into vehicle state are `pub(crate)`, so a
frontend cannot advance a vehicle behind the runtime's back.

The domain deliberately uses `mavio` *types* (`MavCmd`, `MavFrame`, decoded
messages) as its vocabulary: a MAVLink GCS gains nothing from re-declaring the
MAVLink enums. Frontends use them too, from the same workspace dependency.
What a frontend must not use is the *middleware*, and it cannot.

---

## Design decisions

Each decision below records what was chosen and why, so you can tell when a
change would undo it.

### The backend and the frontends are separate crates

`groundhog-logic` holds every piece of vehicle state and every rule about it;
a frontend holds presentation and operator state, and names vehicle state
with the logic crate's own types. A frontend starts the backend with
`Backend::start(BackendOptions { links, .. })`, which wires the router, the
fleet runtime and the link supervisors together and returns the
`BackendEvents` streams to drive. From then on it reads snapshots through
`backend.fleet()`, manages links through `backend.links_mut()`, and starts
recordings with `backend.record(directory)`. Dropping the `Backend` stops
all of it: the links and the fleet runtime.

*Why:* a second frontend (a CLI, a headless logger) must get the same
behaviour without re-implementing any of it, and the compiler, not review,
should keep the UI from reaching into the middleware or mutating vehicle
state. Anything a frontend computes from vehicle state that another frontend
would compute the same way belongs in the logic crate: the calibration panel,
for example, asks `Vehicle::calibration_kinds` and
`Vehicle::calibration_controls` instead of the firmware's command builders.

The same goes for operator state whose rules every frontend would share:
`domain::mission::MissionDraft` decides when a draft follows its vehicle
and how it relates to it, and `domain::parameter::ParamEdits` folds typed
text, loaded files and upload results into pending edits. A frontend keeps
these in its own maps, next to what only it needs (selection, half-typed
field text, which operations run). Events carry what they replace
(`FleetEvent::LivenessChanged { from, to }`, `LinkEvent::previous`), so a
frontend can word a notice without mirroring state to diff against.

What stays in each frontend is shaped by its toolkit or its operator: the
`Operations` set (its tasks are the toolkit's: iced `Task`s, or
`tokio::spawn` with abort handles), message or intent plumbing, widgets,
map layers, and the wording and loudness of notices. The backend must not
decide those, because a headless client or a differently structured UI
would decide them differently.

A second frontend keeps its own configuration file, so two frontends can
run at once without racing each other's writes.

### Vehicle state is owned off the UI thread

`services/runtime.rs` owns the `Fleet` and publishes `Arc<FleetSnapshot>`.
Snapshots are cheap: vehicles sit behind `Arc`, and heavy subsystems (trail,
status log, per-component parameters) sit behind their own `Arc`, so a
mutation copies only the subsystem it touches (`Arc::make_mut`).

*Why:* a GCS must keep tracking liveness, recording and folding telemetry
while the UI is busy, a file dialog is open, or a frame takes long. Snapshots
also make the UI a pure function of data, which makes it easy to test.

### Vehicle state and operator state are separate

The fleet holds only what vehicles *reported*. Mission drafts
(`domain::mission::MissionDraft`) and parameter edits
(`domain::parameter::ParamEdits`) are held by the frontend, in
`features::mission` and `features::parameter_panel`; the fleet never sees
them.

*Why:* an operator's draft must never be overwritten by telemetry, and
planning must work with no vehicle at all (the offline plan). Whether a draft
matches the vehicle is **derived** (the draft compared with the onboard
mission), never tracked with a dirty flag.

### Lossless router events; duplicates identified exactly

The router's event channel applies backpressure instead of dropping frames.
Duplicates (the same packet heard on two links) are identified by
`(sequence, message id, checksum)`, not by sequence alone: autopilots keep one
sequence counter *per channel*, so different packets on two links share
sequence numbers all the time. Routes are refreshed only by the first copy of
a packet, so commands take the faster link.

### Transactions: listen first, serialize per resource, drop means cancel

Every transaction (`mavlink/transaction.rs`, `param.rs`, `mission.rs`):

- installs its reply filter **before** sending, so a fast reply cannot be
  missed;
- takes a `LockKey` for the resource, because MAVLink replies carry no
  transaction id;
- cleans up when its future is dropped. A command in progress gets
  `COMMAND_CANCEL`; a mission transfer gets `MAV_MISSION_OPERATION_CANCELLED`.

Mission transfers only accept messages addressed to this GCS and of their own
mission type, because another GCS, or a fence transfer, may be talking to the
same vehicle.

### Firmware differences live behind one trait

`domain::firmware::Firmware` (ArduPilot, PX4, generic) answers:

- which flight modes exist, which of them a given vehicle offers (its
  parameters can rule some out), and how to command one;
- how parameters are encoded;
- how the mission is laid out (ArduPilot keeps home in mission slot 0);
- how sensors are calibrated (`Firmware::calibration`).

Code outside `domain/firmware/` never matches on `MavAutopilot`; it asks
`vehicle.firmware()`.

### Calibrations are vehicle state, run by a firmware protocol

MAVLink standardises little about calibration. ArduPilot and PX4 share
`MAV_CMD_PREFLIGHT_CALIBRATION` for some sensors but not all (airspeed is
`param3` on ArduPilot and `param6 = 2` on PX4; ArduPilot's compass needs the
ArduPilot dialect's `DO_START_MAG_CAL`), and they report progress completely
differently: ArduPilot with `ACCELCAL_VEHICLE_POS` handshakes and
`MAG_CAL_PROGRESS`/`MAG_CAL_REPORT`, PX4 with `[cal]` status texts.

So `domain::calibration::Session` is the firmware-neutral view (phase, what to
do now, progress, sides done, per-compass results), and a
`CalibrationProtocol` per firmware (`domain/calibration/{ardupilot,px4}.rs`)
gives the start, confirm and cancel commands and folds the vehicle's feedback
into the session. The session lives on the `Vehicle`, so the runtime advances
it as feedback arrives, and the UI (`features/calibration.rs`) only starts,
confirms and cancels through `VehicleClient` and renders what the session and
`Vehicle::calibration_controls` say. Finished runs are announced as `FleetEvent::CalibrationFinished`.

### Missions are stored as they travel on the wire

`domain::mission::MissionItem` holds the wire fields verbatim, so every item
round-trips losslessly, including commands the editor does not understand. The
table `domain::mission::command::SPECS` adds meaning: labels, units, widgets,
defaults, which parameters reference other items.

Plan items have stable `ItemId`s. Every message that targets an item uses its
id, never its index, so a list that changes underneath an in-flight edit or
drag cannot redirect it.

Plans never contain ArduPilot's home item. `domain::mission::codec` inserts
and removes it, and shifts jump targets accordingly, based on the firmware's
`MissionLayout`.

### Parameter writes are refused while the encoding is unknown

Writing a value with a guessed encoding can store something else entirely: an
integer written bytewise to a C-cast autopilot arrives as zero. The encoding
comes from the component's advertised capabilities, else from its firmware,
else it is unknown, and `VehicleClient::set_parameter` returns
`Error::ParamEncodingUnknown`. Stored values keep their raw wire form, so they
are re-decoded if the encoding is learned later.

### The map engine knows nothing about features

Features contribute type-erased `map::MapLayer`s through `map::MapPlugin`.
The engine (`map/overlay.rs`) owns all pointer state in the canvas widget's own
state: hover, press, drag (which keeps the map from panning), click detection,
and escape-to-cancel. A drag can therefore never get stuck through lost
events, and no application state is needed to track one. A layer reporting a
`revision` is drawn from cache until that number, the view or the theme
changes.

### Features talk to the application through a narrow contract

A feature's `update` gets a `features::Context`, holding:

- the snapshot;
- the services;
- the notices;
- settings and the file-dialog directory.

It requests anything else as an `Effect`: confirm, select a vehicle, move the
map, remember a directory. Features never reach into each other or into the
application.

### The workspace is three columns around the map

`layout.rs` keeps a `Workspace`: a left and a right column that stack panes
top to bottom, and a row beneath the map that tiles them left to right. The
map is fixed in the middle. A rail button opens its panel at the end of the
left column (click), the row (middle-click or Ctrl/Cmd-click) or the right
column (right-click or Shift-click), or wherever its button is dragged
onto the grid; a panel joining `n` others takes
`1/(n + 1)` of its stack and the others keep their proportions. Clicking
an open panel's button closes it. A stack holds only as many panes as fit
at `MIN_PANE`: a click into a full stack opens the panel in the next with
room (the right column, the left, the row), and a full stack is no drop
target. A window that shrinks keeps the panes it has, squeezed, and the
sizes it squeezes are not saved.

Dragging a pane moves it within this shape, and the highlight shows only
what the shape allows, where the pane would be laid out if it were added
to the layout on screen (which still holds it in its old place)
(`Layout::drop_hint`, plugged into the vendored grid with
`PaneGrid::drop_target`): dropped in the middle of a pane, the two swap
places and sizes; on the rest of one half of a pane (top or bottom in a
column, left or right in the row), it splits that pane's share in two;
dropped on the divider between two panes, it takes an even share of the
stack; dropped on the map's left or right quarter, it opens that column,
and anywhere else on the map, the row, but only while that stack is empty
(a stack with panes is joined beside them).

A rail button dragged onto the grid lands by the same rules
(`Layout::incoming_hint`): the button is a `widget::drag_source`, which
reports when the drag starts and ends and carries a label for the panel
(its icon and name, styled as a tooltip) centred above the cursor; while it lasts the application
hands the grid the hint with `PaneGrid::incoming`, so the grid highlights
and reports the drop as it does for its own panes. An open panel moves as
if its pane were dragged; a closed one opens, and since it has no place to
swap, the middle of a pane splits it like the rest of its half.

The columns' widths and the row's height are pixels, so resizing the window
resizes the map. The `Workspace` is the persisted truth: the pane grid's
state is rebuilt from it (`State::rearrange`, in the vendored grid, keeps
each surviving pane's identity and so its widget state) on every structural
change and every change of the grid's size, which a `sensor` reports. A
dragged divider changes only what it bounds in the workspace (a column's
width, the row's height, or the shares of the two panes beside it), clamped
so the map keeps its least, and the grid is rebuilt from that during the
drag (`State::rearrange` also keeps each split between the same two panes,
which the drag holds). So the grid never shows a size the workspace does
not hold.

*Why:* free splitting made every new pane take half of whatever it split,
and a layout that tracks fractions makes the side panels grow and shrink
with the window, where the map is what should.

### Two shells over the same panels

A panel's content is the same wherever it is shown
(`Application::pane_body`); a *shell* decides only where panels go
(`shell.rs`):

- **Tiled** (`app/tiled.rs`): the rail and the workspace above, for a mouse
  and a large screen.
- **Floating** (`app/floating.rs`): a phone's navigation by task. A
  navigation bar docked along the bottom (a rail along the left in
  landscape) picks one of five `shell::Destination`s: Fly, Plan, Setup,
  Links and More, each holding panels. Fly and Plan are the map, running
  under the status bar, with their panel in a sheet over it, along the
  bottom in portrait or along the side in landscape
  (`shell::SheetPlacement`), or over the whole map when expanded. The
  others are pages: a top bar, then the panel, or a list of the
  destination's panels (`ui::entry` rows) that opens each as a page with a
  way back. The system's back gesture (the back key; also Escape and a
  mouse's back button) first dismisses an open confirmation, in either
  shell; then it steps out one level at a time (`Floating::back`), and
  from the bare map sends the application to the background, as a phone
  app does. A scrim under the status bar keeps its icons legible
  over any tile. Where the operator is, is view state; nothing about the
  floating shell is persisted.

`Destination::panels` is the one list of panels and their order: the
tiled shell's rail lists them in that order too, evenly, without marking
the groups.

`ShellMode` (a setting) picks one, or leaves it to `Automatic`: floating on
a touch screen whose shortest side is under 600 logical pixels, and in any
window under 640 wide. *Why the shortest side:* rotating a phone does not
change it, so turning the phone keeps the shell and the panel open in it,
where a width threshold swapped the whole layout on every rotation.

The platform's insets (`platform::Insets`, polled over JNI on Android, zero
on the desktop) are given to the shells rather than padding the window:
the tiled shell keeps its panes clear of them, and the floating shell lets
only the map run under them.

Whatever floats over the map is a `widget::surface`, which captures the
presses (and wheel) that land on it. The map follows a finger by the
finger's own position, not the cursor, so without one a press on a sheet's
background would pan the map beneath. Moves and releases pass through, so
a gesture that started on the map ends on the map.

The map is told how much of each edge is covered (`map::Placement`):
screen-anchored layers keep to `MapView::visible` (the attitude HUD clears
the status bar), and `Map::center_on` puts a position in the middle of
what shows, not under the sheet.

### Failures reach the operator; consequential commands ask first

Every failed operation becomes a `Notice`: a toast, plus the log panel's
history. `services::Error` keeps the *reason* (`Rejection::TemporarilyRejected`,
`MissionRejection::NoSpace`, ...) so the notice is actionable.

Commands that change what a vehicle does ask the operator first through a
`Confirmation`: arming, starting a mission, jumping to an item, clearing the
vehicle's mission, and changing its mode (returning and landing included).
A mode change asks while the vehicle is armed or its armed state is
unknown, wherever it comes from (the mode picker or a quick button); a
disarmed vehicle does not act on its mode until it is armed, and arming
asks.

Which mission actions ask, and which are refused, is decided in one place,
`MissionDraft::check`: resuming asks like starting, and both say when the
vehicle flies a mission other than the plan shown; an upload asks while the
vehicle is armed (or may be), when the plan is empty (it deletes the
vehicle's mission), and when the vehicle's mission was changed elsewhere;
pausing, a safety stop, never asks.

### Links are supervised

`services::LinkManager` spawns a supervisor per connected link. The supervisor
opens the transport, waits for it to fail, and reconnects with backoff, unless
the link is configured not to. The status the UI shows is the supervisor's own
(read through a `watch`), never a copy that needs reconciling.

---

## Conventions

### Code

- **Format and lint:** `rustfmt` defaults; clippy with `-D warnings`. The
  lint set is in the root `Cargo.toml` (`[workspace.lints]`, opt in per crate
  with `[lints] workspace = true`), not on the command line, so plain
  `cargo clippy` enforces it; the few settings those lints read (test-code
  exemptions, in particular) are in `clippy.toml`. `clippy::pedantic` is
  deliberately **not** enabled: measured against this codebase it was 42%
  `#[must_use]`-style annotation and 25% `as`-cast objections, which prevents
  nothing. What is enabled is what the code already satisfies and should
  never stop satisfying: no `unwrap`/`panic!` outside tests (a panic in the
  fleet runtime freezes every vehicle's state silently), no `unsafe` outside
  the JNI module, no `let _ =` on something with a destructor (say
  `drop(..)` instead), no `process::exit` (it skips the log flush), no
  `print!` (Android discards stderr), no byte-slicing of strings.
- **Silencing a lint:** fix the cause. If the lint is genuinely wrong for
  this code, write `#[expect(clippy::lint, reason = "…")]` — never
  `#[allow]`: `expect` stops compiling once it is unneeded, so it cannot rot.
  Every attribute needs a `reason` (`allow_attributes_without_reason` is an
  error). Scope it as narrowly as the reason allows: an expression, an item,
  a `mod`. Test code is exempt from the panic-family and print lints in one
  place, `clippy.toml` (`allow-panic-in-tests`, `allow-unwrap-in-tests`,
  `allow-expect-in-tests`, `allow-print-in-tests`), because a test's job is
  to fail loudly; production code keeps them denied. The vendored
  `widget/pane_grid` carries module-wide exceptions with their reasons in its
  header.
- **No dead code.** Delete unused items, or gate test-only helpers with
  `#[cfg(test)]`. The frontend is a binary crate, so the compiler finds them
  there. In the library crates it only finds unused items that are not
  exported, so keep their public API to what a frontend needs: give
  backend-internal items `pub(crate)` (which the compiler checks again), and
  remove a `pub` item when the last frontend use goes. Both library crates
  enable `unreachable_pub`, so a `pub` item that no frontend can reach (one
  in a private module, not re-exported) is an error under CI's
  `-D warnings`.
- **Dependencies:** one crate's dependency goes in that crate's manifest.
  Only a dependency more than one crate uses goes in the root's
  `[workspace.dependencies]`, so they agree on its version. iced, in
  particular, is a dependency of `groundhog-gui-iced` alone.
- **Visibility is the seam.** The middleware, transports and wire codecs are
  private modules of `groundhog-logic`, and everything that advances vehicle
  state (`handle_message`, the `observe_*` folds, session updates) is
  `pub(crate)`. A frontend that needs one of these needs a new backend
  operation instead.
- **Panics:** none in non-test code, except `expect("why this holds")` on a
  proven invariant. The release profile keeps `overflow-checks = true`, so
  arithmetic that would wrap panics instead: an overflowed mission index or
  timestamp is a wrong command, not a warning.
- **Units in names:** `alt_m`, `hold_s`, `yaw_deg`. Convert units exactly:
  `millivolts as f32 / 1000.0`, not `* 1e-3`, which rounds 12000 mV to
  12.000001 V.
- **Identities:** a vehicle is a `SystemId`. A `MavlinkId` is an *address*
  (system plus component) for sending and receiving. Mission items are
  `ItemId`, links `LinkId`, map layers `LayerId`.
- **MAVLink "unknown" sentinels** become `None` at the boundary: `UINT16_MAX`
  volts, `-1` current, NaN yaw meaning "vehicle default".

### Comments

- Every module starts with a `//!` doc saying what it owns and, where it
  matters, why it is shaped that way.
- Public items get `///` docs describing behaviour and contract.
- Comments explain *why*. Never write history ("used to", "was moved
  from", "as before"): that belongs in commit messages, and goes stale in code.

### The UI (iced)

The frontend tracks iced's git `master`, where some APIs differ from the released
versions:

- Convert widgets to `Element` with `.boxed()` (from `iced::Widget`, which must
  be in scope); there is no `From` conversion. `Element::map(...)` returns a
  widget, so follow it with `.boxed()` too.
- `text_input(placeholder, value)` takes an owned `String` for a computed value.
- **Messages must be `Clone + Debug`, and their `Debug` must be cheap.** When
  iced's `debug` feature is enabled, every message is pretty-printed on the UI
  thread before `update`. Give messages that carry
  large data a summary `Debug` (see `FleetUpdate`).
- Long lists use `widget::virtualized::virtual_list` with fixed row heights.
  Text in a fixed-height row must be single-line (`TextExt::single_line`).
- Never block the UI thread. File I/O goes through
  `iced_runtime::task::blocking`, network work through services. `boot` and
  `update` run inside the async runtime, so `tokio::spawn` works there.
- Colours come from the theme palette (`ui/style.rs`), except map overlays,
  which use fixed colours that read well over any map tile.

### The toolkit (`ui`)

Views build widgets through `crate::ui`, never by styling raw iced widgets,
so the look is decided in one place:

- **Tokens:** gaps from `ui::space` (`XXS`…`XL`), sizes from `ui::size`.
  No numeric spacing, padding or text size appears in a view. Sizes a
  finger needs larger are functions of the `ui::Density`
  (`size::control()`, `size::list_row()`): 32 and 26 pixels for a pointer,
  44 for touch. The application sets the density before each view
  (`ui::set_density`): touch in the floating shell, or everywhere when the
  operator asks for it. Read a density-dependent size while building the
  view, never inside a closure that runs later (`let row =
  ui::size::list_row(); .item_height(move |_| row)`).
- **Space, not devices.** The toolkit arranges by the width it is given,
  so a narrow desktop pane and a phone look the same: `ui::field` puts its
  label above the control when they do not fit side by side, `ui::stats`
  shows as many columns as fit, and a header folds its tools into a menu.
  Do not fix widths in a feature where the content could give way
  instead.
- **Text:** `ui::text`, `ui::caption`, `ui::title`, `ui::mono`, and
  `ui::overline` for section headings; colour by meaning with
  `.tone(Tone::Warning)` (`TextExt`), never a palette slot, so a view needs
  no `&Theme`.
- **Buttons** are built: `ui::button("Arm").primary().busy(arming, "Arming…")
  .on_press(message).boxed()`. `.busy` swaps the label and refuses presses
  while an operation runs; `.chosen(bool)` highlights a selected row or an
  active mode; `.hint(..)` adds a tooltip; `.icon(Icon::Download)` puts an
  icon before the label. Labelled and icon buttons share one height. The
  builder is for actions in views; window chrome that looks nothing like
  them (the rail) builds its own buttons from a style in `ui/style.rs`
  (`rail_button`).
- **Icons** are named, never loaded: `ui::icon_button(Icon::Upload)`. To add
  one, put a Lucide SVG in `assets/icons/` and a line in the `icons!` list in
  `ui/icon.rs` (see `assets/icons/README.md`).
- **Layout:** `ui::panel` is every pane's body (`ui::panel_with` puts a
  header above it), `ui::empty` a panel with only a reason. `ui::header`,
  `ui::actions`, `ui::activity`, `ui::badge` and `ui::banner` are the parts
  of the panel anatomy below; `ui::card`, `ui::card_list` (rows split by
  edge-to-edge rules), `ui::section`, `ui::field` (a form row), `ui::stat`
  and `ui::stats` (read-outs in columns), and `ui::list_row` (striped)
  cover the shared arrangements. A panel whose body is a picture (the
  video) uses `ui::picture`, edge to edge inside the pane's border, with
  what it needs floating over it in `ui::floating` cards rather than
  around it. Inputs come from `ui` too (`text_input`,
  `pick_list`, `checkbox` and `unlabelled_checkbox`, `slider`), so they
  take the density's size.

### The design language

Every pane looks alike, and each is laid out for its own work. The look is
shared; the structure follows the panel's workflow.

**One surface.** A pane is one flat surface. Structure comes from space and
type, not from boxes: sections are set apart by an overline heading and
`space::L`, not by borders. A card (an outline on the same surface) is for
an *object* the operator acts on as a whole: a saved link, a calibration
run, the mission item being edited. Never wrap a group of controls in a
card just to group them.

**The anatomy of a pane**, top to bottom, each part optional:

1. **Header** (`ui::header`, via `ui::panel_with`): what the panel is about
   (a vehicle or component picker, a name, a filter) and how it stands
   (badges), then its *tools* (`ui::tool`): actions on things the panel
   holds as a whole (files, adding items), each an icon and a label. Where
   there is room they are quiet icon buttons, the label their hint; where
   there is not, they fold into a menu that lists them labelled. One line,
   divided from the body.
2. **Actions** (`ui::actions`): groups of buttons in workflow order, a
   group per kind of work (talking to the vehicle; flying; editing).
   Groups wrap whole. Hide a group that cannot apply (no vehicle) rather
   than disabling all of it.
3. **Banners** (`ui::banner`): something that needs attention now: a recent
   alert, a blocker, a failure. Only while it is true.
4. **Activity** (`ui::activity`): a transfer or download under way, with
   its progress and a cancel. Only while it runs.
5. **Body**: the panel's content, scrolling on its own (lists keep their
   own scrollable).

**Buttons.** At most one primary button in view, and only while it is the
next thing to do (Upload when there are changes, Arm). Repeated per-row
actions and rare ones are icon buttons with a hint; verbs of the main
workflow are labelled, with an icon when the verb recurs across panels
(download, upload, start). Danger is for stopping something live
(Disarm); a destructive stored change asks through a `Confirmation`
instead of shouting.

**Status.** A state is a `ui::badge` in its tone (Synced, Not uploaded,
Active, Armed, a sensor's health); a value is text, coloured only when its
colour means something. Read-outs put a muted caption above the value
(`ui::stat`); forms put a muted label before the control (`ui::field`).

When a second view would build something the same way, add it to `ui`
rather than copying it.

### Operations from features

Run every vehicle operation through `features::operations::Operations`, keyed
by what it is about. That gives busy state, cancellation (dropping the task
cancels the transaction), progress reporting, and stale-result filtering: a
result whose `Ticket` is no longer current is ignored. In the result handler,
call `finish(&key, ticket)` first and drop the result if it returns `false`.

### Safety defaults

- Never derive a value that commands the vehicle from something that is
  usually zero. New waypoints take the previous waypoint's altitude, or the
  configured default, never the vehicle's current altitude (zero on the ground).
- When something cannot be known (an encoding, a home position), refuse or
  ask. Do not guess.
- Ask for confirmation before any command that changes what the vehicle does
  (for a mode change: while the vehicle is armed or may be).

---

## Testing

All tests run with `cargo test --workspace`; most finish in microseconds. Every bug fix
comes with a test that fails without the fix.

| What | Where | How |
|---|---|---|
| Middleware transactions | logic `mavlink/tests.rs` | `MockVehicle` over `tokio::io::duplex`; script the vehicle's side |
| Router internals | logic `mavlink/router.rs` tests | call `handle_frame` with built frames; inspect the route table |
| End to end | logic `services/tests.rs` | real router, runtime and `VehicleClient` against a mock vehicle; `until(...)` waits for a snapshot condition |
| Domain | logic, next to the code | plain unit tests; fold messages with `Fleet::observe` / `Vehicle::handle_message` |
| Map gestures | gui `map/overlay.rs` tests | drive the canvas `Program` with synthetic mouse and keyboard events |
| Widget composition | gui `map/mod.rs` tests | `iced_test::Simulator` delivers real events through the composed widgets; assert on event status and messages |
| Features | gui `features/*/tests.rs` | build a `Context` with a synthetic `Fleet` snapshot and a real `FleetHandle`; assert on state, notices and requested effects |

Feature tests need a `FleetHandle`, so they are `#[tokio::test]` and start a
backend with `BackendOptions { bootstrap: false, .. }`; the snapshots they
hand to the feature come from a `Fleet` they fold messages into with
`Fleet::observe`. The folding methods and `Fleet::test_begin_calibration` are
public only under `groundhog-logic`'s `test-support` feature, which the
frontend enables from its dev-dependency. To simulate an operation's
result, take its ticket with `Operations::ticket_of` (test-only).

Tests must not touch the user's files. Write into a temporary directory, or
point `XDG_CONFIG_HOME` and `XDG_DATA_HOME` somewhere else.

`iced_test` (a dev-dependency) runs widgets headlessly. Its `Simulator` can
deliver events through any `Element` (see the map's tests), and render the
whole `Application::view()` to PNG with `snapshot(..).matches_image(path)`.
`matches_image` only writes the PNG when no file exists yet (the name gains a
renderer suffix such as `-wgpu`); otherwise it compares against the existing
one. Delete old snapshots before re-rendering.

To *look at* the interface, render the gallery: every panel, the whole
window in the tiled shell, and a phone in the floating shell in both
orientations, light and dark, with a synthetic vehicle (`app/gallery.rs`).
It is
an ignored test that needs its own configuration directory:

```sh
GROUNDHOG_GALLERY=/tmp/gallery XDG_CONFIG_HOME=/tmp/gallery/config \
    XDG_DATA_HOME=/tmp/gallery/data cargo test -p groundhog-gui-iced gallery -- --ignored
```

Render it before and after a change to the look, and compare.

---

## Recipes

### Handle a new MAVLink message in vehicle state

1. Add the field to the right subsystem in `domain/vehicle/`, or create one.
   Put it behind `Arc` if it is large and changes often.
2. Add a fold method to it (`observe_*`), and an arm in
   `Vehicle::handle_message`. Guard vehicle-wide state with
   `if self.is_authoritative(component)`.
3. If the UI should react to a change, add a `FleetEvent` in
   `domain/fleet.rs` and decide its notice in `Application::on_fleet_event`.
4. Test the fold in the subsystem, and the event in `fleet.rs`.

### Add a vehicle operation

1. If the command is new, add a builder to `mavlink/commands.rs`.
2. Add an `async fn` to `services::VehicleClient`. Read wire details from the
   snapshot (`with_vehicle`), and map errors with `Error::from`. If the
   result changes what the vehicle holds, `report` it to the runtime.
3. In the feature: add a request message, which may confirm first, and a
   `Finished(key, Ticket, Result<..>)` message. Run the operation through
   `Operations`, and push a `Notice` on failure.
4. Test the client end to end in `services/tests.rs`, and the feature flow
   in its tests.

### Draw something on the map

Implement `map::MapLayer` for a small struct that borrows your feature's state,
and return it from your feature's `MapPlugin` implementation:

```rust
struct Fences<'a> { fence: &'a Fence }

impl MapLayer<Message> for Fences<'_> {
    fn id(&self) -> LayerId { LayerId::new("fence.polygon") }       // stable and unique
    fn z_index(&self) -> i32 { 8 }                                   // see the table below
    fn revision(&self) -> Option<u64> { Some(self.fence.revision()) } // cached until it changes
    fn draw(&self, frame: &mut MapFrame<'_>, _: Interaction) {
        frame.polyline(self.fence.vertices(), Stroke::default());
    }
    // Optional: interactive handles and what their gestures mean.
    fn handles(&self, _: &MapView) -> Vec<Handle> {
        self.fence.vertices().enumerate()
            .map(|(i, at)| Handle::at(HandleId(i as u64), at, 10.0).draggable())
            .collect()
    }
    fn on_event(&self, event: &LayerEvent) -> Option<Message> {
        match *event {
            LayerEvent::Drag { handle, at } | LayerEvent::DragEnd { handle, at } =>
                Some(Message::MoveVertex(handle.0 as usize, at)),
            _ => None,
        }
    }
}
```

Then add `layers.plugin(&self.features.fence, context, Message::Fence)` in
`Application::pane_body`. Layers in use, by z-index:

| z | Layer |
|---|---|
| `i32::MIN` | map tint (engine) |
| 0 | trail casings: body (cached), tail |
| 1 | trail cores: body (cached), tail |
| 5 | home markers |
| 10 | mission route |
| 20 | vehicle icons (clickable) |
| 30 | mission markers (draggable) |
| 100 | attitude HUD (screen-anchored, clickable) |
| `i32::MAX` | pane frame (engine) |

Higher layers are hit-tested first. Use `Anchor::Screen` for handles fixed to
the overlay rather than to the map.

### Support another autopilot

Implement `domain::firmware::Firmware` in a new file under `domain/firmware/`:
mode tables per `VehicleClass`, parameter encoding, mission layout. Add it to
`firmware::for_autopilot`. Test the mode decoding and the set-mode encoding.

### Support another calibration

Add a `CalibrationKind` (title, preparation, first instruction) and handle it
in each protocol's `start`, `supports` and `observe`; `supports` gets the
vehicle's class, so a kind only some vehicles can run (airspeed needs wings)
is offered only to those. Feedback a protocol does not understand yet can
still go to the session's log with `Session::note`.
Test the start parameters per firmware and the feedback sequence, as
`domain/calibration/tests.rs` does; `services/tests.rs` drives a whole run
against a mock vehicle.

### Support another mission command

Add one `CommandSpec` to `domain::mission::command::SPECS`, following
`common.xml` for the parameter meanings. Mark it `.flown_at_z()` when its `z`
is the altitude the vehicle flies at, so new waypoints may follow it; a
landing's or a region of interest's `z` must not. The editor, the map (for
commands with a location), the file formats, and jump-target translation (for
`ParamKind::ItemIndex` parameters) all pick it up from the table.

### Add a UI panel

1. Add a variant to `layout::Panel`, with a pinned `serde` rename, `title`,
   `icon` (add the icon to `ui::Icon` if it is new) and `summary` (the line
   shown where it is listed among others), and list it in a
   `shell::Destination`'s `panels`. That puts it on the tiled shell's rail,
   and in its destination in the floating shell. Add a
   destination only for a new task: the navigation bar holds five. Where a
   panel opens in the tiled shell is the operator's choice (see
   [The workspace](#the-workspace-is-three-columns-around-the-map)).
2. Create `features/<name>.rs` (or a directory) with its state, `Message`,
   `update(&mut self, message, cx: &mut Context<Message>) -> Task<Message>`,
   and `view`, which returns a `ui::panel`.
3. In `app.rs`: add the state (to `Features` if it takes a `Context`), a
   `Message` variant routed through `dispatch`, and an arm in `pane_body`,
   which both shells show. Check it in a narrow sheet too: a phone gives a
   panel about 400 logical pixels of width.

### Add a setting

Add a field to `config::Settings` (it is `#[serde(default)]`, so old
configurations still load), a message and a control in
`features/settings_panel.rs`, and apply it in `Application::on_settings`.
Configuration is persisted only through `ConfigWriter` (a
`persist::SaveQueue`), which writes one save at a time, newest last, and
replaces the file atomically. The file is the operator's whole setup, so its
schema stays compatible (see `config.rs`'s module doc): persisted enums pin
their variant names, new fields have defaults, and every file in
`crates/groundhog-gui-iced/fixtures/config/` must keep loading; add one when
the schema changes. A session whose configuration could not be read, or was
written by a newer build, saves nothing.

---

## Pitfalls

- **Desktop clippy never sees the Android code.** `platform/android.rs` and
  the `android_main` export are `#[cfg(target_os = "android")]`, so
  `cargo clippy --workspace` skips them entirely — including the crate's only
  `unsafe`. `android/lint.sh` (a CI job) is what lints them; a change to that
  module is unverified until it passes.
- **mavio rejects untruncated MAVLink 2 frames with trailing zero bytes.**
  `Frame::from_header_and_slice` (mavio `protocol/frame.rs`) builds the
  received payload with `Payload::new`, which trims trailing zeros. The header
  and checksum still describe the untrimmed payload, so `decode` fails with a
  checksum error, and re-serializing writes a corrupt frame. ArduPilot and PX4
  truncate when sending, so this rarely shows, but test tools that build frames
  by hand must truncate too.
- **The dialect is `ardupilotmega`**, a superset of `common` (mavio's
  `dlct-ardupilotmega` feature). mavio generates it from MAVLink definitions
  older than current firmware, so some enums lack recent values. A message
  with an unknown enum value fails to decode and is dropped, unnoticed.
  `mavlink/decode.rs` is the one tolerant decoder: it patches the fields
  listed in `TOLERANT` to documented fallbacks. Add a row there (and a test)
  when you rely on another enum-carrying message; mission items use the raw
  codec `wire::mission_item` instead.
- **Groundhog does not request telemetry streams.** It shows what the
  autopilot sends by default: `SYS_STATUS` (battery, sensor health),
  `GPS_RAW_INT`, positions, attitude. A value that stays "–" may simply not
  be streamed; on ArduPilot the `SR*_` parameters set the rates. Do not
  assume a message arrives because the dialect has it.
- **Never resend calibration commands.** Firmwares do not deduplicate them: a
  resent start restarts or repeats the calibration, and a resent ArduPilot
  orientation confirmation is refused once measuring has begun.
  `VehicleClient` sends them with `retries(0)` and a long timeout.
- **Messages are formatted on every update** when iced's `debug` feature is
  on (see [The UI](#the-ui-iced)). A derived `Debug` on a large payload costs
  milliseconds per message.
- **Snapshots arrive about 30 times a second while vehicles report.** Anything
  in `view` or in an uncached map layer runs that often. Keep it linear and
  small, and give map layers a `revision` when they can be cached.
- **A new vehicle reaches the snapshot one publish after it registers.** Code
  that starts from a `FleetEvent` and reads the snapshot must wait for it
  (`FleetHandle::wait_for`), as the bootstrap does.
- **Do not compose the map with iced's `Stack`.** A `Stack` hides the pointer
  from lower layers whenever an upper layer shows a cursor, so hovering a
  handle would disable zoom and pan. `map/layered.rs` is the map's own
  container: the overlay sees events first, and the tiles lose only the events
  the overlay captures. Stacking *over* the whole map, as the floating shell
  and the toasts do, is fine, as long as each piece over it is a
  `widget::surface`: a finger reaches the map by its position whatever the
  `Stack` does with the cursor.
- **Keep the tree's shape when something comes and goes.** iced keys widget
  state on the tree's shape: a widget whose position or parent changes gets
  fresh state. Wrapping content in a `stack` only while an overlay shows
  rebuilds everything beneath it on opening and closing; the map then
  loses its draw cache (a flicker) and lists lose their scroll. Keep the
  layer and leave it empty (`Space`) when there is nothing in it, as
  `ui::dialog::view` and the shells' toasts do.
- **A finger is not the mouse.** iced reports fingers as `Event::Touch`, with
  an id each; the mouse cursor follows the last finger only loosely. Widgets
  that handle presses themselves (the map overlay, `drag_source`) must
  handle touch too, follow one finger by its id, and end what it started on
  `FingerLost`. Hover does not exist: tooltips are off on touch screens
  (`platform::TOUCH`), so a hint must never be the only way to learn what a
  control does.
- **`LayerId`s must be stable and unique.** The engine keys caches and pointer
  state on them.
- **The vendored `widget/pane_grid`** is a fork of iced's. Its header lists
  every change; keep that list current when you edit it.
- **ArduPilot mission numbering:** a plan item's index `i` is wire sequence
  `i + 1`, and `MISSION_CURRENT` reports wire sequences. Convert through
  `MissionLayout` (`wire_seq`, `plan_index`); never by hand.

---

## Glossary

| Term | Meaning |
|---|---|
| **System / `SystemId`** | A vehicle. Its components (autopilot, gimbal, companion computer) are endpoints of one vehicle. |
| **Address / `MavlinkId`** | System and component: where a frame goes or comes from. |
| **Autopilot component** | The component whose heartbeat names a real autopilot. Vehicle-wide state is folded only from it, and commands go to it. |
| **Snapshot** | An immutable `FleetSnapshot`, published by the runtime. |
| **Onboard mission** | What the vehicle holds as of the last transfer (`OnboardMission`). |
| **Draft** | The operator's plan for a vehicle, or the offline plan. |
| **Plan index / wire sequence** | An item's position in the plan, versus its sequence number on the wire. They differ by the firmware's home slot. |
| **Liveness** | Active, stale (no frame for 3 s) or lost (no frame for 10 s). |
| **Revision** | A counter that changes whenever content changes; used as a cache key. |
| **Operation / `Ticket`** | A tracked, cancellable task from the UI, and the identity of one run of it. |
| **Notice** | An operator notification: a toast, plus an entry in the log panel's history. |
