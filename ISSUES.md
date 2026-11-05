# Issues

Every known problem in Groundhog that is still open, in the order to fix
them. Nothing else lives here: no history, no closed or rejected entries.

- **Fixing one:** write the test the entry names first and see it fail (per
  `AGENTS.md`). In the commit that fixes it, delete the entry.
- **Adding one:** check the claim against the code first. Put it in the
  section of its priority, and give it the next free number of its area.
  IDs never change once given.
- **A wrong claim, or one not worth fixing:** delete the entry; say why in the
  commit message.
- **Priority:** P1 a wrong result the operator sees often · P2 degraded
  behaviour or a correctness edge · P3 maintainability or polish. **Effort:**
  S small, M a few modules, L a design change.

Paths are relative to a crate's `src/`. Trust the symbol over any line number.

---

## Fix first

### MISSION-2 · A jump to home cannot be entered, and reaches PX4 as `-1`
**P2** · M

A plan stores ArduPilot's jump to home (wire 0) as `-1`. The editor shows it
as "Home" (`panels/mission/view.rs` `format_value`), but `set_field` refuses
0, so the editor cannot enter what it shows. A plan holding `-1` (an
ArduPilot download, any WPL file) uploaded to PX4, which has no home slot,
sends `param1 = -1`; opaque targets (`1e30`) go to any vehicle as they are. A
wire target of `-1` comes back as `0` after a round trip.

**Fix:**
- `domain::mission::Refusal` gains `InvalidJumpTarget { item }`.
  `MissionDraft::check(Upload)` refuses an `ItemIndex` parameter that, encoded
  under the vehicle's `MissionLayout`, is not a whole number naming a wire
  item other than home. Home on a home-slot layout stays allowed. The panel
  shows the refusal and selects the item.
- Under a home-slot layout the editor accepts 0 as "Home".
- `codec::shift_targets` shifts every whole, finite value by the offset,
  negatives included, and the `-1` special case goes; `item_ref` governs plan
  edits only. Update the docs of `shift_targets` and `MissionLayout`.

**Test:** an ArduPilot jump to home uploaded to a PX4 draft is refused and
names the item; `1e30` is refused on both layouts; the editor takes 0 as
Home under ArduPilot only; wire `-1` and `0` stay distinct through a round
trip.

### VEH-1 · Disabled arming checks read "Not ready" forever; health mixes rules
**P2** · S

ArduPilot always marks `PREARM_CHECK` present, enabled only while arming
checks are on (`GCS_MAVLink/GCS.cpp`). `Health::prearm_passes` ignores
`enabled` (`domain/vehicle/health.rs`), so `ARMING_CHECK = 0` shows a red
"Not ready" forever (`panels/vehicles/mod.rs`). Separately, a sensor's
`enabled` is ORed over its present flags while `healthy` is ANDed, so a
healthy primary beside a disabled backup reads unhealthy, and `failing()`
inherits it.

**Fix:** `prearm_passes() -> Option<bool>` becomes `prearm() ->
Option<Prearm>` with `Prearm::{Passing, Failing, Disabled}`; the panel shows
"Arming checks off" in a warning tone. Judge `healthy` over the enabled flags
(the present ones only when none is enabled).

**Test:** `PREARM_CHECK` present and not enabled is `Disabled`; a healthy
primary with a disabled backup is healthy.

### PROTO-1 · A message with two unknown enum values never decodes
**P2** · S

`mavlink/decode.rs` `decode` patches the one field `InvalidEnumValue` names
and decodes again once. A `HEARTBEAT` whose `type` and `autopilot` are both
newer than the dialect still fails, and the vehicle never registers.

**Fix:** loop over one working copy of the payload: patch the named field,
decode again; stop when it decodes, when the field has no fallback, or after
one patch per fallback field. `patched()` patches a buffer in place.

**Test:** a heartbeat with bytes 4 and 5 both 200 decodes with both fallbacks.

### FW-1 · PX4 sub-modes outside AUTO are dropped
**P2** · S

