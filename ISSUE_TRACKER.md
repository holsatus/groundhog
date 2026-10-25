# Issue tracker

The order and scope of the work that closes [`OPEN_ISSUES.md`](OPEN_ISSUES.md).
That file says *what* is wrong; this one says in which order it is fixed, which
issues each piece of work closes together, and what shape the fix takes. Many
issues are symptoms of one design flaw, so work is grouped by cause, not by the
sections of the issue list.

## How to use this tracker

- Work packages run top to bottom. A package may start early only if every
  package in its **After** line is done.
- A package is one or a few commits. Each commit deletes the `OPEN_ISSUES.md`
  entries it closes (that file's rule) and ticks them here.
- When a package is finished, set its status to **Done** and leave it in place
  until the whole phase is done; then remove the phase.
- A package whose design turns out wrong is rewritten here before the code
  changes, so the tracker never describes a fix nobody is making.
- Status: **Todo** · **In progress** · **Blocked** (say on what) · **Done**.

## Overview

| # | Package | Closes | Priority | After | Status |
|---|---|---|---|---|---|
| 1 | [Empty plans upload nothing](#wp1--empty-plans-upload-nothing) | MISSION-1, CAL-8 | P0 | — | Done |
| 2 | [Raw mission items](#wp2--raw-mission-items) | PROTO-1 (missions) | P0 | 1 | Done |
| 3 | [One tolerant decoder](#wp3--one-tolerant-decoder) | PROTO-1 (status messages) | P0 | — | Done |
| 4 | [Parameter channel lock](#wp4--parameter-channel-lock) | PARAM-1 | P1 | — | Done |
| 5 | [One parameter download model](#wp5--one-parameter-download-model) | PARAM-2, PARAM-6, PARAM-7, SVC-1, API-5, LINK-3, LINK-5, SVC-11 | P1 | 4 | Done |
| 6 | [PX4 modes per class](#wp6--px4-modes-per-class) | CAL-1 | P1 | — | Done |
| 7 | [Overlays that fit the window](#wp7--overlays-that-fit-the-window) | UI-1, UI-10, UI-13 (dialog, toast) | P1 | — | Done |
| 8 | [Shared feature test harness](#wp8--shared-feature-test-harness) | API-10 | P3 (enabler) | — | Done |
| 9 | [Calibration session lifecycle](#wp9--calibration-session-lifecycle) | CAL-2, CAL-3, CAL-4, CAL-5, CAL-6, CAL-7, CAL-9, SVC-14 | P2 | 8 | Done |
| 10 | [Mission transfer identity and cancellation](#wp10--mission-transfer-identity-and-cancellation) | PROTO-3, PROTO-6 | P2 | 2 | Todo |
| 11 | [Mission model at the boundaries](#wp11--mission-model-at-the-boundaries) | MISSION-5, MISSION-6, MISSION-7, MISSION-9, MISSION-10, MISSION-11 | P2 | 2 | Todo |
| 12 | [Mission panel during transfers and loads](#wp12--mission-panel-during-transfers-and-loads) | MISSION-2, MISSION-3, MISSION-4, MISSION-8 | P2 | 8, 11 | Todo |
| 13 | [Router link lifecycle](#wp13--router-link-lifecycle) | PROTO-4, PROTO-5 | P2 | — | Todo |
| 14 | [Duplicate suppression by stream position](#wp14--duplicate-suppression-by-stream-position) | PROTO-2 | P2 | — | Todo |
| 15 | [Link management by id](#wp15--link-management-by-id) | LINK-1, LINK-7, SVC-8, PARAM-10, API-2 | P2 | 13 | Todo |
| 16 | [UDP framing per datagram; serial opens](#wp16--udp-framing-per-datagram-serial-opens) | LINK-2, LINK-6, LINK-4 | P2 | 13 | Todo |
| 17 | [Vehicle state: freshness and health](#wp17--vehicle-state-freshness-and-health) | SVC-3, SVC-4, SVC-12, SVC-16 | P2 | — | Todo |
| 18 | [Fold cost](#wp18--fold-cost) | SVC-9, SVC-10 | P3 | — | Todo |
| 19 | [Test-only fleet folding](#wp19--test-only-fleet-folding) | SVC-13 | P2 | 8 | Done |
| 20 | [Parameter files](#wp20--parameter-files) | PARAM-3, PARAM-4, PARAM-5, PARAM-8, PARAM-9, SVC-17 (parameter files) | P2 | 5 | Todo |
| 21 | [Recording and runtime robustness](#wp21--recording-and-runtime-robustness) | SVC-2, SVC-5, SVC-6, SVC-7, API-3 | P2 | — | Todo |
| 22 | [One finger tracker](#wp22--one-finger-tracker) | UI-7, UI-8 | P2 | — | Todo |
| 23 | [Layout, density and settings](#wp23--layout-density-and-settings) | UI-2, UI-3, UI-4, UI-5, UI-6, UI-9, UI-12, UI-13 (rest) | P2 | 7 | Todo |
| 24 | [Map engine](#wp24--map-engine) | MAP-1 … MAP-7 | P2 | — | Todo |
| 25 | [Video receiver](#wp25--video-receiver) | VIDEO-1 … VIDEO-5 | P2 | — | Todo |
| 26 | [Log panel timeline](#wp26--log-panel-timeline) | UI-11 | P3 | — | Todo |
| 27 | [Feature tests for command features](#wp27--feature-tests-for-command-features) | API-11, API-6 | P3 | 8 | Todo |
| 28 | [Docs and policy](#wp28--docs-and-policy) | API-1, API-4, API-7, API-8, API-9, API-12, API-13, SVC-15, SVC-17 (policy) | P3 | — | Todo |

## Decisions needed before starting

These change the scope of a package; settle them before it starts.

| Package | Question | Decision |
|---|---|---|
| 3 | Also regenerate the dialect in the mavio fork from current MAVLink definitions, or only make Groundhog tolerant? | Tolerance now; the fork bump is a separate change. |
| 11 | Detect a WPL file without a home row (row 0 is home if its `current` column is 1, or it is a `NAV_WAYPOINT` in an absolute frame), or park MISSION-7? | Park MISSION-7; row 0 stays home. |
| 17 | Drop the "clear stale position" part of SVC-3 and show the age of the data instead? | Keep the last known values, show their age and dim them; only a zeroed `HOME_POSITION` clears home. |
| 20 | With pending edits, does "Save to a file" include them, or save reported values and say so? | Save reported values and say the pending edits were not included. |

## Claims to reframe in `OPEN_ISSUES.md`

When the package that would have applied the issue's own **Fix:** starts, move
that part to Appendix B with one sentence:

- **MISSION-6's "refuse to write a home row"** (package 11): the WPL format
  requires one; the fault is reading a zero home back as real.
- **SVC-3's clearing of position** (package 17, if decided as leaning).
- **UI-3's "report the expanded sheet's area"** (package 23): nothing of the
  map shows while the sheet is expanded; the collapsed sheet is what covers it.

---

## Phase 1 — safety and whole-feature failures (P0, P1)

### WP1 · Empty plans upload nothing
**Closes:** MISSION-1, CAL-8 · **Status:** Done

`codec::encode` inserts a home item whenever the layout has a home slot, so an
empty plan travels as `MISSION_COUNT = 1` and ArduPilot appends a waypoint at
0,0. Ship this alone, before the larger mission changes.

- `codec::encode` returns no items for an empty plan, whatever the layout: the
  home slot exists only when there are items to follow it. `mission_upload`
  then sends `MISSION_COUNT = 0`, which both ArduPilot and PX4 complete with an
  ACK.
- `encode` returns `Result` and refuses a plan longer than the wire can number
  (`u16::MAX` minus the home slot); `MissionLayout::wire_seq` uses checked
  arithmetic. `upload_mission` maps the refusal to a `services::Error`.
- Update `encode`'s doc: the zero home placeholder is sent only beside items.

**Tests:** an empty plan encodes to nothing under the ArduPilot layout; an
oversized plan is refused; `services/tests.rs` uploads an empty plan and the
mock vehicle receives count 0.

### WP2 · Raw mission items
**Closes:** PROTO-1 (mission half) · **After:** 1 · **Status:** Done

`MissionItem.command` is a `MavCmd` and `frame` a `MavFrame`, so an item whose
command or frame is newer than the dialect cannot be represented: its
`MISSION_ITEM_INT` fails to decode and the download re-requests it until it
times out. The module doc's promise that unknown commands round-trip is false.

- `MissionItem` holds `command: u16` and `frame: u8`, with `known_command() ->
  Option<MavCmd>` and `known_frame() -> Option<MavFrame>`. Constructors keep
  taking typed values. `SPECS` stays keyed by `MavCmd`; an unknown command has
  no spec and is edited through the raw `Param`/`X`/`Y`/`Z` fields that exist
  for commands without metadata, labelled "Command 31050".
- A crate-private `wire::mission_item` encodes and decodes the fixed
  `MISSION_ITEM_INT` payload layout (including the `mission_type` extension
  and MAVLink 2 truncation). The outbound form implements `mavio::Message`, which
  `MavlinkRouter::route` already takes as `&dyn Message`; `MissionListener`
  decodes items with it instead of `DefaultDialect`.
- `codec` and `storage::mission` work on the raw form; WPL rows with unknown
  commands now load and save.
- The GUI's command picker and map read `known_command()`.

**Tests:** a mock vehicle serves an item with an unknown command and frame; the
download succeeds and re-uploading sends identical bytes. A WPL row with an
unknown command round-trips. Existing codec and editor tests still pass.

### WP3 · One tolerant decoder
**Closes:** PROTO-1 (status-message half) · **Status:** Done

Four places decode frames on their own (`services/runtime.rs`,
`MissionListener::next`, the router's ACK routing, `mavlink/recv.rs`), and only
the runtime special-cases one message (`MAG_CAL_REPORT`). A heartbeat with a
newer `MavType` never registers its vehicle.

- One `pub(crate)` decode function in `mavlink/`, used by all four.
- On `SpecError::InvalidEnumValue` it consults a short table of messages
  whose enum fields have a meaningful fallback, patches any unknown value at
  its offset to that fallback, and decodes again:
  - `HEARTBEAT`: type → `MAV_TYPE_GENERIC` (classified `VehicleClass::Other`,
    still a vehicle); autopilot → `MAV_AUTOPILOT_GENERIC` (so parameter
    writes stay refused).
  - `COMMAND_ACK`: result → `MAV_RESULT_UNSUPPORTED` (values 7–9 are all
    "not this way"), which triggers the existing fallbacks.
  - `MISSION_ACK`: type → `MAV_MISSION_ERROR`.
  - `MAG_CAL_REPORT`: status → `MAG_CAL_FAILED` (moved from the runtime).
- Each patch logs once at debug with the raw value. Messages not in the table
  keep being dropped, and the `AGENTS.md` pitfall says to add a row.

**Tests:** a heartbeat with an unknown `MavType` registers its vehicle; one
with an unknown autopilot registers it with generic firmware; a `COMMAND_ACK`
with result 8 reaches its transaction as unsupported; the existing
`MAG_CAL_REPORT` test passes through the shared decoder.

### WP4 · Parameter channel lock
**Closes:** PARAM-1 · **Status:** Done

A list download and a set both consume `PARAM_VALUE` from the same target, so a
list value can satisfy a set.

- `LockMap` gains shared and exclusive modes for one new key,
  `LockKey::ParamChannel(target)`. `param_list` holds it exclusively;
  `param_set` holds it shared, plus its existing per-parameter lock, so sets of
  different parameters still run side by side inside the permit window.
- The preference for a reply equal to the request stays: another GCS's
  download or the vehicle's own broadcasts cannot be locked out.
- The parameter panel disables Upload and Upload all while its component
  downloads, with a hint saying why.

**Tests:** in `mavlink/tests.rs`, a set started during a download waits for it,
and a list value for the same ident does not answer it; two sets of different
parameters still overlap. A panel test: Upload is unavailable during a download.

### WP5 · One parameter download model
**Closes:** PARAM-2, PARAM-6, PARAM-7, SVC-1, API-5, LINK-3, LINK-5, SVC-11 ·
**After:** 4 · **Status:** Done

`Parameters` keeps two records of one download: `LoadingState`, fed by the
fold, and `DownloadState`, set from the transaction's result. They disagree
(a refused value counts in one and not the other), the call's return value
gives a third verdict, and nothing removes a parameter the vehicle no longer
has.

- Replace both with one `ParamDownload` in `Parameters`: its phase, the
  advertised count, the indices received, how many values were refused, and a
  generation per download.
- The fold notes an index even when it refuses the value, and counts the
  refusal. `value_from_c_cast` returns `None` for non-finite and out-of-range
  values, so a sentinel is refused rather than shown as an editable value
  (LINK-3). The `wire::param` module doc names the unsupported 64-bit types
  (LINK-5).
- Each stored parameter remembers the generation it was last reported in. A
  download that completes removes parameters it did not report; a failed or
  cancelled one keeps them.
- The transaction returns what happened, including "nothing ever arrived".
  The service turns that into a `DownloadOutcome`: `Complete`,
  `Incomplete(Progress)`, `NoParameters`, or `Failed(String)` (the snapshot
  cannot hold a services `Error`). It stores the outcome in the snapshot
  **and** returns the same value, so the panel's notice and banner cannot
  disagree. `NoParameters` shows as a warning with a reason and Retry, not a
  danger banner.
- One `Progress { done, total }`, shared from the crate root and re-exported
  by services, replaces `TransferProgress`, `ParamListSummary` and
  `(usize, u16)`.
- `on_param_value` uses `component_mut_if` for an unchanged repeat (SVC-11).

**Tests:** a mock that answers `PARAM_REQUEST_LIST` with nothing gives
`NoParameters`; a partial download gives the same `Incomplete` in snapshot and
result; a refused value still completes the download; a parameter missing from
a complete reload disappears, and survives an incomplete one; a NaN C-cast
value is refused.

### WP6 · PX4 modes per class
**Closes:** CAL-1 · **Status:** Done

- Per-class tables in `firmware/px4.rs`, as `ardupilot.rs` has: multicopter,
  fixed-wing, VTOL, rover and boat, submarine. Takeoff and Land go to aircraft
  only, Precision Land to multicopters and VTOL, VTOL Takeoff to VTOL.
- `decode_mode` looks up the union of the tables, so a mode the vehicle
  *reports* outside its class's table still gets its name; `modes(class)`
  decides only what is offered.

**Tests:** `modes()` and `mode_of_kind()` per class; a rover offers no Land,
so the quick Land button is hidden; a reported mode outside the class's table
still decodes.

### WP7 · Overlays that fit the window
**Closes:** UI-1, UI-10, UI-13 (dialog and toast) · **Status:** Done

- Move the confirmation dialog and its backdrop into `ui::dialog`, and the
  toast into `ui::toast`. Both take the platform insets.
- The dialog fills the width up to a max-width token, padded by `space::L`
  plus the insets. Its body scrolls, so the buttons stay on screen in a short
  landscape window. On a narrow window the buttons stack.
- Toasts keep clear of the system bars; so does the maximized-pane fallback in
  `app/tiled.rs`.

**Tests:** a `Simulator` test at 400×700 and at 700×320: the accept button is
inside the window and pressing it answers. Gallery before and after.

---

## Phase 2 — degraded behaviour and correctness edges (P2)

### WP8 · Shared feature test harness
**Closes:** API-10 · **Status:** Done

Done first in this phase because packages 9, 12, 19 and 27 add panel tests.
Extract `#[cfg(test)] features/test_support.rs` with the `Env { fleet, handle,
notices, settings }` and `step` that three test modules now define each, and
move those modules onto it.

### WP9 · Calibration session lifecycle
**Closes:** CAL-2, CAL-3, CAL-4, CAL-5, CAL-6, CAL-7, CAL-9, SVC-14 ·
**After:** 8 · **Status:** Done

- **Confirmation sent** (CAL-2): `CalibrationUpdate::Confirmed(side)` sets
  `Instruction::Confirming(side)` ("Waiting for the vehicle…"), and
  `calibration_controls().confirm` is false until the vehicle's next progress.
  The panel runs the confirmation through `Operations`, so it is busy meanwhile.
- **Reboots** (CAL-3): the vehicle tracks a boot count, advanced when the
  autopilot's `time_boot_ms` goes backwards, and the fleet emits
  `FleetEvent::Rebooted`. This is vehicle state other subsystems can use later
  (mission and parameter staleness).
- **One way to end a session** (CAL-3, SVC-14): `Vehicle::end_calibration(reason)`
  is used for loss, reboot and a no-progress timeout. The timeout runs only
  while the instruction is not waiting on the operator, so a vehicle waiting to
  be placed on its side does not time out.
- **Cancel** (CAL-4, CAL-7): the protocol supplies the caption for a kind that
  cannot be cancelled (shown only while the start is pending), and the retry
  count for its cancel command.
- **PX4 parsing** (CAL-5, CAL-6): parse the per-side compass progress; read the
  protocol version; an unmatched `pending:` side is noted, not recorded as done.
- **Tests** (CAL-9): the compass `pending:` to next-side transition, per-side
  progress text, timeout while not waiting on the operator, reboot ends a run,
  and a panel test of a whole accelerometer run.

### WP10 · Mission transfer identity and cancellation
**Closes:** PROTO-3, PROTO-6 · **After:** 2 · **Status:** Todo

- **Correlation** (PROTO-3): an upload of `n > 0` items accepts an `Accepted`
  ACK only after it served item `n - 1`; an error ACK is accepted at any time,
  since the vehicle may refuse the count at once. A download takes the first
  `MISSION_COUNT` only after its own `MISSION_REQUEST_LIST` was sent; the
  listener is already installed before that send.
- **Cancellation** (PROTO-6): `COMMAND_CANCEL` and `MISSION_ACK(cancelled)`
  go through one router helper that `Drop` can call: it spawns a short task that
  waits a bounded time for room in the control queue and logs a failure at
  warn, instead of discarding a `try_send` error.

**Tests:** a stale `Accepted` ACK from a timed-out upload does not complete the
next one; dropping a command future sends `COMMAND_CANCEL`, even when the
control queue is momentarily full.

### WP11 · Mission model at the boundaries
**Closes:** MISSION-5, MISSION-6, MISSION-7, MISSION-9, MISSION-10, MISSION-11 ·
**After:** 2 · **Status:** Todo

- **Planned home** (MISSION-6): `MissionPlan` gets `home: Option<Position>`,
  set from a loaded file and used when saving one. A WPL home of exactly zero
  reads as `None`, the same rule as other "unknown" sentinels, so the
  placeholder the format requires is never taken for a real home. Saving uses
  the vehicle's reported home if the draft follows one, else the planned home,
  else the zero placeholder.
- **Files without a home row** (MISSION-7): per the decision above; parked by
  default.
- **Fields follow the frame** (MISSION-9): `fields()` offers latitude and
  longitude in degrees only where `position()` would return one; local frames
  show raw `X`/`Y`/`Z`.
- **Validation** (MISSION-5, MISSION-11): `nan` only for optional fields;
  non-finite values refused for coordinates and altitude; WPL coordinates
  outside ±90/±180 refused; a UTF-8 BOM stripped.
- **Jump targets** (MISSION-10): removing an item maps a target pointing at it
  to its successor, or clears it when there is none; `shift_targets` skips a
  target below home instead of clamping it to 0.

**Tests:** load → save → load keeps a real home and invents none; a local-frame
item shows raw fields; `nan` latitude and `inf` altitude are refused; a BOM file
loads; out-of-range coordinates are refused; removing a jump's target, and the
last item, leave valid targets.

### WP12 · Mission panel during transfers and loads
**Closes:** MISSION-2, MISSION-3, MISSION-4, MISSION-8 · **After:** 8, 11 ·
**Status:** Todo

- Selection and Deselect are not edits: they leave `edit()` and always apply.
  The editor's fields and the row actions are disabled (not silently ignored)
  while a transfer runs.
- `Loaded` goes through the same gate: refused with a notice during a transfer,
  and confirmed through a `Confirmation` when the draft is modified.
- Reword the "changed elsewhere" banner to match the upload confirmation.
- `MissionDraft::follow`: skip the `seen` update when it declines, so a
  reverted draft adopts the onboard mission; test it.

**Tests:** during a transfer a row click selects and an edit does nothing;
loading over a modified draft asks; a revert after a declined follow adopts the
onboard mission.

### WP13 · Router link lifecycle
**Closes:** PROTO-4, PROTO-5 · **Status:** Todo

- **Closed means removed** (PROTO-5): split the link's cancellation token into
  "shutdown requested" and "closed". The reader's tail cancels "closed" only
  after `remove_link`, and `LinkGuard::closed()` waits for that one, so the
  supervisor never sees a link as lost while `route()` can still pick it.
- **The writer owns the heartbeat** (PROTO-4): the writer task sends the
  heartbeat itself on its interval, so it never waits behind a full queue,
  and the separate heartbeat task goes. Each write has a stall timeout; a
  stalled write closes the link and the supervisor reconnects.
- `LinkStats` gains `last_tx`.

**Tests:** after `closed()` resolves, routing to that link fails; a writer that
never completes a write closes the link within the stall timeout; a flood of
bulk traffic does not delay the heartbeat.

### WP14 · Duplicate suppression by stream position
**Closes:** PROTO-2 · **Status:** Todo

A window of 192 packets cannot catch a backlog longer than itself. Instead of a
larger window, track where each link is in a source's stream:

- Each source counts the packets it accepted (its stream position). Each
  (source, link) pair remembers the position and sequence of the last
  duplicate that link delivered, which marks the link as behind.
- A later packet on a link that is behind is placed by its sequence difference
  from that duplicate (which accounts for the link's own losses) and is dropped
  while it does not pass the newest accepted packet. Once it does, the link is
  no longer behind.
- A link with its own sequence counter (a separate autopilot channel) never
  delivers a duplicate, so it is never held back. The reset-gap flush stays.
- Rewrite the module doc, which says "however many arrive in a row".

**Tests:** a backlog of 500 packets is dropped in full; an independent channel
is unaffected; a lagging link that catches up takes over when the fast link
goes silent.

### WP15 · Link management by id
**Closes:** LINK-1, LINK-7, SVC-8, PARAM-10, API-2 · **After:** 13 · **Status:** Todo

- Replace `LinkManager::save` with `add(config) -> Result<LinkId>` and
  `update(&LinkId, config) -> Result<Saved>`. Both refuse a name another link
  has (`LinkConfigError::DuplicateName`). `LinkConfigurator` carries the edited
  link's `Option<LinkId>`.
- `LinkManager::update_restarts(&LinkId, &config) -> bool` lets the panel ask
  through a `Confirmation` before an update disconnects a live link.
- Link events only on an actual change: the first event no longer repeats the
  seeded status.
- Serial-port detection runs through `Operations`.
- `LinkManager::new` becomes `pub(crate)`; enable the `private_interfaces`
  lint in the workspace lint set.

**Tests:** renaming a link keeps one link with the old id; a duplicate name is
refused; changing a live link's transport asks first; the first link event
differs from its previous status.

### WP16 · UDP framing per datagram; serial opens
**Closes:** LINK-2, LINK-6, LINK-4 · **After:** 13 · **Status:** Todo

- Listen-mode UDP checks each datagram's MAVLink framing (magic byte, declared
  payload lengths that add up to the datagram) before passing it on. That is
  framing, not messages, so `link/` still knows nothing about MAVLink messages.
  A peer is admitted to the table only after a plausible datagram, and an
  implausible datagram is dropped whole instead of spliced onto the next.
- A serial reconnect waits for the previous blocking open to return before
  starting another, so retries cannot contend for the device.

**Tests:** random datagrams from a new source do not admit it and do not delay
the next real frame; a truncated datagram does not corrupt the following one.

### WP17 · Vehicle state: freshness and health
**Closes:** SVC-3, SVC-4, SVC-12, SVC-16 · **Status:** Todo

- **Freshness** (SVC-3): position, velocity and GPS carry when they were
  received; views show the age next to a stale or lost vehicle and dim it. A
  zeroed `HOME_POSITION` clears home. Scope per the decision above.
- **Health** (SVC-4): a sensor's `healthy` is judged over its *enabled* flags
  (over the present ones only when none is enabled), the same rule as
  `enabled`; `failing()` follows.
- **Names** (SVC-12): `string_from_cstring` decodes lossily, like status texts.
- **Events** (SVC-16): `ArmedChanged` and `ModeChanged` carry `from` as well
  as `to`; the before values are already computed in `Fleet::observe`.

**Tests:** a healthy primary with a disabled backup is healthy; a zeroed home
clears it; one invalid byte keeps the rest of a vendor name; mode and arm
events carry both values.

### WP18 · Fold cost
**Closes:** SVC-9, SVC-10 · **Status:** Todo

- Keep the pending `STATUSTEXT` chunks outside the shared `StatusLog`, so a
  chunk does not deep-copy the log.
- Store the trail as chunks behind `Arc` (a few dozen points each): adding a
  point copies the list of chunk pointers and the last chunk, not the trail.

**Tests:** behaviour tests stay as they are; add one asserting that a snapshot
taken before a fix shares all but the last chunk with the one after.

### WP19 · Test-only fleet folding
**Closes:** SVC-13 · **After:** 8 · **Status:** Done

Add a `test-support` feature to `groundhog-logic`, enabled only from
`groundhog-gui-iced`'s dev-dependency. `Fleet::observe` and
`Fleet::set_onboard_mission` become `pub(crate)`, re-exported publicly only
under that feature. Note the feature in `AGENTS.md`'s testing section.

### WP20 · Parameter files
**Closes:** PARAM-3, PARAM-4, PARAM-5, PARAM-8, PARAM-9, SVC-17 (parameter
files) · **After:** 5 · **Status:** Todo

- One reader policy for `.ini`, `.param` and `.params`: a bad line is skipped
  and reported as a typed `LineError`, replacing `Result<_, String>`. The
  reader returns the values with the file's address (where the format has one),
  the skipped lines, the truncated names and the duplicates.
- `ParamEdits::load` reports all of it in the load summary, including "N values
  were for another component", and leaves those values out (PARAM-4).
- Saving is disabled while the encoding is unknown, with a reason (PARAM-3).
- Saving with pending edits says they were not included (PARAM-8, per the
  decision above).
- `SaveTo` and `Loaded` get a summary `Debug` (PARAM-9).

**Tests:** each format with a bad line, an unknown type, a 17-byte name and a
duplicate gives the same summary; a `.params` file for another component loads
nothing and says so; Save is unavailable while the encoding is unknown.

### WP21 · Recording and runtime robustness
**Closes:** SVC-2, SVC-5, SVC-6, SVC-7, API-3 · **Status:** Todo

- The recorder's channel is bounded, and each entry carries its receive time,
  which the tlog writes (SVC-5). Overflow drops the oldest and is counted.
- A write error ends the recording visibly: `TlogRecorder` exposes its status,
  and the frontend raises a notice. `stop()` is async and returns a `Result`
  after the final flush (SVC-2).
- The `Backend` watches the runtime task; a panic becomes a lasting
  `BackendEvent` that the UI shows as a banner ("vehicle state stopped
  updating") rather than a silently frozen snapshot (SVC-6).
- `Rejection` and `MissionRejection` are built only from failure values: replace
  the `From` conversions that map `Accepted` to `Unknown` (SVC-7).
- Write down the service layer's result convention in `services/mod.rs` and
  `AGENTS.md` (API-3).

**Tests:** a writer that fails reports it through the status and `stop()`; the
tlog's timestamps are receive times; a panicking fold produces the backend
event.

### WP22 · One finger tracker
**Closes:** UI-7, UI-8 · **Status:** Todo

Three widgets follow a finger, and two of them get it wrong. Add one
`widget::touch` tracker that follows one finger by id with its own position,
ignores other fingers, and ends on lift or `FingerLost`. Use it in
`drag_source` (publishing `on_end` on every terminal path) and in the vendored
`pane_grid` (record the change in its header), and move the map overlay onto
it.

**Tests:** a second finger neither starts nor ends a `drag_source` drag; a lost
finger publishes `on_end`; a pane-grid divider drag follows the finger, not the
cursor.

### WP23 · Layout, density and settings
**Closes:** UI-2, UI-3, UI-4, UI-5, UI-6, UI-9, UI-12, UI-13 (rest) ·
**After:** 7 · **Status:** Todo

- The tiled rail scrolls when it does not fit (UI-2).
- The map's covered area is always the collapsed sheet's, whether or not the
  sheet is expanded (UI-3).
- `Settings::sanitized()`, applied on load, clamps the scale factor and the
  waypoint altitude to the same ranges the settings controls use, defined once
  (UI-4).
- Density-dependent width tokens replace `HEADER_HEIGHT`, `INPUT_WIDTH`, the
  mode picker's and the Arm button's fixed sizes (UI-5); the unlabelled
  checkbox gets a `size::control()` hit area (UI-6); icons scale with density.
- The menu opens on the side with room, or clamps and scrolls (UI-9).
- The scale preview is cancelled when the slider loses focus (UI-12).
- The remaining UI-13 items: `ui::text_input_marked`, the "More" trigger's
  state, and the attitude docs' ±40°.

**Tests:** a short window reaches the last rail button; loading a scale factor
of 0 gives the minimum; centring with an expanded sheet places the vehicle in
the collapsed sheet's visible area. Gallery before and after.

### WP24 · Map engine
**Closes:** MAP-1, MAP-2, MAP-3, MAP-4, MAP-5, MAP-6, MAP-7 · **Status:** Todo

- `MapView::key()` includes the visible rectangle (MAP-1); `visible()` clamps
  its origin too (MAP-6).
- Each layer's handles are cached per (revision, view key) and rebuilt only
  when either changes; `mouse_interaction` reads the cache (MAP-2).
- A debug assertion that layer ids are unique per frame (MAP-3).
- Hover is recomputed after any view change: wheel zoom, momentum, `center_on`
  (MAP-4).
- A drag's origin is `Origin::Geo` or `Origin::Screen`, so cancelling a
  screen-anchored drag restores it exactly (MAP-5).
- `Layout::place` finds the neighbour before removing the panel (MAP-7).

**Tests:** in `map/overlay.rs`: a cached layer redraws when the visible
rectangle changes; handles are built once across pointer moves; hover clears
after a zoom; a cancelled screen-anchored drag returns to its origin.

### WP25 · Video receiver
**Closes:** VIDEO-1, VIDEO-2, VIDEO-3, VIDEO-4, VIDEO-5 · **Status:** Todo

- **No overlap** (VIDEO-2): `Receiver::replace(self, config)` gives the old
  supervisor's join handle to the new one, which waits for it on its own thread
  before opening a pipeline; the UI never blocks. The stop signal becomes a
  channel instead of a flag polled every 100 ms.
- **Terminal failures** (VIDEO-3): GStreamer errors that cannot heal by waiting
  (resource not found or not authorized, a missing plugin or decoder) end in
  `Status::Failed`, which the panel shows with a Retry; the rest keep
  retrying.
- **Backoff** (VIDEO-4): reset only after an attempt streamed for a few
  seconds, not after any frame ever.
- **Tests** (VIDEO-1): an RTSP test exercising `select-stream`, and a test of
  the retry and backoff decisions with the pipeline factored out.
- **Cleanups** (VIDEO-5): `config-interval=-1` on every H.264 path; `Unknown`
  colour matrix not taken for BT.709; a failed supervisor spawn reports
  `Failed`; frame generation assigned under the lock and advanced by `clear()`.

---

## Phase 3 — maintainability and polish (P3)

### WP26 · Log panel timeline
**Closes:** UI-11 · **Status:** Todo

Keep the merged, filtered timeline in the panel, updated incrementally from
the status logs' and notices' revisions, instead of collecting and sorting
~8500 entries per view. The header counts the entries before truncation.

### WP27 · Feature tests for command features
**Closes:** API-11, API-6 · **After:** 8 · **Status:** Todo

One `Context`/`Effect` test per command feature: arm and disarm confirmation,
a mission finished-failure notice, parameter save, load, cancel and progress.
While there, give each feature one `Finished(Key, Ticket, Result<Output>)`
message per operation kind (API-6).

### WP28 · Docs and policy
**Closes:** API-1, API-4, API-7, API-8, API-9, API-12, API-13, SVC-15,
SVC-17 (policy) · **Status:** Todo

Mostly writing; one commit per issue.

- API-8: fix the two sentences in `vehicle/mod.rs` and `parameter.rs`.
- API-9: a one-line `//!` for each vendored pane-grid file.
- API-1: `///` docs on `Vehicle`, `Identity`, `Battery`, `AttitudeState`,
  `PositionTrack`, `StatusLog`, `GlobalPosition` and their fields. Then decide
  on `missing_docs` and record the decision in `AGENTS.md`.
- API-4, SVC-17: the error policy in `AGENTS.md` (structured errors in the
  libraries, `String` only for display at the boundary), and the
  `ServiceResult` import alias in features.
- SVC-15: document the four feature update shapes and three persistence routes
  in `features/mod.rs` and `AGENTS.md`.
- API-7: decide on private identity fields with conversion methods; at least
  remove the `u64` to `u32` truncation in the map's helper ids.
- API-12: state in `AGENTS.md` that the gallery is a manual tool, and check that
  its `XDG_*` directories are throwaway.
- API-13: no change unless the layout schema must evolve; move to Appendix C.
