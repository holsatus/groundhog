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

Phase 1 of the first review (packages 1–7) is done and removed; packages
29–40 come from the 2026-11-04 review (R-1104) and now form Phase 1.

| # | Package | Closes | Priority | After | Status |
|---|---|---|---|---|---|
| 29 | [Panic-free mission references](#wp29--panic-free-mission-references) | MISSION-12, MISSION-13, MISSION-14, MISSION-15, MISSION-16 | P0 | — | Done |
| 30 | [Only intact frames reach the router](#wp30--only-intact-frames-reach-the-router) | PROTO-7 | P1 | — | Done |
| 31 | [Parameter downloads and edits in order](#wp31--parameter-downloads-and-edits-in-order) | PARAM-11, PARAM-12, PARAM-13, PARAM-15 | P1 | — | Done |
| 32 | [Calibration ends on both sides](#wp32--calibration-ends-on-both-sides) | CAL-10, CAL-11, CAL-12, CAL-13 | P1 | — | Done |
| 33 | [Command transaction timing](#wp33--command-transaction-timing) | PROTO-8, PROTO-9, PROTO-11, PROTO-12 | P1 | — | Todo |
| 34 | [Mission transfers known to the runtime](#wp34--mission-transfers-known-to-the-runtime) | MISSION-17, MISSION-18, MISSION-19, API-20 (transfer progress) | P1 | — | Todo |
| 8 | [Shared feature test harness](#wp8--shared-feature-test-harness) | API-10 | P3 (enabler) | — | Done |
| 9 | [Calibration session lifecycle](#wp9--calibration-session-lifecycle) | CAL-2, CAL-3, CAL-4, CAL-5, CAL-6, CAL-7, CAL-9, SVC-14 | P2 | 8 | Done |
| 10 | [Mission transfer identity and cancellation](#wp10--mission-transfer-identity-and-cancellation) | PROTO-3, PROTO-6 | P2 | 2 | Done |
| 11 | [Mission model at the boundaries](#wp11--mission-model-at-the-boundaries) | MISSION-5, MISSION-6, MISSION-9, MISSION-10, MISSION-11 | P2 | 2 | Done |
| 12 | [Mission panel during transfers and loads](#wp12--mission-panel-during-transfers-and-loads) | MISSION-2, MISSION-3, MISSION-4, MISSION-8 | P2 | 8, 11 | Done |
| 13 | [Router link lifecycle](#wp13--router-link-lifecycle) | PROTO-4, PROTO-5, LINK-8, LINK-9 | P2 | — | Todo |
| 14 | [Duplicate suppression by stream position](#wp14--duplicate-suppression-by-stream-position) | PROTO-2 | P2 | 30 | Todo |
| 15 | [Link management by id](#wp15--link-management-by-id) | LINK-1, LINK-7, SVC-8, PARAM-10, API-2 | P2 | 13 | Todo |
| 16 | [UDP framing per datagram; serial opens](#wp16--udp-framing-per-datagram-serial-opens) | LINK-2, LINK-6, LINK-4 | P2 | 13 | Todo |
| 17 | [Vehicle state: freshness and health](#wp17--vehicle-state-freshness-and-health) | SVC-3, SVC-4, SVC-12, SVC-20 | P2 | — | Todo |
| 18 | [Fold cost](#wp18--fold-cost) | SVC-9, SVC-10 | P3 | — | Todo |
| 19 | [Test-only fleet folding](#wp19--test-only-fleet-folding) | SVC-13 | P2 | 8 | Done |
| 20 | [Parameter files](#wp20--parameter-files) | PARAM-3, PARAM-4, PARAM-5, PARAM-8, PARAM-9, PARAM-14, SVC-17 (parameter files) | P2 | 31 | Todo |
| 21 | [Recording, persistence and runtime robustness](#wp21--recording-persistence-and-runtime-robustness) | SVC-2, SVC-5, SVC-6, SVC-7, SVC-21, API-3 | P2 | — | Todo |
| 22 | [One finger tracker](#wp22--one-finger-tracker) | UI-7, UI-8 | P2 | — | Todo |
| 23 | [Layout, density and settings](#wp23--layout-density-and-settings) | UI-2, UI-3, UI-4, UI-5, UI-6, UI-9, UI-12, UI-13 (rest) | P2 | — | Todo |
| 24 | [Map engine](#wp24--map-engine) | MAP-1 … MAP-7 | P2 | — | Todo |
| 25 | [Video receiver](#wp25--video-receiver) | VIDEO-1 … VIDEO-5 | P2 | — | Todo |
| 35 | [Decoding coverage](#wp35--decoding-coverage) | PROTO-10, CAL-14 | P2 | — | Todo |
| 36 | [Bootstrap lifecycle and readiness](#wp36--bootstrap-lifecycle-and-readiness) | SVC-18, SVC-19, API-16 | P2 | — | Todo |
| 37 | [Clients that match their target](#wp37--clients-that-match-their-target) | API-14, API-20 (flags) | P2 | 36 | Todo |
| 38 | [Observing the fleet](#wp38--observing-the-fleet) | API-15, API-18, SVC-16 | P2 | 36 | Todo |
| 39 | [Safety rules in the backend](#wp39--safety-rules-in-the-backend) | API-17 | P2 | — | Todo |
| 26 | [Log panel timeline](#wp26--log-panel-timeline) | UI-11 | P3 | — | Todo |
| 27 | [Feature tests for command features](#wp27--feature-tests-for-command-features) | API-11, API-6 | P3 | 8 | Todo |
| 28 | [Docs and policy](#wp28--docs-and-policy) | API-1, API-4, API-7, API-8, API-9, API-12, API-13, SVC-15, SVC-17 (policy) | P3 | — | Todo |
| 40 | [Public vocabulary](#wp40--public-vocabulary) | API-19, API-20 (rest), API-4 (R-1104 part) | P3 | 37, 38 | Todo |

## Decisions needed before starting

These change the scope of a package; settle them before it starts.

| Package | Question | Decision |
|---|---|---|
| 17 | Drop the "clear stale position" part of SVC-3 and show the age of the data instead? | Keep the last known values, show their age and dim them; only a zeroed `HOME_POSITION` clears home. |
| 20 | With pending edits, does "Save to a file" include them, or save reported values and say so? | Save reported values and say the pending edits were not included. |
| 32 | Is the 60 s no-progress timeout too strict? | Keep 60 s, but reset it on any feedback that belongs to the run, changed or not (CAL-10). Then it fires only when the vehicle has gone quiet about the calibration. |
| 33 | PROTO-9: drain stray ACKs after a resend, or accept the limitation and document it? | **Open.** Leaning: drain. It costs nothing until a command is resent, and then delays only the next command of the same kind to the same target, by at most one response timeout. If declined, do only the documentation step and move PROTO-9 to Appendix C. |
| 36 | After a reboot, re-run the whole bootstrap, or only parameters and home? | Leaning: the whole bootstrap; a reboot can follow a reflash or a mission change. The capability request is still skipped when the encoding is known. |
| 37, 38, 40 | Break the public API in place, or deprecate first? | Break in place, and update `groundhog-gui-iced` in the same commit: the crate has one frontend. |

## Claims to reframe in `OPEN_ISSUES.md`

When the package that would have applied the issue's own **Fix:** starts, move
that part to Appendix B with one sentence:

- **SVC-3's clearing of position** (package 17, if decided as leaning).
- **UI-3's "report the expanded sheet's area"** (package 23): nothing of the
  map shows while the sheet is expanded; the collapsed sheet is what covers it.

---

## Phase 1 — panics and wrong results (P0, P1)

Every package in this phase is a bug fix: per `AGENTS.md`, write the tests it
names first and see them fail. Line numbers are as of `757d6f4`; trust the
symbol.

### WP29 · Panic-free mission references
**Closes:** MISSION-12, MISSION-13, MISSION-14, MISSION-15, MISSION-16 ·
**Status:** Done

Item-reference parameters (`ParamKind::ItemIndex`, e.g. DO_JUMP's target) are
`f32`s that three places move with integer arithmetic. Give them one rule, and
narrow every editor value before checking it.

- **One rule** (MISSION-12): add `pub(crate) fn item_ref(param: f32) ->
  Option<u16>` in `domain/mission/item.rs`, next to `item_index_slots`. It is
  `Some` only for a finite value with no fractional part in `0.0..=65535.0`.
  Anything else (negative, huge, NaN, fractional) is *opaque*: no code moves
  it.
- `MissionPlan::remap_targets` (`plan.rs:306`) reads each target with
  `item_ref` instead of `target as usize`, skips opaque ones, and writes back
  a mapped value only if it is `<= u16::MAX`; otherwise it leaves the target
  as it was (its existing "`None` means untouched" rule). `insert`'s closure
  then cannot overflow.
- `codec::shift_targets` (`codec.rs:111`) reads each target with `item_ref`
  and shifts with `checked_add_signed`; an opaque value or a result outside
  `u16` is left as it was, except for the home slot below.
- **The home slot** (MISSION-13): under a home-slot layout, decoding maps wire
  target `0` to `-1.0` in the plan, and encoding maps exactly `-1.0` back to
  `0`. Document on `shift_targets` that `-1` in a plan means "the home slot".
  It is opaque to plan edits by the rule above. In the GUI, the editor shows
  an `ItemNumber` field holding `-1` as "Home"
  (`features/mission/view.rs`). WPL load/save go through `codec`, so they
  follow.
- **Narrow, then check** (MISSION-14), in `MissionItem::set_field`
  (`item.rs:353-373`):
  - `Altitude`/`Z` and plain params: `let narrowed = value as f32;` refuse
    unless `narrowed.is_finite()`.
  - `X`/`Y`: `i32::try_from` of `value.round()` (through an `f64` range check,
    since `f64` has no `TryInto<i32>`); refuse out of range.
  - `ItemIndex`: refuse a value outside `1.0..=65535.0` before storing
    `value - 1`.
- **WPL** (MISSION-14, MISSION-15, `storage/mission.rs`): `param1..4` refuse
  `±inf` (NaN stays accepted where WP11 allowed it). `row.seq =
  u16::try_from(rows.len())` with a new `WplError::TooManyRows`.
- **Limit** (MISSION-16): `MissionLayout::max_items` returns 65534 with a home
  slot and 65535 without; `MISSION_COUNT.count` is the `u16` that bounds them,
  not the highest sequence number. Correct the comment and
  `a_plan_that_the_wire_cannot_number_is_refused`.

**Tests:**
- A DO_JUMP whose target is `1e30`: `push`, `insert(0, ..)`, `remove`,
  `codec::encode` under the ArduPilot layout, and a WPL save neither panic nor
  change it.
- `set_field(ItemNumber, 1e30)` and `set_field(Altitude, 1e40)` return false;
  `set_field(X, 3e9)` returns false.
- Under the ArduPilot layout, a downloaded DO_JUMP to wire 0 re-encodes to 0;
  the same through WPL load and save.
- A plan of exactly `max_items()` encodes, and one more is `PlanTooLong`.

### WP30 · Only intact frames reach the router
**Closes:** PROTO-7 · **Status:** Done

The owner asked whether duplicate suppression should commit only after
"parsing and CRC-checking" the packet. A checksum check is what is needed;
full decoding is not. A frame that fails to decode because of an unknown
enum value is intact, and must still suppress its duplicates. Decoding stays
in the runtime's tolerant decoder.

- Move the `is_corrupt` check into the start of
  `MavlinkRouter::handle_frame`, so unit tests reach it, and check each frame
  once: `handle_frame` returns `Result<Option<Event>, Corrupt>` (a unit
  `struct Corrupt`). A corrupt frame returns `Err(Corrupt)` before
  `dispatch`: no suppression entry, no route refresh, no ACK routing, no
  transaction or recorder filter (the tlog recorder is a filter, so corrupt
  frames are no longer recorded), no event. The reader counts `Err` in
  `parse_errors` and `Ok` in `rx_frames` (`router.rs:727-745`), and keeps
  updating `last_rx` for both: bytes are arriving, so the link is alive.
  Existing tests that call `handle_frame` change mechanically
  (`.expect("intact")`).
- Frames of messages outside the dialect cannot be checked
  (`validate_checksum` has no CRC_EXTRA for them) and pass as today; say so
  where `is_corrupt` is defined.
- Rewrite the reader's comment ("It is still routed (decoding refuses it
  later)"), and add to `AGENTS.md`'s "Lossless router events; duplicates
  identified exactly": "only frames whose checksum verifies take part".
- PROTO-13 (the parser's resynchronisation) is a change in the mavio fork,
  not here; open it there and leave the entry.

**Tests** (router tests, with `handle_frame`):
- Build a `COMMAND_ACK` frame, serialize it, flip one payload byte, and parse
  it back. `handle_frame(A, corrupt)` is `None`, then `handle_frame(B,
  intact)` dispatches it, and `best_link_for(source) == Some(B)`.
- A corrupt frame registers no pending ACK delivery.

### WP31 · Parameter downloads and edits in order
**Closes:** PARAM-11, PARAM-12, PARAM-13, PARAM-15 · **Status:** Done

- **Start under the lock** (PARAM-11): `MavlinkRouter::param_list` takes a
  third argument, `on_started: impl FnOnce() + Send`, and calls it right after
  `lock_channel` returns (`mavlink/param.rs:148`), before the capability
  requests. In `VehicleClient::fetch_parameters`, `DownloadReport` gains a
  `started: AtomicBool` (the callback runs inside the future, so it must be
  `Send + Sync` to borrow). `DownloadReport::new` reports nothing;
  `started(&self)` reports `DownloadUpdate::Start` and sets the flag. `Drop`
  reports `Cancelled` only if started. `finish` reports `Start` first if the
  flag is unset, which should not happen, then the outcome. A download
  dropped while queued then leaves no trace in the snapshot.
- **Re-decode or drop** (PARAM-12): in `Parameters::set_encoding`
  (`domain/parameter.rs`), a parameter whose raw form the new decoder
  refuses is removed from the map, just as the fold does not store a value
  refused on arrival in `Parameters::observe` (use `retain`). If the removed
  value was stamped with the current download generation, it also counts in
  `ParamDownload::refused` (add a private `ParamDownload::refuse_one` for
  it), so `received - refused` stays the number stored from that download. A
  value from an older generation is only removed.
- **Loads clear stale edits** (PARAM-13): in `ParamEdits::load`, a loaded
  value equal to the reported value removes an existing edit for that ident
  unless its status is `EditStatus::Uploading` (that upload's result settles
  it). `LoadSummary` is unchanged: it counts neither equal values nor
  removed edits today.
- **C-cast range** (PARAM-15), `wire/param.rs`: `value_into_c_cast` returns
  `Option<(f32, MavParamType)>`. For `I32` and `U32` it is `None` when `v as
  f32` does not convert back to the same integer (above 2²⁴ some do, some do
  not). `ParamEncoding::encode` returns `Option` too (bytewise is always
  `Some`). `param_set` turns `None` into `MavError::ParamEncoding` before
  taking any lock or sending; the services layer already maps that to
  `Error::ParamEncoding`, which `set_parameter` documents. The other
  `encode` callers are tests; update them mechanically.

**Tests:**
- PARAM-11, in `services/tests.rs` (helpers: `start_download`,
  `param_value`, `until`, `parameters`). A queued download sends nothing and,
  after the fix, shows nothing in the snapshot, so there is no state to wait
  on: spawn it, then give it time to be queued with a short real sleep
  (e.g. 50 ms) before going on.
  - A is started and receives index 0 of 2. B is spawned and queued. A
    receives index 1 and completes. Both parameters are in the snapshot and
    the outcome is `Complete`; before the fix the first one is gone.
  - A is started and receives index 0 of 2. B is spawned, queued, and
    aborted (`JoinHandle::abort`). A completes. The outcome is `Complete`,
    with both values, and no `Cancelled` from B reaches the snapshot.
- PARAM-12: a C-cast `UINT8` with raw `-1.0`, folded while the encoding is
  unknown, then the encoding set to C-cast: the parameter is gone and counted
  as refused.
- PARAM-13: edit `GAIN = 5` over a reported 1, then load a file with
  `GAIN = 1`: no edit remains.
- PARAM-15: setting `I32(i32::MAX)` under C-cast is `ParamEncoding` and sends
  nothing; `I32(1 << 24)` is sent.

### WP32 · Calibration ends on both sides
**Closes:** CAL-10, CAL-11, CAL-12, CAL-13 · **Status:** Done

- **Liveness, not change** (CAL-10, part 1): `CalibrationProtocol::observe`
  returns `bool`: whether the feedback belonged to the running session,
  whether or not it changed it. `Vehicle::observe_calibration` sets
  `calibration_progress_at = Some(time)` when it returns true, replacing the
  `*session != before` test. What belongs:
  - ArduPilot: an `ACCELCAL_VEHICLE_POS`; a `MAG_CAL_PROGRESS` or
    `MAG_CAL_REPORT` for a compass in the run (the reports that
    `observe_compass_report` already rejects as from an earlier run do not
    count). Not its status texts: they are still noted in the session's log,
    but `mentions_calibration` matches any text with "compass", "mag", "gyro"
    and the like, and ArduPilot repeats pre-arm failures ("PreArm: Compass
    not calibrated") about every 30 s while disarmed, which would keep a
    stalled run alive forever. ArduPilot's runs report through those
    messages, so the texts add no liveness.
  - PX4: any `[cal]` text, repeats included. The note log still drops
    repeats.
  - `CALIBRATION_TIMEOUT` stays 60 s; rewrite its doc to "the vehicle said
    nothing about the calibration for this long".
- **Tell the vehicle** (CAL-10, part 2):
  - `Vehicle::end_calibration(reason)` returns `Option<EndedRun>` instead of
    `bool` (`None`: no run was going), with a crate-private `EndedRun {
    cancel: Option<CalibrationCancel> }` and `CalibrationCancel { target:
    MavlinkId, command: CommandLong, retries: u8 }`. Its fields come from the
    protocol, read before the session is failed: `protocol.cancel(session)`,
    `protocol.cancel_retries()`, and `vehicle.target()`.
  - `Fleet::tick` returns a crate-private `Tick { events: Vec<FleetEvent>,
    cancels: Vec<CalibrationCancel> }`, and collects the cancel of a run it
    ends by timeout or by loss of contact. The cancel for loss is best
    effort: the vehicle may come back still calibrating, and a send that
    finds no link just fails. A reboot ends the run on the vehicle's side, so
    `Fleet::observe` keeps discarding its result. Update the existing `tick`
    tests mechanically (`.events`).
  - The runtime (`services/runtime.rs`, the liveness branch) spawns each
    cancel as `router.command(target, command).retries(retries)` and logs a
    failure at warn. Use the task set from WP36 if it has landed, else a bare
    `tokio::spawn`.
- **One announcement** (CAL-11): in `Fleet::tick`'s `Lost` branch, emit
  `calibration_finished` only when `end_calibration` returned `Some`.
- **Confirming holds** (CAL-12): in `calibration/ardupilot.rs`
  `observe_position`, when the session's instruction is
  `Instruction::Confirming(side)` and the requested position is the same
  `side`, change nothing; return true (it is feedback for the run). A
  different side, success or failure proceed as today.
- **All compasses** (CAL-13), in `calibration/ardupilot.rs`:
  - `Session` keeps a crate-private `expected_compasses: u8`, the union of
    `cal_mask` from this run's `MAG_CAL_PROGRESS` only. Reports are left
    out: ArduPilot repeats the last run's reports, so their masks can be
    stale.
  - `observe_compass_report` accepts a report for a compass that has shown
    progress (as today), *or* whose bit is in `expected_compasses` once
    `session.observed` is set; such a compass gets its entry then. Its
    progress may have been lost on a saturated link, while its report is
    this run's.
  - The run finishes only when every compass in `expected_compasses` and
    every compass with an entry has a final status. A zero mask (firmware
    that does not fill it) reduces this to today's rule.

**Tests** (`domain/calibration/tests.rs` and `fleet.rs`, plus one in
`services/tests.rs`):
- 62 s of identical `MAG_CAL_PROGRESS` (87 %) at 1 Hz: still running. Then
  61 s of silence about the calibration, with heartbeats still arriving (a
  vehicle silent for 10 s is `Lost`, which ends the run first): failed with
  "the calibration made no progress", and `Tick::cancels` holds
  `DO_CANCEL_MAG_CAL` for the vehicle. The same with PX4's identical
  `[cal] progress` texts.
- In `services/tests.rs` (module `calibration`), an ArduPilot compass run
  whose mock keeps heartbeating but stops reporting: the mock receives
  `DO_CANCEL_MAG_CAL` after the timeout.
- Loss of contact during a compass run also yields a cancel in
  `Tick::cancels`; a reboot does not.
- A gyro calibration that succeeded, then the vehicle goes `Lost`: no second
  `CalibrationFinished`.
- `Place(Level)`, `Confirmed(Level)`, then `ACCELCAL_VEHICLE_POS(Level)`: the
  instruction stays `Confirming(Level)`, and `calibration_controls().confirm`
  is false.
- Progress for compass 0 with `cal_mask = 0b11`, then a success report for
  compass 0: still running; a report for compass 1 (which never sent
  progress) finishes it. A report for compass 2, outside the mask and
  never seen, is ignored.

### WP33 · Command transaction timing
**Closes:** PROTO-8, PROTO-9, PROTO-11, PROTO-12 · **Status:** Todo ·
**Decision:** PROTO-9 (above)

All in `mavlink/transaction.rs` and `mavlink/mission.rs`.

- **Progress keeps the caller's patience** (PROTO-8): in
  `CommandRequest::run`, on `IN_PROGRESS` set `timeout =
  self.response_timeout.max(PROGRESS_TIMEOUT)`. Keep the cancel that
  `Pending::drop` sends when a command in progress is given up on, and say in
  `CommandRequest`'s doc that giving up cancels it, as dropping does.
- **Drain stray ACKs** (PROTO-9, if decided):
  - `run` counts the attempts it sent. When it completes with more than one
    sent, it does not drop its `Pending` and lock at once. It hands both to a
    drain that receives and discards at most `attempts - 1` further ACKs, for
    at most `response_timeout`, then drops them. The caller gets its result
    without waiting.
  - When `run` acquired the lock itself (the common case), the drain is a
    spawned task owning `Pending` and the `KeyGuard`; `KeyGuard` is already
    owned, so this means keeping it in a local rather than a `&mut`
    temporary. When the guard is borrowed (`command_with_lock`, used by
    `request_message`), drain inline before returning: its callers are
    background requests.
  - `Pending::in_progress` is false after completion, so the drain never
    sends a cancel.
  - If declined: document the limitation at `CommandRequest` and in
    `AGENTS.md`'s Pitfalls, and move PROTO-9 to Appendix C.
- **No component 0** (PROTO-11): `debug_assert_ne!(target.component, 0)` in
  `run`, with a comment on `route_ack` explaining the system-wide fallback is
  for ACKs that name no target. Do not build system-wide locking until a
  caller needs it.
- **Clear ignores stale ACKs** (PROTO-12): in `mission_clear`, set
  `listener.not_before = Some(Instant::now())` before the first send, as the
  download does (`mission.rs:454`).

**Tests** (`mavlink/tests.rs`, paused clock):
- `.timeout(10 s)`, the mock answers `IN_PROGRESS` and then `ACCEPTED` 7 s
  later: `Ok`, and no `COMMAND_CANCEL` is received.
- `.timeout(1 s)`, `IN_PROGRESS`, then silence: `Timeout`, and
  `COMMAND_CANCEL` is received (kept behaviour).
- PROTO-9: arm with a 100 ms timeout, two attempts, the mock ACKs after the
  resend; then disarm, the mock sends `ACCEPTED` (the straggler) then
  `DENIED`: the disarm returns `Rejected(Denied)`. A command with no resend
  releases its lock at once (a second command starts without delay).
- PROTO-12: an `Accepted` `MISSION_ACK` queued before `mission_clear` sends
  does not complete it.

### WP34 · Mission transfers known to the runtime
**Closes:** MISSION-17, MISSION-18, MISSION-19, API-20 (transfer progress) ·
**Status:** Todo

Parameter downloads already report to the runtime from start to end (WP5,
WP31). Give mission transfers the same shape, and the race and the callback go
away together.

- **Inputs:** `runtime::Input` gains `MissionTransfer { system, update:
  TransferUpdate }`, with `TransferUpdate::{Start(Direction), Progress(Progress),
  End}` and `Direction::{Upload, Download, Clear}`. A drop guard in
  `services/vehicle.rs`, shaped like `DownloadReport`, reports `Start`
  before the router call, `Progress` from the router's progress callback,
  and `End` when dropped. It drops after the `Input::OnboardMission` result
  has been reported, so the runtime sees the result first.
- **State:** `VehicleMission` gains `pub transfer: Option<MissionTransfer {
  direction, progress }>`, set by `Start`/`Progress` and cleared by `End`.
  `onboard_is_stale()` returns false while `transfer.is_some()`
  (MISSION-17). `Fleet::observe` already compares stale-before and
  stale-after, so no event fires mid-transfer. After `End`, the next
  `MISSION_CURRENT` decides as usual.
- **API** (API-20's progress part): `download_mission` and `upload_mission`
  lose their `progress` parameter; the GUI reads progress from
  `vehicle.mission.transfer`, as it reads `ParamDownload`. Bootstrap's
  download becomes visible to every reader.
- **Id-less change detection** (MISSION-18): in `onboard_is_stale`, when
  `progress.mission_id` is `None` and `progress.total` is `Some(total)`,
  `usize::from(total) != onboard.items.len()` also means stale (`total`
  excludes home, as does `items`). Say in the doc that this catches only a
  changed item count.
- **Draft contract** (MISSION-19): document on
  `MissionDraft::transferred` that a frontend must not let the plan change
  while a transfer runs (the iced frontend's WP12 gate), and point to
  `VehicleMission::transfer` as the way to know.

**Tests:**
- Multi-thread runtime, PX4 mock, twenty rounds: upload, ACK with a new
  `opaque_id`, and the mock sends `MISSION_CURRENT` with that id
  immediately. No `OnboardMissionStale` is emitted.
- The same with `clear_mission`.
- During a slow upload a snapshot shows `transfer: Some(Upload, progress)`;
  after it, `None`. A dropped upload ends the transfer.
- No mission id, a reported total of 3 against an onboard mission of 2
  items: stale; equal counts: not stale.

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
**Closes:** PROTO-3, PROTO-6 · **After:** 2 · **Status:** Done

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
**Closes:** MISSION-5, MISSION-6, MISSION-9, MISSION-10, MISSION-11 ·
**After:** 2 · **Status:** Done

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
**Status:** Done

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
**Closes:** PROTO-4, PROTO-5, LINK-8, LINK-9 · **Status:** Todo

- **Closed means removed** (PROTO-5): split the link's cancellation token into
  "shutdown requested" and "closed". The reader's tail cancels "closed" only
  after `remove_link`, and `LinkGuard::closed()` waits for that one, so the
  supervisor never sees a link as lost while `route()` can still pick it.
- **The writer owns the heartbeat** (PROTO-4): the writer task sends the
  heartbeat itself on its interval, so it never waits behind a full queue,
  and the separate heartbeat task goes. Each write has a stall timeout; a
  stalled write closes the link and the supervisor reconnects.
- `LinkStats` gains `last_tx`.
- **Shutdown waits for the transport** (LINK-8): `LinkGuard::shutdown(self)`
  requests the shutdown and resolves once the reader and writer tasks have
  dropped their halves of the transport and `remove_link` has run. That is
  the "closed" token above, plus the writer's exit; the reader tail at
  `router.rs:769-772` must not hold the transport while it awaits
  `events.send`.
- **The supervisor is the only publisher** (LINK-9), in `services/link.rs`:
  - `Session` holds a `CancellationToken` and the supervisor's
    `JoinHandle`. `disconnect` cancels the token and moves the handle to
    `LinkEntry::closing`. It publishes nothing itself; `status()` of an
    entry without a session still reads `Disconnected` at once.
  - `supervise` races each of its awaits (open, `closed()`, the backoff
    sleep) against the token. On cancellation it calls
    `guard.shutdown().await` if attached, publishes `Disconnected`, and
    returns.
  - `connect` takes `closing` and passes it to the new supervisor, which
    awaits it before its first open (LINK-8). A reopen then never races the
    old socket or port, and every event of the old session precedes the
    new one's.
  - Dropping the `LinkManager` still aborts everything.

**Tests:** after `closed()` resolves, routing to that link fails; a writer that
never completes a write closes the link within the stall timeout; a flood of
bulk traffic does not delay the heartbeat. On a multi-thread runtime,
fifty times each:
- A fixed-port UDP link with `auto_reconnect: false`, disconnected and
  connected again, reaches `Connected`.
- Connect, wait 20 µs, then disconnect: the last event is `Disconnected`.

### WP14 · Duplicate suppression by stream position
**Closes:** PROTO-2 · **After:** 30 · **Status:** Todo

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
**Closes:** SVC-3, SVC-4, SVC-12, SVC-20 · **Status:** Todo

SVC-16 (`from`/`to` on events) moved to WP38, which reshapes `FleetEvent`
once.

- **Freshness** (SVC-3): position, velocity and GPS carry when they were
  received; views show the age next to a stale or lost vehicle and dim it. A
  zeroed `HOME_POSITION` clears home. Scope per the decision above.
- **Health** (SVC-4): a sensor's `healthy` is judged over its *enabled* flags
  (over the present ones only when none is enabled), the same rule as
  `enabled`; `failing()` follows.
- **Names** (SVC-12): `string_from_cstring` decodes lossily, like status texts.
- **Arming checks** (SVC-20): `Health::prearm_passes() -> Option<bool>`
  becomes `prearm() -> Option<Prearm>` with `Prearm::{Passing, Failing,
  Disabled}`: present but not enabled is `Disabled`. The vehicles panel
  (`features/vehicles/mod.rs`, the read-out near line 462) shows "Arming
  checks off" in a warning tone for `Disabled`.

**Tests:** a healthy primary with a disabled backup is healthy; a zeroed home
clears it; one invalid byte keeps the rest of a vendor name; `PREARM_CHECK`
present and not enabled is `Disabled`.

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
**Closes:** PARAM-3, PARAM-4, PARAM-5, PARAM-8, PARAM-9, PARAM-14, SVC-17
(parameter files) · **After:** 31 · **Status:** Todo

- One reader policy for `.ini`, `.param` and `.params`: a bad line is skipped
  and reported as a typed `LineError`, replacing `Result<_, String>`. The
  reader returns the values with the file's address (where the format has one),
  the skipped lines, the truncated names and the duplicates. Every reader
  refuses non-finite values the same way (PARAM-14: today the INI reader does
  not).
- `ParamEdits::load` reports all of it in the load summary, including "N values
  were for another component", and leaves those values out (PARAM-4).
- Saving is disabled while the encoding is unknown, with a reason (PARAM-3).
- Saving with pending edits says they were not included (PARAM-8, per the
  decision above).
- `SaveTo` and `Loaded` get a summary `Debug` (PARAM-9).

**Tests:** each format with a bad line, an unknown type, a 17-byte name and a
duplicate gives the same summary; a `.params` file for another component loads
nothing and says so; Save is unavailable while the encoding is unknown.

### WP21 · Recording, persistence and runtime robustness
**Closes:** SVC-2, SVC-5, SVC-6, SVC-7, SVC-21, API-3 · **Status:** Todo

- The recorder's channel is bounded, and each entry carries its receive time,
  which the tlog writes (SVC-5). Overflow drops the oldest and is counted.
  Each record (timestamp and frame) is built in one buffer and written with
  one `write_all`.
- A write error ends the recording visibly: `TlogRecorder` exposes its status,
  and the frontend raises a notice. `stop()` is async and returns a `Result`
  after the final flush (SVC-2). On stop, the writer first drains the frames
  already queued (`try_recv` until empty), then flushes. The app's close path
  (`app.rs`, beside the config writer's flush) awaits it.
- `persist` (SVC-21): `write_durably` creates the temporary file with the
  target's permissions on unix (`OpenOptions::new().write(true)
  .create_new(true).mode(..)`, from the target's metadata, else the default),
  before writing; elsewhere it keeps setting them after. `read_or_set_aside`
  moves the file to the first free name of `<name>.corrupt`,
  `<name>.corrupt.1`, … rather than replacing an earlier one.
- The `Backend` watches the runtime task; a panic becomes a lasting
  `BackendEvent` that the UI shows as a banner ("vehicle state stopped
  updating") rather than a silently frozen snapshot (SVC-6).
- `Rejection` and `MissionRejection` are built only from failure values: replace
  the `From` conversions that map `Accepted` to `Unknown` (SVC-7).
- Write down the service layer's result convention in `services/mod.rs` and
  `AGENTS.md` (API-3).

**Tests:** a writer that fails reports it through the status and `stop()`; the
tlog's timestamps are receive times; `stop()` right after a burst of frames
returns with all of them on disk; a panicking fold produces the backend
event; a `0600` file is replaced by a `0600` file; setting aside twice keeps
both copies.

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
**Status:** Todo

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

### WP35 · Decoding coverage
**Closes:** PROTO-10, CAL-14 · **Status:** Todo

- **Several unknown enums** (PROTO-10), `mavlink/decode.rs`: replace the
  single patch-and-retry with a loop over one working copy of the payload
  bytes. On `InvalidEnumValue { enum_name }`, look the field up in the
  message's `TolerantMessage` row and patch it in the buffer. Stop when the
  message decodes, when the named field has no fallback, or after
  `fields.len()` patches. `patched()` becomes "patch this buffer in place".
  Each patch keeps its debug log line.
- **PX4 sub-modes** (CAL-14), `firmware/px4.rs`: first check the values
  against PX4's `src/modules/commander/px4_custom_mode.h` (expected:
  `PX4_CUSTOM_SUB_MODE_POSCTL_POSCTL = 0`, `_ORBIT = 1`, `_SLOW = 2`).
  - Add reported-only modes "Orbit" (`ModeKind::Guided`: the vehicle flies
    itself to an operator's target) and "Position Slow"
    (`ModeKind::PositionHold`). They go in the union `decode_mode` searches,
    not in any class's offered table.
  - `decode_mode` keeps the sub byte for `MAIN_AUTO` and `MAIN_POSCTL`.
    For those two, an unknown sub-mode decodes to `None` (unknown mode),
    not to the main mode's name; for the other main modes the sub byte stays
    ignored.
  - Update the comment "Only AUTO distinguishes sub modes".

**Tests:** a heartbeat with `type` and `autopilot` both 200 decodes with both
fallbacks; POSCTL/1 decodes "Orbit" and POSCTL/0 "Position"; POSCTL/9 is
`None`; a change between Position and Orbit emits `ModeChanged`.

### WP36 · Bootstrap lifecycle and readiness
**Closes:** SVC-18, SVC-19, API-16 · **Status:** Todo · **Decision:** 36
(above)

- **Owned tasks** (SVC-18): the runtime keeps a `tokio::task::JoinSet<()>` (a
  local in `run`) for every task it starts: bootstraps, `describe_camera`,
  and WP32's calibration cancels. Aborting the runtime drops the set, which
  aborts them. Reap finished tasks with `join_next` in the `select!` loop
  (a branch guarded by `!set.is_empty()`), so the set does not grow.
- **One bootstrap per vehicle at a time, re-run when needed** (SVC-19): the
  runtime keeps `BTreeMap<SystemId, AbortHandle>` of running bootstraps. A
  bootstrap starts on `Registered`, on `Rebooted` (aborting a running one),
  and on `LivenessChanged { to: Active, .. }` from `Lost` when the vehicle's
  last bootstrap did not finish cleanly.
- **Readiness in the snapshot** (API-16): `Vehicle` gains `pub bootstrap:
  Bootstrap`, with `Bootstrap::{Pending, Running, Done { failed: Vec<Step> }}`
  and `Step::{Capabilities, Home, Parameters, Mission, Camera}`.
  - `VehicleClient::bootstrap` reports `Running` through a new
    `Input::Bootstrap`, notes each step that fails, and reports `Done` at
    the end. A drop guard reports `Done` with the remaining steps failed if
    it is aborted.
  - The fleet emits `FleetEvent::Bootstrapped(SystemId)` on `Done`. A
    frontend that needs the encoding waits for this, or for
    `component_state(c).param_encoding().is_some()`.
- `VehicleClient::bootstrap` becomes `pub(crate)`.

**Tests:**
- Dropping the `Backend` while a bootstrap waits on a silent mock: the
  mock's link sees no further requests.
- A vehicle that reboots (boot time steps back) is bootstrapped again:
  the mock receives a second `PARAM_REQUEST_LIST`.
- A failed mission download yields `Done { failed: [Mission] }`, and a
  return from `Lost` re-runs it.
- `Bootstrapped` is emitted once per run.

### WP37 · Clients that match their target
**Closes:** API-14, API-20 (flags) · **After:** 36 · **Status:** Todo

- Split `services::VehicleClient`:
  - `VehicleClient { fleet, system }` keeps the vehicle's operations: arm,
    modes, missions, calibration, cameras (they take a `CameraId`, which
    names its component).
  - A new `ComponentClient { fleet, address: MavlinkId }` gets
    `fetch_parameters` and `set_parameter`, moved from `VehicleClient`.
  - `FleetHandle::component(address) -> ComponentClient`, and
    `VehicleClient::parameters(&self) -> ComponentClient` for the vehicle's
    default parameter component, resolved from the snapshot when called.
  - `component_of` and the `component: Option<u8>` field go.
- Flags become operations: `arm()` / `disarm()`, `pause_mission()` /
  `resume_mission()`.
- Update the call sites in `groundhog-gui-iced` (`parameter_panel.rs`, the
  vehicles and mission features), and bootstrap
  (`self.parameters().fetch_parameters()`).
- Re-export `ComponentClient` from `services`, and update the
  `services/mod.rs` and `AGENTS.md` references to `VehicleClient` where
  they mean parameters.

**Tests:** existing parameter tests through `ComponentClient`; a
`ComponentClient` has no `arm` (a compile-level guarantee, nothing to test);
`disarm()` sends param1 = 0.

### WP38 · Observing the fleet
**Closes:** API-15, API-18, SVC-16 · **After:** 36 · **Status:** Todo

- **Many readers, bounded:**
  - The runtime publishes events on a `tokio::sync::broadcast` channel
    (capacity 1024) instead of the unbounded `mpsc`; snapshots stay on the
    `watch`. `FleetHandle::subscribe() -> FleetUpdates` makes a reader, and
    `BackendEvents::fleet` is simply the first.
  - `FleetUpdate` gains `lagged: u64`: events dropped for this reader since
    its last update (from `RecvError::Lagged`). A reader that lagged
    re-derives from the snapshot. Document it on `FleetUpdate`.
  - `LinkEvents` gets the same treatment from `LinkManager`.
- **Waiting:** `FleetHandle::wait_until<T>(&self, timeout, f: impl
  FnMut(&FleetSnapshot) -> Option<T>) -> Option<T>`; `wait_for` becomes a
  one-line use of it, or goes.
- **One event shape** (SVC-16, API-18):
  - Every `FleetEvent` variant has named fields: `ArmedChanged { system,
    from, to }`, `ModeChanged { system, from, to }` (the before values are
    already computed in `Fleet::observe`),
    `CalibrationFinished { system, kind, outcome }`, and so on.
  - Add `FleetEvent::system(&self) -> SystemId`.
  - Update `app.rs`'s `on_fleet_event` and the feature matches.
- **`#[non_exhaustive]`** (API-18) on `FleetEvent`, `services::Error`,
  `Rejection`, `MissionRejection`, `LinkStatus`, `DownloadOutcome`, and
  `calibration::Outcome`. Not on structs: a non-exhaustive struct cannot be
  built outside the crate even with `..Default::default()`, which would break
  `BackendOptions { links, .. }`.

**Tests:** two subscribers both receive every event; a subscriber that never
reads does not stop the other, and when it reads again it sees `lagged > 0`
and the current snapshot; `wait_until` returns the parameter outcome once the
download ends; `ModeChanged` carries `from`.

### WP39 · Safety rules in the backend
**Closes:** API-17 · **Status:** Todo

Mirror `MissionDraft::check` for vehicle operations, so every frontend asks
and refuses alike.

- `domain::vehicle::VehicleAction::{Arm, Disarm, SetMode(FlightMode),
  StartCalibration(CalibrationKind)}` and `Vehicle::check(&self, action) ->
  Verdict`, reusing `domain::mission::Verdict`. Move `Verdict`, `Caution` and
  `Refusal` to a shared module (`domain::check`) and re-export them from
  `mission` so existing paths keep working. The rules, moved from the GUI:
  - `Arm` always confirms, with no caution (`Verdict::Confirm(vec![])`, as
    `Start` does for missions).
  - `SetMode` confirms with `Caution::Armed` while `armed() != Some(false)`
    (`features/vehicles/mod.rs` `mode_change_asks`).
  - `StartCalibration` is refused while `armed() == Some(true)`
    (`Refusal::Armed`) or `liveness == Lost` (`Refusal::NotResponding`),
    and when `calibration_kinds()` lacks the kind
    (`features/calibration.rs:129-135`).
  - `Disarm` goes without asking.
- `VehicleClient::start_calibration` enforces its refusals: it returns a new
  `Error::Refused(Refusal)` before sending.
- `Vehicle::heading_rad() -> Option<f32>`: attitude yaw, else course over
  ground when ground speed exceeds 0.5 m/s, else `None`. Moved from
  `features/vehicles/map.rs:367`, which then calls it.
- The GUI's three call sites use `check`, keeping their wording.

**Tests:** in `domain/vehicle`: `SetMode` confirms when armed or unknown and
goes when disarmed; `StartCalibration` is refused when armed and when lost;
`heading_rad` falls back to the course and is `None` when slow. In
`services/tests.rs`, `start_calibration` on an armed vehicle sends nothing.

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

### WP40 · Public vocabulary
**Closes:** API-19, API-20 (rest), API-4 (R-1104 part) · **After:** 37, 38 ·
**Status:** Todo

Mechanical, but every item changes a public name. Do one commit per bullet,
each updating `groundhog-gui-iced` in the same commit.

- **Units in names** (API-20):
  - `BatteryState { voltage_v, current_a, remaining_pct }`.
  - `Home::altitude_msl_m`, `climb_rate_mps()`, `ground_speed_mps()`.
  - `GlobalPosition::alt_agl` → `alt_rel_m`, with the doc "above home".
- **Sentinels to `Option`** (API-20): `OnboardMission::opaque_id:
  Option<u32>`, with `None` where it is 0 today. The router keeps the raw
  value; map it at `codec::decode` and at `upload_mission`.
  `ParamDownload::expected() -> Option<u16>`.
- **Severity** (API-20): replace `status::rank` with `StatusText::is_at_least(
  MavSeverity) -> bool` (MAVLink severities count down: `Emergency` is 0).
  `app.rs:622` uses it.
- **Firmware behind the vehicle** (API-19): the GUI calls only
  `vehicle.firmware().name()`.
  - Add `Vehicle::firmware_name() -> &'static str`.
  - Make `Vehicle::firmware()`, the `Firmware` and `CalibrationProtocol`
    traits, and `Firmware::set_mode`/`decode_mode` `pub(crate)`. What a
    frontend needs is on `Vehicle` already (`modes`, `mode_of_kind`,
    `calibration_kinds`, `calibration_controls`).
  - `unreachable_pub` will name whatever else must follow.
- **Crate-private internals** (API-19): `DownloadUpdate`,
  `Vehicle::is_authoritative`, `target`, `target_for` become `pub(crate)`.
  `Vehicle::new` becomes `pub(crate)`, re-exported under `test-support` if
  the frontend's tests need it. `MissionProgress::current_seq` becomes
  `pub(crate)` and `current_index(layout)` goes; frontends use
  `Vehicle::current_mission_index`.
- **No nalgebra in the API** (API-19):
  - `Attitude` keeps its quaternion private and offers
    `roll_rad()`/`pitch_rad()`/`yaw_rad()`.
  - `velocity_ned` becomes a crate type
    `Ned { north_mps, east_mps, down_mps }`.
  - The GUI's `euler_angles()` calls go.
- **Re-export what the API needs** (API-19): `pub use mav_param::{Ident,
  Value};` in `domain::parameter`. The GUI imports them from there.
- **Typed errors** (API-4, R-1104 part):
  - `Error::NoSuchItem { index }` for `set_current_mission_item`'s
    out-of-range index.
  - `DownloadOutcome::Failed(services::Error)`. Move the `Clone` error type
    so `domain` can name it, or keep a small domain enum of reasons; pick
    whichever keeps `domain` free of `services`.

**Tests:** existing tests, renamed; `is_at_least` across the scale; an
upload's `opaque_id` of 0 reads `None`; attitude angles match
`euler_angles()` for a few quaternions.