`decode_mode` zeroes the sub byte unless the main mode is AUTO
(`domain/firmware/px4.rs`). PX4 reports Orbit as POSCTL sub-mode 1 (Position
Slow as 2), so an orbiting vehicle reads "Position" and entering Orbit raises
no `ModeChanged`.

**Fix:** check the values against PX4's `px4_custom_mode.h` first. Add
reported-only modes "Orbit" (`ModeKind::Guided`) and "Position Slow"
(`ModeKind::PositionHold`) to the set `decode_mode` searches, not to any
offered table. Keep the sub byte for AUTO and POSCTL; there an unknown
sub-mode decodes to `None`.

**Test:** POSCTL/1 is Orbit, POSCTL/0 Position, POSCTL/9 `None`; Position to
Orbit emits `ModeChanged`.

### PARAM-1 · The INI reader lets inf and NaN reach a vehicle
**P2** · S

`.param` and `.params` filter `is_finite` (`storage/parameter.rs` `parse_as`);
the INI reader (`parameters_from_ini_str`) does not, so `f32 = inf` loads as
an uploadable value.

**Fix:** refuse non-finite values in the INI reader as the others do.

**Test:** an INI file with `f32 = inf` and `f32 = NaN` loads neither.

### VEH-2 · The backend lets an armed vehicle start a calibration
**P2** · M

`VehicleClient::start_calibration` sends whatever it is asked. The rules
every frontend would share live in the GUI: whether a mode change asks
(`panels/vehicles/mod.rs` `mode_change_asks`), what blocks a calibration
(`panels/calibration.rs` `StartBlocked`), and the heading shown
(`panels/vehicles/map.rs` `heading`). `AGENTS.md` puts such rules in the
logic crate, as `MissionDraft::check` does for missions.

**Fix:**
- Move `Verdict`, `Caution` and `Refusal` to `domain::check` (re-exported
  from `mission`). Add `VehicleAction::{Arm, Disarm, SetMode(FlightMode),
  StartCalibration(CalibrationKind)}` and `Vehicle::check(action) -> Verdict`:
  `Arm` confirms; `SetMode` confirms with `Caution::Armed` unless
  `armed() == Some(false)`; `StartCalibration` is refused when armed, lost, or
  the kind is not offered; `Disarm` goes.
- `start_calibration` enforces its refusals with `Error::Refused(Refusal)`
  before sending.
- `Vehicle::heading_rad() -> Option<f32>`: yaw, else course over ground above
  0.5 m/s.
- The three GUI sites call these and keep their wording.

**Test:** `SetMode` confirms when armed or unknown; `StartCalibration` is
refused when armed and when lost; in `services/tests.rs`, `start_calibration`
on an armed vehicle sends nothing.

---

## Next

### LINK-1 · A reconnected link races its own closing transport
**P2** · M

`LinkManager::disconnect` only aborts the supervisor and publishes
`Disconnected` itself (`services/link.rs`); the transport closes later, when
the router's tasks wind down. So:
- A reopen (Disconnect then Connect, or a save that changes the transport)
  opens the new transport first: a fixed-port UDP link fails with
  `EADDRINUSE` 28 times in 50 and, without auto-reconnect, ends in `Failed`.
- The aborted supervisor, mid-poll on another worker, can still publish
  `Connected` after `Disconnected` (26 in 200): the app toasts "connected"
  while `status()` says disconnected.
- `LinkGuard::closed()` fires before the reader's tail runs `remove_link`
  (`mavlink/router.rs`), so for a moment `route()` can hand a frame to a
  cancelled writer and report success.
- The status watch is seeded with `Connecting { attempt: 1 }` and the
  supervisor publishes the same, so the first event has `previous == status`.

**Fix:**
- Split the link's token into "shutdown requested" and "closed"; the reader
  cancels "closed" after `remove_link`, and `closed()` waits for it.
  `LinkGuard::shutdown(self)` resolves once both tasks have dropped their
  transport halves.
