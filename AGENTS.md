# Working on Groundhog

Groundhog is a MAVLink ground control station (GCS) written in Rust with the
[iced](https://github.com/iced-rs/iced) GUI toolkit. It flies real aircraft, so
the code favours being correct and obvious over being clever.

This file is for **people and coding agents alike**. Read the first two
sections before changing anything; the rest is reference, to be read when you
touch the area it describes.

1. [Quick start](#quick-start)
2. [The architecture in one page](#the-architecture-in-one-page)
3. [The layers](#the-layers)
4. [Design decisions](#design-decisions) and why they were made
5. [Conventions](#conventions)
6. [Testing](#testing)
7. [Recipes](#recipes) for common changes
8. [Pitfalls](#pitfalls)
9. [Glossary](#glossary)

---

## Quick start

```sh
cargo run                                   # the application
cargo test                                  # every test; fast (< 1 s after building)
cargo fmt --check                           # CI gate
cargo clippy --all-targets -- -D warnings   # CI gate: warnings are errors
```

CI (`.github/workflows/ci.yml`) runs exactly the last three. **A change is done
when all three pass**, the change has tests, and the docs it touched (module
docs, this file) still tell the truth.

To try the application without a vehicle, run ArduPilot or PX4 SITL and add a
UDP link listening on the port it sends to (usually 14550). In listen mode a
UDP link serves every vehicle that sends to it.

Logs go to stderr through `env_logger`, at debug level for this crate and
error level for dependencies (see `main.rs`).

---

## The architecture in one page

```
 features/  UI features: panels and map plugins          ┐
 map/       map engine: layers, hit-testing, gestures     ├─ UI thread (iced)
 app.rs     composition root, message routing, policy     ┘
    │ reads snapshots ▲            │ intents (operations)
    │                 │            ▼
 services/  fleet runtime, vehicle client, link supervisors, recording  ┐
 domain/    vehicle state, missions, parameters, firmware, calibration  │
 mavlink/   router and transactions (commands, parameters, missions)    ├─ async runtime (tokio)
 link/      transports: TCP, UDP, serial                                │
 wire/      pure codecs shared by domain and middleware                 ┘
 storage/   file formats (missions, parameters)
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
4. **The UI** (`app.rs`, `features/`) keeps the latest snapshot and only
   *operator* state: mission drafts, parameter edits, the selected vehicle,
   view state. It changes vehicles only through operations on
   `services::VehicleClient`, never by mutating state. Results that change
   what a vehicle holds (a transferred mission, a parameter list) flow back
   through the runtime, so every reader sees them.

Vehicle state therefore keeps advancing when the UI is busy or stalled, and the
UI can never observe a half-applied update.

---

## The layers

A layer may use the layers listed below it in the diagram; never one above.
`foundation.rs` (`MavlinkId`, the wire *address* type) is shared by all.

| Module | Owns | Depends on | Must not |
|---|---|---|---|
| `link/` | Opening transports; `LinkConfig` | `foundation` | know about MAVLink messages |
| `wire/` | Pure codecs (parameter encodings) | `mavio` | hold state |
| `mavlink/` | Router, duplicate suppression, transactions, outbound queues | `link`, `wire`, `mavio`, `tokio` | know about vehicles as state, or the UI |
| `domain/` | Vehicle state and its folding, missions, parameters, firmware adapters, calibration protocols | `wire`, `mavio` types as vocabulary | do I/O, spawn tasks, use iced, call the middleware |
| `storage/` | File formats (WPL missions; INI/`.param`/`.params` parameters) | `domain` | touch the filesystem (callers do) |
| `services/` | Fleet runtime, `FleetHandle`, `VehicleClient`, `LinkManager`, tlog recording | `mavlink`, `domain`, `link` | use iced; expose middleware machinery (router, frames, transactions) in its API. Plain data it re-exports, like `TransferProgress`, is fine |
| `map/` | The map engine: layers, projection, pointer state machine, caching | iced, `slippery`, `domain::geo` | know about vehicles or missions |
| `features/` | One module per UI feature, plus shared UI building blocks | `services`, `domain`, `map`, `storage` | use `mavlink` (tests excepted) or each other's state |
| `app.rs` | Wiring at boot, message routing, effects, notice policy | everything | contain feature logic |
| `widget/` | Reusable widgets (vendored `pane_grid` fork, `virtual_list`, attitude) | iced | depend on the domain |

The domain deliberately uses `mavio` *types* (`MavCmd`, `MavFrame`, decoded
messages) as its vocabulary: a MAVLink GCS gains nothing from re-declaring the
MAVLink enums. What the domain must not use is the *middleware* (`mavlink/`).

---

## Design decisions

Each decision below records what was chosen and why, so you can tell when a
change would undo it.

### Vehicle state is owned off the UI thread

`services/runtime.rs` owns the `Fleet` and publishes `Arc<FleetSnapshot>`.
Snapshots are cheap: vehicles sit behind `Arc`, and heavy subsystems (trail,
status log, per-component parameters) sit behind their own `Arc`, so a
mutation copies only the subsystem it touches (`Arc::make_mut`).

*Why:* a GCS must keep tracking liveness, recording and folding telemetry
while the UI is busy, a file dialog is open, or a frame takes long. Snapshots
also make the UI a pure function of data, which makes it easy to test.

### Vehicle state and operator state are separate

The fleet holds only what vehicles *reported*. Mission drafts live in
`features::mission`, and parameter edits (`domain::parameter::ParamEdit`) in
`features::parameter_panel`.

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

- which flight modes exist and how to command one;
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
confirms and cancels through `VehicleClient` and renders what the session
says. Finished runs are announced as `FleetEvent::CalibrationFinished`.

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
`revision` is drawn from cache until that number or the view changes.

### Features talk to the application through a narrow contract

A feature's `update` gets a `features::Context`, holding:

- the snapshot;
- the services;
- the notices;
- settings and the file-dialog directory.

It requests anything else as an `Effect`: confirm, select a vehicle, move the
map, remember a directory. Features never reach into each other or into the
application.

### Failures reach the operator; consequential commands ask first

Every failed operation becomes a `Notice`: a toast, plus the log panel's
history. `services::Error` keeps the *reason* (`Rejection::TemporarilyRejected`,
`MissionRejection::NoSpace`, ...) so the notice is actionable.

Commands that change what a vehicle does ask the operator first through a
`Confirmation`: arming, starting a mission, jumping to an item, clearing the
vehicle's mission, returning or landing.

### Links are supervised

`services::LinkManager` spawns a supervisor per connected link. The supervisor
opens the transport, waits for it to fail, and reconnects with backoff, unless
the link is configured not to. The status the UI shows is the supervisor's own
(read through a `watch`), never a copy that needs reconciling.

---

## Conventions

### Code

- **Format and lint:** `rustfmt` defaults; clippy with `-D warnings`. Do not
  add `#[allow(...)]` to silence a lint; fix the cause. The vendored
  `widget/pane_grid` is the only exception.
- **No dead code.** This is a binary crate, so an unused `pub` item is a
  warning. Delete it, or gate test-only helpers with `#[cfg(test)]`.
- **Panics:** none in non-test code, except `expect("why this holds")` on a
  proven invariant.
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

This crate tracks iced's git `master`, where some APIs differ from the released
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
  Text in a fixed-height row must be single-line
  (`.wrapping(Wrapping::None).ellipsis(Ellipsis::End)`).
- Never block the UI thread. File I/O goes through
  `iced_runtime::task::blocking`, network work through services. `boot` and
  `update` run inside the async runtime, so `tokio::spawn` works there.
- Colours come from the theme palette (`style.rs`), except map overlays,
  which use fixed colours that read well over any map tile.

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
- Ask for confirmation before any command that changes what the vehicle does.

---

## Testing

All tests run with `cargo test`; most finish in microseconds. Every bug fix
comes with a test that fails without the fix.

| What | Where | How |
|---|---|---|
| Middleware transactions | `mavlink/tests.rs` | `MockVehicle` over `tokio::io::duplex`; script the vehicle's side |
| Router internals | `mavlink/router.rs` tests | call `handle_frame` with built frames; inspect the route table |
| End to end | `services/tests.rs` | real router, runtime and `VehicleClient` against a mock vehicle; `until(...)` waits for a snapshot condition |
| Domain | next to the code | plain unit tests; fold messages with `Fleet::observe` / `Vehicle::handle_message` |
| Map gestures | `map/overlay.rs` tests | drive the canvas `Program` with synthetic mouse and keyboard events |
| Widget composition | `map/mod.rs` tests | `iced_test::Simulator` delivers real events through the composed widgets; assert on event status and messages |
| Features | `features/*/tests.rs` | build a `Context` with a synthetic `Fleet` snapshot and a real `FleetHandle`; assert on state, notices and requested effects |

Feature tests need a `FleetHandle`, so they are `#[tokio::test]` and spawn the
runtime with `RuntimeOptions { bootstrap: false }`. To simulate an operation's
result, take its ticket with `Operations::ticket_of` (test-only).

Tests must not touch the user's files. Write into a temporary directory, or
point `XDG_CONFIG_HOME` and `XDG_DATA_HOME` somewhere else.

`iced_test` (a dev-dependency) runs widgets headlessly. Its `Simulator` can
deliver events through any `Element` (see the map's tests), and render the
whole `Application::view()` to PNG with `snapshot(..).matches_image(path)`.
`matches_image` only writes the PNG when no file exists yet (the name gains a
renderer suffix such as `-wgpu`); otherwise it compares against the existing
one. Delete old snapshots before re-rendering.

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
in each protocol's `start`, `supports` and `observe`. Feedback a protocol does
not understand yet can still go to the session's log with `Session::note`.
Test the start parameters per firmware and the feedback sequence, as
`domain/calibration/tests.rs` does; `services/tests.rs` drives a whole run
against a mock vehicle.

### Support another mission command

Add one `CommandSpec` to `domain::mission::command::SPECS`, following
`common.xml` for the parameter meanings. The editor, the map (for commands
with a location), the file formats, and jump-target translation (for
`ParamKind::ItemIndex` parameters) all pick it up from the table.

### Add a UI panel

1. Add a variant to `layout::Panel`, with a pinned `serde` rename, `title`,
   `badge` and `default_edge`, and list it in `Panel::DOCKABLE`.
2. Create `features/<name>.rs` (or a directory) with its state, `Message`,
   `update(&mut self, message, cx: &mut Context<Message>) -> Task<Message>`,
   and `view`.
3. In `app.rs`: add the state (to `Features` if it takes a `Context`), a
   `Message` variant routed through `dispatch`, and an arm in `pane_body`.

### Add a setting

Add a field to `config::Settings` (it is `#[serde(default)]`, so old
configurations still load), a message and a control in
`features/settings_panel.rs`, and apply it in `Application::on_settings`.
Configuration is persisted only through `ConfigWriter`, which writes one save
at a time, newest last.

---

## Pitfalls

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
  with an unknown enum value fails to decode and is dropped, unnoticed. The
  runtime works around this for `MAG_CAL_REPORT`
  (`services/runtime.rs`, `decode_mag_cal_report`); check for the same when
  you rely on another enum-carrying message.
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
  the overlay captures.
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