- The supervisor is the only publisher. `Session` holds a token and the
  supervisor's `JoinHandle`; `disconnect` cancels and moves the handle to
  `LinkEntry::closing`. `supervise` races each await against the token and
  on cancel shuts the guard down, publishes `Disconnected`, and returns.
  `connect` hands `closing` to the new supervisor, which awaits it before
  opening.
- Publish only actual changes.

**Test:** on a multi-thread runtime, fifty times each: a fixed-port UDP link
without auto-reconnect, disconnected and connected again, reaches
`Connected`; connect, wait 20 µs, disconnect: the last event is
`Disconnected`. After `closed()` resolves, routing to the link fails. The
first event differs from its previous status.

### LINK-2 · Renaming a link leaves a duplicate behind
**P2** · M · after LINK-1

`LinkManager::save` finds the link by display name and otherwise adds a new
one. The editor carries no `LinkId`, so renaming "A" to "B" creates "B" and
keeps "A"; if "A" auto-connects, the next start opens both to the same
endpoint. A save that changes a live link's transport also restarts it
without asking (the app only shows a notice afterwards).

**Fix:** replace `save` with `add(config) -> Result<LinkId>` and
`update(&LinkId, config) -> Result<Saved>`, both refusing a name another link
has. `LinkConfigurator` carries the edited `Option<LinkId>`.
`update_restarts(&LinkId, &config) -> bool` lets the panel ask through an
`Effect::Confirm` first. Make `LinkManager::new` `pub(crate)` (it takes the
crate-private `MavlinkRouter`) and enable the `private_interfaces` lint.

**Test:** renaming keeps one link with the old id; a duplicate name is
refused; changing a live link's transport asks first.

### LINK-3 · A listening UDP link admits any datagram as a peer
**P2** · S

`link/udp.rs` admits any source to the peer table, and every outbound frame
goes to every live peer. One spoofed datagram gets 10 s of everything
Groundhog sends; sixteen evict every real peer. A truncated datagram is also spliced
onto the next one through the shared byte buffer.

**Fix:** check each datagram's MAVLink framing (start byte, declared lengths
adding up to the datagram) before passing it on. That is framing, not
messages, so `link/` stays message-agnostic. Admit a peer only after a
plausible datagram; drop an implausible one whole.

**Test:** random datagrams from a new source neither admit it nor delay the
next real frame; a truncated datagram does not corrupt the following one.

### REC-1 · A failed telemetry recording stops silently
**P2** · M

`services/recording.rs` returns from its task on a write error (a full disk)
with only a log line, while `TlogRecorder` still looks alive and the UI still
shows the file. `stop()` returns `()` without awaiting the final flush, and
leaves frames already queued unwritten. Timestamps are taken when the writer
gets to a frame, not when it arrived, and each record is two `write_all`s,
so a failed write can leave a timestamp without its frame.

**Fix:** bound the frame channel (drop the oldest, count it) and carry each
frame's receive time. Build each record in one buffer, written once. Expose
the recorder's status; a write error ends it visibly and the app raises a
notice. `stop()` becomes async, drains the queue, flushes, and returns a
`Result`; the app's close path awaits it.

**Test:** a failing writer is reported through the status and `stop()`;
timestamps are receive times; `stop()` right after a burst has every frame on
disk.

### RUN-1 · Background tasks are unowned, and a vehicle is bootstrapped once
**P2** · M

The runtime starts each bootstrap, `describe_camera` and calibration cancel
with a bare `tokio::spawn` (`services/runtime.rs`). Dropping the `Backend`
does not stop them, and a failed calibration cancel is only logged, so the
vehicle may go on calibrating after the operator was told the run failed.
`Registered` fires once per system id, so a bootstrap that failed over a
lossy link is never retried, and after a reboot or reflash the parameters,
home and mission stay stale. Nothing says when a bootstrap is done:
`VehicleClient::bootstrap` is `pub`, and a headless client that sets a
parameter right after the vehicle appears gets `ParamEncodingUnknown`.

**Fix:**
- The runtime keeps a `JoinSet` for every task it starts, reaped in its
  `select!` loop; dropping the runtime aborts them.
- A failed cancel emits `FleetEvent::CalibrationCancelFailed { system, kind,
  error }`, shown as a warning ("The vehicle may still be calibrating").
- One bootstrap per vehicle at a time, started on `Registered`, on
  `Rebooted` (aborting a running one), and on a return from `Lost` when the
  last one did not finish cleanly. A reboot re-runs the whole bootstrap; the
  capability request is still skipped when the encoding is known.
- `Vehicle` gains `bootstrap: Bootstrap::{Pending, Running, Done { failed:
  Vec<Step> }}`, reported through `Input::Bootstrap` with a drop guard; the
  fleet emits `FleetEvent::Bootstrapped`. `bootstrap` becomes `pub(crate)`.

**Test:** dropping the `Backend` mid-bootstrap stops requests to a silent
mock; a reboot brings a second `PARAM_REQUEST_LIST`; a failed mission
download gives `Done { failed: [Mission] }` and a return from `Lost` re-runs
it; a refused cancel produces the event.

### PARAM-2 · The three parameter file readers disagree
**P2** · M

`.param` and `.params` abort on one bad line (including an unknown 64-bit
type); `.ini` logs and skips. Unknown names are dropped before
`ParamEdits::load` can count them, names are truncated to 16 bytes silently,
and duplicates are last-wins (`storage/parameter.rs`). QGC `.params` files
carry a system and component per line, which the reader discards, so a file
from another vehicle loads into whichever component is open. Errors are
`Result<_, String>`.

**Fix:** one policy for all three: skip a bad line and report it as a typed
`LineError`. The reader returns the values, the file's address where the
format has one, the skipped lines, truncated names and duplicates.
`ParamEdits::load` reports all of it in the load summary, including "N values
were for another component", and leaves those out.

**Test:** each format with a bad line, an unknown type, a 17-byte name and a
duplicate gives the same summary; a `.params` file for another component
loads nothing and says so.

### PARAM-3 · "Save to a file" can save the wrong values without saying so
**P2** · S

The save dialog (`panels/parameters.rs`, `Message::SaveDialog`) exports
`Parameters::values()` even while the encoding is unknown, so a
bytewise-decoded integer is saved as if it were real; uploads are gated,
saves are not. With pending edits, the header says "3 changed" while the
file holds only reported values. `SaveTo` and `Loaded` carry a whole
`ParamValues` with a derived `Debug`, which iced prints on the UI thread with
its `debug` feature on.

**Fix:** disable Save, with a reason, until the encoding is known. Save
reported values and say the pending edits were not included. Give `Message` a
summary `Debug` for those variants.

**Test:** Save is unavailable while the encoding is unknown; saving with
edits pending says so.

### VEH-3 · Vehicle state never ages
**P2** · M

`Fleet::tick` ages only liveness (`domain/fleet.rs`). A lost vehicle keeps
showing its last position, velocity, GPS health and home as if current, and a
zeroed `HOME_POSITION` is ignored rather than clearing home.

**Fix:** keep the last values. Position, velocity and GPS carry their receive
time; views show the age beside a stale or lost vehicle and dim it. A zeroed
`HOME_POSITION` clears home.

**Test:** a zeroed home clears it; a vehicle silent past the stale window
reports the age of its position.

### UI-1 · Touch drags follow the wrong finger or the mouse
**P2** · M

`widget/drag_source.rs` treats any `FingerPressed` as a new press and any
lift as the end, so a second finger abandons a drag without publishing
`on_end` and leaves the pane grid's highlight stuck. The vendored
`widget/pane_grid.rs` handles finger presses but reads `mouse::Cursor` for
positions, so divider and title-bar drags follow a stale mouse cursor on a
tablet.

**Fix:** one `widget::touch` tracker that follows one finger by id with its
own position, ignores other fingers, and ends on lift or `FingerLost`. Use it
in `drag_source` (publishing `on_end` on every terminal path) and in the
pane grid (note the change in its header), and move the map overlay onto it.

**Test:** a second finger neither starts nor ends a drag; a lost finger
publishes `on_end`; a divider drag follows the finger.

### VIDEO-1 · A replaced receiver overlaps the old one
**P2** · M

`Receiver`'s `Drop` only sets a flag the supervisor polls every 100 ms
(`groundhog-video` `receiver.rs`), so a replacement (every source change and
low-latency toggle) runs beside the old pipeline for a moment. UDP `reuse`
lets both bind; a camera that allows one RTSP session refuses the new one,
which then waits for a retry.

**Fix:** `Receiver::replace(self, config)` gives the old supervisor's join
handle to the new one, which waits for it on its own thread before opening;
the UI never blocks. The stop signal becomes a channel.

**Test:** replacing a receiver never has two pipelines playing at once.

### VIDEO-2 · Failures that cannot heal are retried forever
**P2** · S

Every bus error becomes `Ended::Retry` (`receiver.rs` `watch`). With
`decodebin3`, a missing decoder plugin is a bus error, so a stream the
machine can never decode retries every second forever. A failed supervisor
thread spawn (`Receiver::new`) is only logged, leaving the status
`Connecting` for good.

**Fix:** a missing plugin or decoder ends in `Status::Failed`, which the panel
shows with a Retry; other errors keep retrying, since a camera may boot after
the GCS. A failed spawn sets `Failed`.

**Test:** a stream needing a decoder that is not installed ends in `Failed`.

---

## Later

### PERSIST-1 · Two edges in durable writes
**P3** · S

`persist.rs` `write_durably` creates the temporary file with default
permissions and copies the target's afterwards, so a `0600` file's contents
are briefly readable by others. `read_or_set_aside` renames onto
`<name>.corrupt`, destroying an earlier set-aside file.

**Fix:** on unix, create the temporary file with the target's mode
(`OpenOptions` + `mode()`). Set aside to the first free name of
`<name>.corrupt`, `<name>.corrupt.1`, …

**Test:** setting aside twice keeps both copies.

### RUN-2 · A panic in a fold freezes the vehicle state silently
**P3** · S

The runtime task (`services/runtime.rs` `run`) is the only task that
advances vehicle state, and nothing watches it. A panic ends it and the UI
renders the last snapshot forever.

**Fix:** the `Backend` watches the task; a panic becomes a lasting event the
UI shows as a banner ("vehicle state stopped updating").

**Test:** a panicking fold produces the event.

### UI-2 · Small layout and settings gaps
**P3** · S

- The tiled rail (`app/tiled.rs` `view_rail`) is a plain column of 48 px buttons
  and the window has no minimum height, so a short window cuts off the last
  panels. Make it scroll.
- Loaded settings are not sanitised (`config.rs`): `scale_factor: 0` blanks
  the window, and the waypoint altitude is unchecked. Add
  `Settings::sanitized()` on load, with the ranges the settings controls use,
  defined once.
- Fixed pixel sizes ignore `ui::Density`: the parameter list's
  `HEADER_HEIGHT`, the Arm button and mode picker widths
  (`panels/vehicles/mod.rs`), and icons. The unlabelled checkbox
  (`ui/input.rs`) is a 22 px target at touch density; give it a
  `size::control()` hit area.

**Test:** a short window reaches the last rail button; a scale factor of 0
loads as the minimum.

### UI-3 · The log panel rebuilds its whole timeline on every view
**P3** · M

`panels/logs.rs` collects, filters, sorts and truncates up to ~8500 status
texts and notices each time it is drawn, and the header counts the entries
after truncation.

**Fix:** keep the merged, filtered timeline in the panel, updated from the
status logs' and notices' revisions. Count before truncating.

### MAP-1 · Map view edge cases
**P3** · S

- `MapView::key()` omits the visible rectangle (`map/view.rs`), so a cached
  layer keeps the old one when a sheet opens at constant map size. Latent:
  the only `visible()` user is not cached.
- `visible()` clamps the size but not the origin, so an over-large `covered`
  puts the rectangle outside the map.
- Layer caches key on `LayerId` alone (`map/overlay.rs`); two layers sharing
  an id share a cache. Add a debug assertion that ids are unique.
- Hover is refreshed only on `CursorMoved`, so after a wheel zoom, momentum
  pan or `center_on` the old handle stays highlighted.

**Test:** a cached layer redraws when the visible rectangle changes; hover
clears after a zoom.

### PROTO-2 · A duplicate backlog longer than the window is accepted
**P3** · M

The duplicate window holds 192 packets per source (`mavlink/dedup.rs`). A
link that lags further behind (a store-and-forward radio, a TCP connection
catching up after a few seconds) has every one of its copies accepted and
folded again. The module doc claims "however many arrive in a row".

**Fix:** track where each link is in a source's stream. Each source counts
the packets it accepted; each (source, link) remembers the position and
sequence of the last duplicate that link delivered, marking it behind. A
later packet on a behind link is placed by its sequence difference from that
duplicate and dropped until it passes the newest accepted packet. A link with
its own sequence counter never delivers a duplicate, so it is never held
back. Rewrite the module doc.

**Test:** a 500-packet backlog is dropped in full; an independent channel is
unaffected; a lagging link takes over when the fast one goes silent.

### PROTO-3 · A wedged writer is not detected
**P3** · M · after LINK-1

The heartbeat is the only outbound liveness write, and a full control queue
skips it (`mavlink/router.rs`). A write that never completes (a stuck serial
port) leaves the link attached and `closed()` silent, and `LinkStats` has no
`last_tx`.

**Fix:** the writer sends the heartbeat itself on its interval, so it never
waits behind a full queue, and the heartbeat task goes. Each write has a
stall timeout that closes the link. `LinkStats` gains `last_tx`.

**Test:** a writer that never completes a write closes the link within the
timeout; a flood of bulk traffic does not delay the heartbeat.

### DOCS-1 · Small doc fixes
**P3** · S

- `widget/attitude.rs`' module doc says ±30° while `MAX_ATTITUDE_DEG` is 40.
- `domain/vehicle/mod.rs` says the drafts "live with the features that edit
  them"; the GUI calls them panels now.
- `AGENTS.md`: the vendored pane-grid files are exempt from the module-doc
  rule (most start with `use`), and the gallery (`app/gallery.rs`) is a
  manual tool with no committed reference images.
- `services/error.rs`: `From<MavResult>` maps `Accepted`/`InProgress` to
  `Rejection::Unknown` and `MavMissionAccepted` to `Error`. Unreachable
  today; build rejections only from failure values.
- `domain/vehicle/mod.rs` `string_from_cstring` drops a whole name for one
  invalid UTF-8 byte; decode lossily, as status texts do.

---

## Public API

These matter once a second frontend or a headless client uses
`groundhog-logic`. Each breaks the public API in place and updates
`groundhog-gui-iced` in the same commit: the crate has one frontend.

### API-1 · A component client sends vehicle commands to the autopilot
**P3** · M · after RUN-1

`FleetHandle::component(address)` returns a `VehicleClient`, whose component
only parameters honour: `arm`, `set_mode`, mission and calibration operations
target `Vehicle::target()`. So `fleet.component(gimbal).arm(true)` arms the
autopilot. The GUI uses it only for parameters (`panels/mod.rs`).

**Fix:** `VehicleClient { fleet, system }` keeps the vehicle's operations; a
new `ComponentClient { fleet, address }` gets `fetch_parameters` and
`set_parameter`. `FleetHandle::component` returns it, and
`VehicleClient::parameters()` gives the default parameter component's. Flags
become operations: `arm()`/`disarm()`, `pause_mission()`/`resume_mission()`.

**Test:** the parameter tests through `ComponentClient`; `disarm()` sends
param1 = 0.

### API-2 · One reader, unbounded queues, nothing to await
**P3** · M · after RUN-1

`FleetUpdates` and `LinkEvents` each have one consumer and an unbounded queue
(`services/runtime.rs`, `services/link.rs`): a frontend that does not drain
them grows without bound, and a second reader cannot subscribe. Waiting for
anything but a vehicle's appearance means polling. `FleetEvent` mixes tuple
and struct variants, and only `LivenessChanged` carries the value before.

**Fix:** events go on a `broadcast` channel (capacity 1024);
`FleetHandle::subscribe()` makes a reader, and `FleetUpdate` gains `lagged`
for a reader that fell behind and must re-read the snapshot. `LinkEvents`
the same. `FleetHandle::wait_until(timeout, f)` replaces `wait_for`. Every
`FleetEvent` variant gets named fields with `from`/`to` where a value
changed, and `FleetEvent::system()`.

**Test:** two subscribers both get every event; one that never reads does
not stop the other and later sees `lagged > 0`; `ModeChanged` carries `from`.

### API-3 · Internals, dependency types and unitless names in the public API
**P3** · M · after API-1, API-2

- `Firmware` and `CalibrationProtocol` are public traits nobody outside can
  register, exposing wire builders and a fold. Add
  `Vehicle::firmware_name()` and make them, and `Vehicle::firmware()`,
  `pub(crate)`.
- `DownloadUpdate`, `Vehicle::new`, `is_authoritative`, `target`,
  `target_for` and `MissionProgress::current_seq` are public; make them
  `pub(crate)` and drop `current_index(layout)` for
  `Vehicle::current_mission_index`.
- nalgebra is in public fields (`Attitude::attitude`, `velocity_ned`): offer
  `roll_rad()`/`pitch_rad()`/`yaw_rad()` and a crate `Ned` type.
- `mav_param::{Ident, Value}` are needed to call `set_parameter` but not
  re-exported: `pub use` them from `domain::parameter`.
- No units in names: `BatteryState { voltage, current, remaining }`,
  `Home::altitude_msl`, `climb_rate()`, `ground_speed()`.
  `GlobalPosition::alt_agl` is height above home: rename it `alt_rel_m`.
- Sentinels: `OnboardMission::opaque_id` (0 = none) and
  `ParamDownload::expected()` (0 = unknown) become `Option`s.
- `status::rank` is inverted (lower is worse) and compared by hand
  (`app.rs`, `panels/logs.rs`): replace it with
  `StatusText::is_at_least(MavSeverity)`.
- Typed errors: `Error::NoSuchItem { index }` for
  `set_current_mission_item`, and `DownloadOutcome::Failed` holding a typed
  reason, not a `String`.

### API-4 · No written error and result policy
**P3** · S/M

Structured errors, `String` and `anyhow` coexist. `services::Result` does not
cover recording, `connect`, `remove` or `bootstrap`; `LinkStatus::Failed`,
`video::Status`, `Source::parse`, `CalibrationUpdate::Acknowledged` and
`EditStatus::Failed` carry `String`s. Write the policy in `AGENTS.md`
(structured errors in the libraries, `String` only for display at the
boundary) and in `services/mod.rs`, then bring the outliers in line.

### API-5 · Central public types are undocumented
**P3** · M

`Vehicle`, `Identity`, `Battery`, `AttitudeState`, `PositionTrack`,
`StatusLog`, `GlobalPosition` and many of their fields have no `///`. Document
them, then decide on `missing_docs` and record the decision in `AGENTS.md`.

### API-6 · Identity types are half-typed
**P3** · M

`SystemId(pub u8)` and `MavlinkId { pub system, pub component }` are public
and `Default` (0/0, broadcast); the GUI converts with `as` and truncates
`u64` to `u32` in the map's helper ids. Components are a bare `u8`
throughout (`component_state(u8)`, `target_for(u8)`). Consider private
fields with conversions and a `ComponentId`; at least remove the truncation.
