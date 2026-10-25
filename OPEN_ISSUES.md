# Open issues

A single living list for Groundhog, merged from `CODE_REVIEW.md`,
`LOGIC_REVIEW.md` and `REVIEW.md` and verified against `5389934` on
2026-10-25. Only open work is here. Fixed items are in
[Appendix A](#appendix-a--fixed-since-the-reviews); claims that were wrong or
too weak to act on are in [Appendix B](#appendix-b--rejected-claims) and
[Appendix C](#appendix-c--parked).

## How to use this list

- Work from the top. Priority is operator impact, not effort.
- An entry is done when the code, the test named in it, and any docs it
  contradicts are updated. **Delete the entry in the fixing commit**; do not
  leave a checked box.
- Keep the IDs stable when reordering. If a claim turns out wrong, move it to
  Appendix B with one sentence, rather than deleting it silently.
- Line numbers were current at `5389934` and will drift. Trust the symbol and
  the file; re-check the line when you edit nearby.

**Priority:** `P0` safety or a whole feature failing · `P1` common wrong result
or operator-visible failure · `P2` degraded behaviour or a correctness edge ·
`P3` maintainability, design or polish. **Effort:** `S` small, `M` a few
modules, `L` a design or multi-subsystem change.

## Start here

| | Issue |
|---|---|
| **P0** | [PROTO-1](#proto-1--tolerate-enum-values-newer-than-the-bundled-dialect) — a newer enum value can hide a vehicle or break a whole mission |
| **P1** | [PARAM-1](#param-1--parameter-writes-and-list-downloads-share-the-reply-channel) — a list value can satisfy a parameter set |
| **P1** | [PARAM-2](#param-2--a-component-with-no-parameters-reports-a-failed-download) — a zero-parameter component fails instead of showing 0 |
| **P1** | [CAL-1](#cal-1--px4-offers-flight-modes-the-vehicle-cannot-fly) — a rover is offered Takeoff and Precision Land |
| **P1** | [UI-1](#ui-1--the-confirmation-dialog-does-not-fit-a-phone) — the accept button can be off-screen |

---

## Protocol & middleware

### PROTO-1 · Tolerate enum values newer than the bundled dialect
**P0** · Effort M · From LOGIC-1

`mavlink/mission.rs:123` (`MissionListener::next`) drops anything
`DefaultDialect` cannot decode; `services/runtime.rs:224-241` special-cases
only `MAG_CAL_REPORT`. A `HEARTBEAT` with a newer `MavType`/`MavAutopilot`
never registers the vehicle; a `MISSION_ITEM_INT` with a newer command,
frame or mission type is re-requested until the transfer times out. A
`COMMAND_ACK` with a newer command is silently dropped (`router.rs:546`).

**Fix:** add tolerant decoders for `HEARTBEAT` and `MISSION_ITEM_INT` using
fixed offsets and a patched payload, as `decode_mag_cal_report` does, or
decode the raw fields and represent unknown enum values. At minimum, report
"the vehicle holds an item this build cannot decode" instead of timing out.

**Test:** encode a heartbeat with an unknown `MavType` and a mission item with
an unknown command; assert registration and download still succeed.

### PROTO-2 · A duplicate backlog longer than the window is accepted
**P2** · Effort M · From REVIEW-H2

The eight-drop override is gone, but the window holds 192 packets
(`dedup.rs:48,112-121`). A store-and-forward link that delivers more than that
gets its oldest copies accepted, evicts the real entries, and then the whole
tail passes; routes are refreshed on those frames.

**Fix:** treat a regressed sequence as a resync — drop it and the following
window, or accept exactly the resync frame and discard the rest. Update the
module doc, which currently says "however many arrive in a row".

**Test:** replay a backlog longer than `DUPLICATE_WINDOW` and assert the tail
is dropped.

### PROTO-3 · Mission transfers have no identity beyond target and type
**P2** · Effort M · From LOGIC-4

`MissionListener` filters by source, target and mission type only
(`mission.rs:88-105`). A late `MISSION_ACK` (or a `MISSION_COUNT` from a
previous download) can be consumed by the next transfer after a timeout,
failing it or reporting success for a transfer that never ran. The described
"cancel → ACK" trigger is wrong (see Appendix B), but the race is real.

**Fix:** after taking the lock, drain replies that predate the new request, or
correlate the first reply with the `MISSION_REQUEST_LIST`/`MISSION_COUNT`
just sent, or require a short quiet period.

**Test:** begin a transfer, time it out, let its ACK arrive, start a new
transfer and assert the old ACK is ignored.

### PROTO-4 · Liveness depends on a heartbeat that a full queue may skip
**P2** · Effort M · From REVIEW area 1

The heartbeat is the only outbound liveness write. A full control queue is
logged and skipped (`router.rs:743-749`); a wedged writer leaves the link
attached and `LinkGuard::closed()` silent, and `LinkStats` has no `last_tx`.

**Fix:** add a last-transmit watchdog (or send the heartbeat outside the
congested queue) so a wedged write is detected.

### PROTO-5 · `LinkGuard::closed()` fires before the link is removed
**P2** · Effort S · From REVIEW area 1

The guard's token cancels before the reader tail runs `remove_link`
(`router.rs:781-783` vs `:712`). Between the two, `LinkManager` reports the
link lost while `route()` can still `try_send` into a cancelled writer and
return success for a frame never sent.

**Fix:** remove first, then cancel, or have `closed()` await teardown.

### PROTO-6 · Cancellation messages go through the congested path and drop errors
**P3** · Effort S · From REVIEW area 1

`COMMAND_CANCEL` (`transaction.rs:269-277`) and `MISSION_ACK(cancelled)`
(`mission.rs:192-205`) use `route(..)` and discard the result, although the
module docs promise the vehicle is told. There is also no test that dropping a
command future sends `COMMAND_CANCEL` (the mission download equivalent is
tested).

## Mission

### MISSION-2 · The editor and selection stay live during a transfer
**P2** · Effort S · From LOGIC-13 / REVIEW area 10

`edit()` refuses every edit while `transferring` (`features/mission/mod.rs:329-333`),
but the row click (`view.rs:272-275`), `Deselect` (`:362`) and every editor
field remain enabled and their messages are silently discarded.

**Fix:** disable selection/Deselect and the editor while transferring, or let
selection through without treating it as an edit.

**Test:** a transfer in progress; a row click and Deselect produce no state
change and no lost message.

### MISSION-3 · Loading a file replaces a modified draft without asking
**P2** · Effort S · From REVIEW area 10

`MessageKind::Loaded` calls `set_items` unconditionally
(`features/mission/mod.rs:301-311`); it does not check `transferring` or
whether the draft has edits.

**Fix:** refuse or confirm when the draft is modified or a transfer runs.

### MISSION-4 · The "changed elsewhere" banner contradicts the upload path
**P3** · Effort S · From LOGIC-13

`view.rs:160-165` says "Download it before uploading", while upload over
`VehicleChanged` now asks and then proceeds (`draft.rs:217`).

**Fix:** reword the banner to match the confirmation, or make the banner hide
once the operator has been warned.

### MISSION-5 · `nan`/`inf` are accepted for required fields
**P2** · Effort S · From REVIEW area 5

`set_field_from_str` accepts `"nan"` for any field
(`domain/mission/item.rs:354`), and `inf` parses as a number. A NaN latitude
becomes `x = 0` (`degrees_e7` saturates the cast) and an infinite altitude is
uploaded.

**Fix:** only accept `nan` for optional fields; reject non-finite values for
required coordinates and for altitude.

**Test:** setting latitude to `nan` and altitude to `inf` is refused.

### MISSION-6 · Saving WPL fabricates a home, and loading drops a real one
**P2** · Effort S/M · From REVIEW area 5

`write_wpl` calls `codec::encode`, which inserts `(0,0,0)` when no home is
given (`storage/mission.rs:63`, `codec.rs:27-35`). `parse_wpl` now returns
`WplMission.home`, but the GUI's `Loaded` uses only `mission.items`
(`features/mission/mod.rs:301-311`), so load → save loses a real home and
invents a fake one.

**Fix:** keep the loaded home in the draft (or pass it back on save) and refuse
to write a home row when none is known, or write a file without one.

### MISSION-7 · A WPL file without a home row loses its first item
**P3** · Effort S · From REVIEW area 5

`parse_wpl` always decodes under `WPL_LAYOUT` (home slot) and takes seq 0 as
home (`storage/mission.rs:121`, `codec.rs:55-75`); the `index`/`current`
columns are parsed and discarded (`:99-107,165-167`).

**Fix:** detect whether row 0 is home from the current/index columns or the
header convention, and keep it as an item when it is not.

### MISSION-8 · `MissionDraft::follow` records `seen` before declining
**P3** · Effort S · From LOGIC-14.1 / CODE-F11.4

`draft.rs:271-272` stores the onboard mission, then refuses to adopt it when
the draft has edits. After the operator reverts, the same onboard mission is
not adopted until it changes again, so the draft reports `Modified` with no
local edits.

**Fix:** either skip the `seen` update when declining, or document why the
one-shot behaviour is intended and test it.

### MISSION-9 · `fields()` ignores the frame that `position()` honours
**P2** · Effort S/M · From REVIEW area 5

`fields()` presents lat/lon in degrees for every positional command
(`item.rs:262-284`), while `position()` returns `None` for local frames
(`:192-211`). A local-frame item is edited as degrees and the map shows
nothing.

**Fix:** drive the editor fields from the same frame check, or carry the frame
explicitly in `FieldView`.

### MISSION-10 · Jump targets can outlive their item
**P3** · Effort S · From REVIEW area 5

`Plan::remove` maps `target > index` down but leaves `target == index`
(`plan.rs:141-154`); removing the last item can leave a `DO_JUMP` pointing at
`len`. `codec::shift_targets` clamps a below-home target to 0 rather than
skipping it (`codec.rs:81-88`).

**Fix:** map a removed target to its successor (or clear it when there is
none); skip invalid targets instead of clamping.

### MISSION-11 · WPL BOM and out-of-range coordinates
**P3** · Effort S · From REVIEW area 5

A UTF-8 BOM breaks the header (`storage/mission.rs:99-107`), and out-of-range
lat/lon saturate to `i32::MAX` and import as a plausible waypoint
(`wire/mod.rs:23`).

**Fix:** strip the BOM; reject coordinates outside ±90/±180.

## Parameters

### PARAM-1 · Parameter writes and list downloads share the reply channel
**P1** · Effort M · From REVIEW-H1 / LOGIC-5

`LockKey::Param(target, ident)` and `LockKey::ParamList(target)` are distinct
(`locks.rs:28,30`), but both consume `PARAM_VALUE` from the same target.
`param_set` now prefers a value equal to the request (`param.rs:90-105`), which
removes the common false rejection, but a missed/late echo still returns an
old list value. The panel also allows Upload during a download
(`parameter_panel.rs:472`).

**Fix:** one lock per reply channel (`LockKey::ParamValue(target)`), or hold
`ParamList` for the duration of a set. Add the UI guard as defence in depth.

**Test:** a list value for the same ident arrives before the echo; the set does
not return it.

### PARAM-2 · A component with no parameters reports a failed download
**P1** · Effort S · From LOGIC-12

`expected` is only set from a received `PARAM_VALUE`; a component that sends
none leaves it `None`, so `param.rs:158,203` returns `Timeout` after the
retries and the panel stores `Failed` (`services/vehicle.rs:246-254`).

**Fix:** treat "list requested, idle timeout, no values" as an empty complete
list `(0, 0)`.

**Test:** a mock that answers `PARAM_REQUEST_LIST` with nothing.

### PARAM-3 · "Save to a file" writes display values while encoding is unknown
**P2** · Effort S · From REVIEW area 6

`Parameters::values()` exports decoded values (`parameter.rs:92-98`), and
`SaveDialog` uses it even when `encoding_known` is false
(`parameter_panel.rs:246-265`). Uploads are gated; saves are not, so a
bytewise-decoded integer can be exported as if it were real.

**Fix:** disable saving until the encoding is known, or export raw values with
a warning.

### PARAM-4 · QGC `.params` files ignore their system/component columns
**P2** · Effort S/M · From LOGIC-11

`storage/parameter.rs:117-120` discards the first two fields, so a file from
another vehicle or component loads silently into whichever component is open.

**Fix:** compare the file's address with the target and warn or filter
mismatches; at least report "N values were for another component".

### PARAM-5 · The three parameter readers disagree
**P2** · Effort M · From REVIEW area 6

`.param`/`.params` abort on one bad line (including an unknown 64-bit type),
`.ini` logs and skips; unknown names are dropped before
`ParamEdits::load` can count them; names are truncated to 16 bytes silently;
duplicates are last-wins (`storage/parameter.rs:104-131,250-276`).

**Fix:** define one policy (skip with a counted warning is friendlier), share
it, and report truncation/duplicates in the load summary.

### PARAM-6 · A new download does not clear parameters the vehicle removed
**P2** · Effort S · From REVIEW area 6

`Parameters::loading` clears `has_loaded` but not `map`
(`parameter.rs:154-158`), so removed parameters survive every reload and every
exported file.

**Fix:** clear the map when a download starts, or replace it on completion.

### PARAM-7 · A refused value leaves the download badge stuck
**P2** · Effort S · From REVIEW area 6

A value `observe` refuses (unsupported type) is dropped before `loading.note`
is updated (`parameter.rs:110-134`), but the transaction still counts its
index. `DownloadState` can say `Complete` while `LoadingState` never completes.

**Fix:** note the refused index in `LoadingState` too, or count only accepted
values consistently on both sides.

### PARAM-8 · "Save to a file" omits pending edits without saying so
**P3** · Effort S · From REVIEW area 11

The header can say "3 changed" while the saved file contains only reported
values (`parameter_panel.rs:246-264`). Either say the edits are not included
or save the edits too.

### PARAM-9 · Large `ParamValues` messages derive `Debug`
**P3** · Effort S · From CODE Appendix D

`SaveTo`/`Loaded` carry a whole `ParamValues` map with a derived `Debug`
(`parameter_panel.rs:80-105`), which iced pretty-prints on the UI thread when
the `debug` feature is on. Use a summary `Debug` as `FleetUpdate` does.

### PARAM-10 · Serial-port enumeration is outside `Operations`
**P3** · Effort S · From CODE-F4.1

`link_panel/mod.rs:112-119` spawns `detect_serial_ports` with no busy flag, so
repeated actions can race. Put it behind `Operations` or a small busy guard.

## Links & transports

### LINK-1 · Renaming a link leaves a duplicate behind
**P2** · Effort M · From LOGIC-8

`LinkManager::save` finds an existing link by display name and otherwise mints
a new id (`link.rs:309-318`). The editor carries no `LinkId`, so editing "A"
to "B" creates "B" and leaves "A". If "A" auto-connects, the next start opens
both to the same endpoint.

**Fix:** keep the edited `LinkId` in `LinkConfigurator` and update by id; use
name only for a new link. Add a test for rename.

### LINK-2 · A listening UDP link admits any datagram as a peer
**P2** · Effort S · From REVIEW-H3

The peer table is capped at 16 (`udp.rs:32,80-89`), but `note_peer` admits any
source (`:69-97`) and `poll_write` sends every outbound frame to every live
peer (`:220-228`). One spoofed datagram gets 10 s of outbound traffic; 16
evict every real peer.

**Fix:** admit a peer only after it has produced a plausible MAVLink frame;
keep the cap and eviction.

### LINK-3 · C-cast decoding saturates silently
**P2** · Effort S · From REVIEW area 3

`value_from_c_cast` turns NaN into 0, `-1.0` into 0 for a `Uint8`, and `300.0`
into 255 (`wire/param.rs:83-96`). A sentinel can be presented as a real
editable value.

**Fix:** return `None` for non-finite/out-of-range values, or document the
saturation as intentional.

### LINK-4 · A serial open cannot be cancelled
**P3** · Effort M · From REVIEW area 3

`link/transport.rs:70-83` runs the open in `spawn_blocking`; `OPEN_TIMEOUT`
stops waiting but the thread lives on and retries can contend for the device.

### LINK-5 · 64-bit parameter types decode to `None` without saying so
**P3** · Effort S · From REVIEW area 3

`wire/param.rs:43,63,92` supports neither `Uint64` nor `Int64`; name them in
the module doc (or implement them).

### LINK-6 · Listen-mode UDP splices a truncated datagram onto the next
**P3** · Effort S · From REVIEW area 3

`link/udp.rs:134-139,169-174` uses a byte buffer across datagram boundaries,
so one bogus frame precedes resync. Document the behaviour or parse per
datagram.

### LINK-7 · Saving a live link can restart it without asking
**P3** · Effort S · From CODE-F4.3

`LinkManager::save` reconnects when the transport changes
(`link.rs:331-336`); the panel warns by wording/colour but does not confirm.
Ask when a live link will be disconnected.

## Firmware & calibration

### CAL-1 · PX4 offers flight modes the vehicle cannot fly
**P1** · Effort S/M · From REVIEW area 7

`px4.rs:46-62` has one table for every class, and `modes()` returns it for any
`is_vehicle()` (`:74-76`). A rover, boat or submarine is offered Takeoff,
Land, Mission, Precision Land and VTOL Takeoff.

**Fix:** filter the table by `VehicleClass` (or add per-class tables); test
`modes()` and `mode_of_kind()` per class.

### CAL-2 · There is no "confirmation sent" state
**P2** · Effort S · From REVIEW area 7

After the operator confirms an ArduPilot orientation, the session still shows
`Instruction::Place(side)` and an enabled "In position" button
(`features/calibration.rs:220-230`). A second press re-sends the position,
which ArduPilot refuses once measuring has begun, producing a false failure.

**Fix:** record that the confirmation was sent for this side and show an
"Awaiting vehicle…" instruction until progress arrives; disable the button.

### CAL-3 · A calibration session has no timeout or reboot check
**P2** · Effort M · From REVIEW area 7

Only `Liveness::Lost` fails a session (`fleet.rs:277-283`). An autopilot reboot
mid-run leaves the session `Running` forever, with Start disabled and Dismiss
refused.

**Fix:** time out a session that makes no progress, and fail it on a boot-id
change or a new heartbeat naming a different boot.

### CAL-4 · The "cannot cancel" caption is wrong for `completes_on_ack` kinds
**P2** · Effort S · From REVIEW area 7

`calibration/mod.rs:265-271` documents `cancel == false` as "the vehicle keeps
waiting until it reboots", which is not true for kinds that complete inside the
start command. The panel shows that caption while a real run progresses
(`features/calibration.rs:265-276`).

**Fix:** word the caption per kind, and only while the start is still pending.

### CAL-5 · PX4 per-side compass progress is logged, not parsed
**P2** · Effort S · From REVIEW area 7

PX4 sends `"[cal] … side calibration: progress <n>"`; the parser matches only
`"progress <"` (`calibration/px4.rs:70-79,139-143`), so the text becomes a note
and floods the six-line log, evicting actionable warnings.

**Fix:** parse the per-side form (or filter it from the note log).

### CAL-6 · PX4 protocol version and `pending:` handling
**P3** · Effort S · From REVIEW area 7

The protocol version is ignored, and a `pending:` side whose name cannot be
matched is treated as done (`px4.rs:83,96-103`), so a renamed orientation can
be recorded as measured.

### CAL-7 · Cancel retry policy is firmware-blind
**P3** · Effort S · From CODE-F3.1

`services/vehicle.rs:549-560` always retries the cancel once because PX4
acknowledges it twice. Put the policy on `CalibrationProtocol` so another
firmware can answer differently.

### CAL-9 · Missing calibration tests
**P3** · Effort S · From REVIEW area 7 / CODE-F10

No test covers the compass `pending:` → next-side transition, PX4 per-side
progress text, or the whole calibration panel flow (there are no panel tests
for `features/calibration.rs`).

## Services & vehicle state

### SVC-1 · A partial parameter download has two verdicts
**P2** · Effort S · From REVIEW area 8

A partial list stores `Failed("received 3 of 500")` in the snapshot while the
call returns `Ok(ParamListSummary)` (`services/vehicle.rs:216-257`), so the
panel can show both a danger banner and an incomplete warning.

**Fix:** return the failure to the caller too, or make the snapshot the single
source of the outcome.

### SVC-2 · Recording is unbounded and can stop silently
**P2** · Effort M · From REVIEW area 8

`recording.rs:38,52-79` uses an unbounded filter channel and returns on a
write error while `TlogRecorder` still looks alive and `stop()` returns `()`.
A full disk ends the recording with only a log line.

**Fix:** bound the channel (drop oldest or report overflow) and surface a
failed stop to the caller/notice.

### SVC-3 · Vehicle state never ages
**P2** · Effort M · From REVIEW area 4

`Fleet::tick` only ages liveness (`fleet.rs:255-288`). A lost vehicle keeps its
last `position.current`, velocity, GPS health and `home_position`; a zeroed
`HOME_POSITION` is ignored rather than clearing the old home.

**Fix:** age position/velocity and home in `tick`, or mark them stale alongside
liveness.

### SVC-4 · Health combines `enabled` and `healthy` inconsistently
**P2** · Effort S · From REVIEW area 4

`health.rs:143-152` ORs `enabled` over the present flags but ANDs `healthy`, so
a healthy primary plus a present-but-disabled backup with a clear health bit
renders red; `failing()` inherits it.

**Fix:** decide whether a sensor counts when disabled, and apply the same rule
to both flags. Test the mixed case.

### SVC-5 · Tlog timestamps are sampled at write time
**P3** · Effort S · From REVIEW area 8

`recording.rs:60-63` uses `SystemTime::now()` in the writer, not the receive
time; a lagging writer skews the timeline. `stop()` also does not await the
final flush (the test sleeps to hide it).

### SVC-6 · A fold panic kills the only state-advancing task
**P3** · Effort S · From REVIEW area 8

`services/runtime.rs:151-210` has no supervision; a panic drops the task and
the UI renders the last snapshot forever. Catch and report, or document the
non-panic policy more loudly.

### SVC-7 · `From<MavResult>` maps accepted replies to `Unknown`
**P3** · Effort S · From REVIEW area 8

`services/error.rs:75,113` maps `Accepted`/`InProgress` to
`Rejection::Unknown` and `MavMissionResult::Accepted` to `Error`. Unreachable
today; a trap for the next caller.

### SVC-8 · The first `LinkEvent` repeats the status
**P3** · Effort S · From REVIEW area 8

The watch is seeded with `Connecting { attempt: 1 }` and the supervisor
publishes the same status (`services/link.rs:231,391`), so the first event has
`previous == status`.

### SVC-9 · `STATUSTEXT` copies the whole shared log per chunk
**P3** · Effort M · From REVIEW area 4

`pending` lives inside the shared `StatusLog` (`status.rs:53-58`), so every
chunk deep-copies up to 1000 texts with their `String`s through
`Arc::make_mut` (`vehicle/mod.rs:151,585`). Keep `pending` outside the shared
value.

### SVC-10 · The trail is copied on every fix
**P3** · Effort M · From REVIEW area 4

`PositionTrack` sits behind an `Arc` but is mutated on every fix
(`vehicle/mod.rs:561-573`), copying up to 1000 positions per publish. Make the
trail a ring buffer with interior ownership or accept the cost explicitly.

### SVC-11 · Every `PARAM_VALUE` copies the parameter map
**P3** · Effort S · From REVIEW area 4

`on_param_value` always takes `component_mut` (`vehicle/mod.rs:456`), even for a
repeat of an unchanged value; `component_mut_if` exists for this. Use it when
the value and type are unchanged.

### SVC-12 · Invalid UTF-8 drops a whole name
**P3** · Effort S · From REVIEW area 4

`string_from_cstring` (`vehicle/mod.rs:606-616`) rejects the whole name, while
statustexts decode lossily. One stray byte loses vendor/model.

### SVC-13 · The public `Fleet` folding API is a test seam
**P2** · Effort S/M · From CODE-F2 / REVIEW-H17

`Fleet::observe` (`fleet.rs:168`) and `set_onboard_mission` (`:292`) are
`pub`; their only out-of-crate callers are feature tests and
`app/gallery.rs`. The architecture says only the runtime advances state.

**Fix:** add a `test-support` feature (or a fixture constructor) and make the
production folds `pub(crate)`. Add a note to `AGENTS.md`.

### SVC-14 · Calibration loss bypasses `update_calibration`
**P3** · Effort S · From CODE-F11.2

`fleet.rs:277-283` mutates `vehicle.calibration` directly on loss while every
other update goes through `update_calibration`. Move it, or document why
liveness is special.

### SVC-15 · The feature contract and persistence paths are split
**P3** · Effort M · From CODE-F1

Four update shapes remain (`Context`, `LinkManager` + `Event`, video `Event`,
no context), and persistence takes three routes (`Effect::RememberDirectory`,
link/video events, direct settings save). Either document the split in
`features/mod.rs` and `AGENTS.md`, or add `links` and `Effect::Persist` to
`Context`. The split is defensible; the silence is the problem.

### SVC-16 · `FleetEvent` does not carry `from`/`to`
**P3** · Effort M · From CODE-F5

Only `LivenessChanged` carries both values; `ArmedChanged`/`ModeChanged` carry
the new one, and events are produced both by folds and by before/after diffing
(`fleet.rs:219-243`). Either carry `from` consistently and return events from
the fold, or change the `AGENTS.md` rule.

### SVC-17 · No written error policy
**P3** · Effort S/M · From CODE-F6

Structured errors, `String` and `anyhow` coexist; `storage/parameter.rs` uses
`Result<_, String>` while `storage/mission.rs` has `WplError`. Write the policy
in `AGENTS.md`, and give parameter files a typed line error. In features,
import the services result as `ServiceResult` to avoid
`std::result::Result` noise.

## UI & shells

### UI-1 · The confirmation dialog does not fit a phone
**P1** · Effort S · From REVIEW area 11

`features/confirm.rs:103` fixes the dialog at 420 px; a phone sheet is about
400, so the accept button can sit off-screen.

**Fix:** use a max width relative to the window (or `ui::space`/`Length::Fill`
with sensible bounds). Test at 400 px wide.

### UI-2 · The tiled rail cannot scroll
**P2** · Effort S · From REVIEW area 9

`app/tiled.rs:98-102` is a plain `column` of every panel button (~48 px each);
a short window makes the tail (Log, Video, Settings) unreachable.

**Fix:** make the rail scrollable or fold the overflow into a menu.

### UI-3 · An expanded floating sheet reports the map as uncovered
**P2** · Effort S · From REVIEW area 9

`SheetPlacement::of` returns `None` when expanded (`shell.rs:318-321`), so
`covered` misses the sheet and follow/centre mode centres the vehicle where
nothing is visible.

**Fix:** report the expanded sheet's area, or disable follow/centre while it is
expanded.

### UI-4 · Loaded settings are not sanitised
**P2** · Effort S · From REVIEW area 9

`scale_factor: 0` blanks the window, and the waypoint altitude is unchecked
(`config.rs:41-57`, `app.rs:273-275`); `Workspace::sanitized` is the only
sanitiser.

**Fix:** clamp scale factor and altitude (and any future numeric setting) on
load.

### UI-5 · Density-sensitive sizes are fixed pixels
**P2** · Effort M · From CODE-F9.2

`HEADER_HEIGHT = 28` and `INPUT_WIDTH = 96` (`parameter_panel.rs:540,685`), the
mode picker and Arm button (`vehicles/mod.rs:274,290`), and the dialog width do
not grow with `ui::Density`. Some other fixed widths (columns) are legitimate.

**Fix:** derive control/row sizes from `size::control()`/`size::list_row()`;
add width tokens for repeated control widths.

### UI-6 · The unlabelled checkbox is a 22 px target at touch density
**P2** · Effort S · From REVIEW area 13

`ui/input.rs:64-66` uses `size::checkbox()` without a `control()`-sized hit
area. Wrap it in a 44 px container on touch.

### UI-7 · `drag_source` ignores the finger id
**P2** · Effort S · From REVIEW area 13

`widget/drag_source.rs:153-166,222-250` treats any `FingerPressed` as a new
press and any lift as the end. A second finger abandons a drag without
publishing `on_end`, leaving the pane grid's highlight stuck.

**Fix:** follow one finger by id; ignore presses from another finger until the
tracked one lifts or is lost; publish `on_end` on all terminal paths.

### UI-8 · Pane-grid touch drags use the mouse cursor
**P2** · Effort M · From REVIEW area 14

`widget/pane_grid.rs:761-767,827-835` handles `FingerPressed`/`FingerLifted`
but reads `mouse::Cursor` for positions, so divider and title-bar drags follow
a stale mouse cursor on a tablet.

**Fix:** carry the touch position through the gesture state, as the map and
`drag_source` do.

### UI-9 · Menu placement can choose a two-pixel gap
**P2** · Effort S · From REVIEW area 13

`widget/menu.rs:186-201` chooses "below" when `below >= size.height ||
below >= above`, so a trigger near the bottom can get a clipped menu.

**Fix:** choose the side with room, or clamp the menu's height/scroll.

### UI-10 · Toasts and full-screen video ignore system insets
**P3** · Effort S · From REVIEW area 9

The tiled panes are inset, but toasts and the maximized-pane fallback use only
`space::XL` (`app.rs:756-762`, `app/tiled.rs:29-44`), so they can sit under
system bars.

### UI-11 · The log panel rebuilds the whole timeline per view
**P3** · Effort M · From REVIEW area 11

`log_panel.rs:150-157,226-262` collects, filters, sorts and truncates ~8000
texts + 500 notices every view; the header shows the post-truncation count.
Keep a maintained, bounded view or cache by revision.

### UI-12 · A lost slider release leaves the scale preview applied
**P3** · Effort S · From REVIEW area 11

`settings_panel.rs:72-77` commits only on release; `app.rs:665-670` has no
unfocus/cursor-left path. Cancel the preview on unfocus like the map does.

### UI-13 · Small toolkit drifts
**P3** · Effort S · From CODE-F9.1 / REVIEW area 13

The confirmation dialog, backdrop, toast and marked input are still built in
features (`confirm.rs:86-114`, `notifications.rs:198-205`,
`parameter_panel.rs:649`); move them into `ui` as `dialog`, `toast` and
`text_input_marked`. The "More" trigger is permanently `Active`
(`ui/tool.rs:130`), icons do not scale with density (`ui/style.rs:86,95`), and
the attitude docs say ±30° while `MAX_ATTITUDE_DEG` is 40
(`widget/attitude.rs:4,16,27`).

## Map & widgets

### MAP-1 · The cache key omits the visible rectangle
**P2** · Effort S · From REVIEW area 12

`MapView::key()` has five fields and omits `visible` (`map/view.rs:90-100`),
which derives from `covered`. A cached layer keeps the old visible rectangle
when a sheet opens/closes at constant map size. Latent because the only
`visible()` user reports no revision.

**Fix:** include `visible` in the key.

### MAP-2 · `handles()` is rebuilt on every pointer move
**P2** · Effort M · From REVIEW area 12

`overlay.rs:105-128,451-461` rebuilds and re-allocates every layer's handles
(and again for `mouse_interaction`); a 200-item mission projects the whole
route per event. Cache handles per revision/view, or make them cheap.

### MAP-3 · Layer caches key on `LayerId` alone
**P2** · Effort S · From REVIEW area 12

`overlay.rs:260,274-291` has no uniqueness check, so two layers sharing an id
silently share/clear each other's cache. Assert uniqueness in debug, or key on
(id, index).

### MAP-4 · Hover is not refreshed when the view moves
**P3** · Effort S · From REVIEW area 12

`overlay.rs:432-461` refreshes hover only on `CursorMoved`; after a wheel zoom,
momentum pan or `center_on`, a handle under the old cursor stays highlighted.
Recompute after a view change.

### MAP-5 · Screen-anchored drag origins are lossy
**P3** · Effort S · From REVIEW area 12

`Anchor::Screen` handles can be draggable, but `Target::origin` and the cancel
payload are `GeoPosition` (`overlay.rs:117-123,151-159,533-539`), so a
screen-anchored drag cannot restore its origin exactly.

### MAP-6 · `visible()` clamps size but not origin
**P3** · Effort S · From REVIEW area 12

`map/view.rs:186-193` clamps width/height but not x/y; an over-large `covered`
can put the visible rectangle outside the map, clipping the HUD.

### MAP-7 · Pane-grid `place` removes before resolving a neighbour
**P3** · Effort S · From REVIEW area 14

`Layout::place` removes the panel before finding its neighbour
(`layout.rs:522-531`), returning `false` with the panel already gone. A stale
`Maximize(unknown pane)` is effectively unreachable after the recent fixes;
fix `place` if touching it and leave the rest.

## Video

### VIDEO-1 · RTSP and the retry machine are untested
**P2** · Effort M · From REVIEW area 15 / C1

`receiver.rs:432-559` covers `test://` and RTP only; there is no RTSP test, and
plugin-dependent tests print "skipped" and pass. The C1 "wrong index" claim is
wrong (see Appendix B), but the missing RTSP coverage is real.

**Fix:** add a `test://`-style RTSP test that exercises `select-stream` with
`[object, num, caps]`, and a state-machine test for `Retry`/backoff.

### VIDEO-2 · `Drop` only sets a flag, so pipelines can overlap
**P2** · Effort M · From REVIEW area 15

`receiver.rs:85-91` asks the supervisor to stop; `stop` is polled every 100 ms
(`:150-164`), so a replacement receiver (including every low-latency toggle)
can briefly coexist. UDP `reuse` lets both bind; an RTSP camera sees two
sessions.

**Fix:** join the old supervisor (or hand the new receiver the old one's
teardown) before starting the replacement.

### VIDEO-3 · Every bus error becomes an endless retry
**P2** · Effort M · From REVIEW area 15

`receiver.rs:327-341` maps every `Error`/`EOS` to `Retry`; `Status::Failed` is
reachable only for an unparseable pipeline. A 401/404/wrong-port/no-decoder
stream retries forever at the 10 s cap.

**Fix:** classify terminal failures (HTTP status, missing decoder) as
`Failed`, and keep retrying only recoverable ones.

### VIDEO-4 · `streamed` means "one frame ever"
**P2** · Effort S · From REVIEW area 15

`receiver.rs:131-143,305-320` resets the backoff to 1 s after any frame ever,
so a flapping camera never backs off.

**Fix:** track recent frames (a window), not a lifetime boolean.

### VIDEO-5 · Smaller video cleanups
**P3** · Effort S · From REVIEW area 15

`h264parse config-interval=-1` is set only on the RTP path
(`source.rs:184-188`); every non-BT.601 matrix becomes BT.709 including
`Unknown` (`receiver.rs:411-416`); a failed supervisor spawn leaves
`Connecting` forever (`:46-65`); frame generation is assigned outside the lock
and `clear()` does not advance it (`frame.rs:118-143`); per-frame caps parsing
and two mutexes plus a fresh `Vec` per plane add cost (`receiver.rs:386-421`).

## API, architecture & documentation

### API-1 · Central public types are undocumented
**P3** · Effort M · From CODE-F8.5

`Vehicle`, `Identity`, `Battery`, `AttitudeState`, `PositionTrack`,
`StatusLog`, `GlobalPosition` and many accessors/fields have no `///`. Add docs
to the central types and DTO fields. Enabling `missing_docs` crate-wide is a
larger mechanical change; decide whether it is worth it rather than leaving
the rule unenforced.

### API-2 · `LinkManager::new` exposes a private router type
**P3** · Effort S · From REVIEW-H17

`services/link.rs:183` is `pub fn new(&MavlinkRouter, ...)`, but `MavlinkRouter`
lives in a private module and no frontend can name it. Make it `pub(crate)` or
add a frontend-safe constructor; enable `private_interfaces` to catch the next
one.

### API-3 · The service layer has no single result convention
**P3** · Effort S · From REVIEW cross-cutting

`services::Result` does not cover recording (`io::Result`), `wait_for`,
`connect`, `remove` (`bool`) or `bootstrap` (`()`). Either accept it and say
so, or give the service layer one failure type.

### API-4 · Errors become `String` at boundaries
**P3** · Effort M · From REVIEW cross-cutting

`LinkStatus::{Reconnecting, Failed}`, `detect_serial_ports`,
`video::Status`, `Source::parse` and the config writer use `String`;
`CalibrationUpdate::Acknowledged(Result<(), String>)` and
`EditStatus::Failed(String)` lose the reason. Keep `String` only for
display-boundary passthrough and say so in `AGENTS.md`.

### API-5 · Progress has three shapes
**P3** · Effort M · From CODE-F4.1

`TransferProgress { done, total }`, `ParamListSummary { received, expected }`
and `Result<(usize, u16)>` describe the same thing. Introduce one `Progress`
type (or alias) in services.

### API-6 · Result variants name the same lifecycle differently
**P3** · Effort S · From CODE-F4.2

`Finished` (mission/vehicles/calibration) vs `Reloaded`/`Uploaded`
(parameters); mission uses `.map(|()| None)` to fit commands and transfers into
one variant. Standardise on `Finished(Key, Ticket, Result<Output>)` per
feature.

### API-7 · Identity types are half-typed
**P3** · Effort M · From REVIEW cross-cutting

`SystemId(pub u8)` and `MavlinkId { pub system, pub component }` are public and
`Default` (0/0 broadcast); the GUI hand-converts with `as` and truncates
`u64`→`u32` in map helper ids. Consider private fields and conversion methods.

### API-8 · Docs that still contradict the code
**P3** · Effort S · From CODE-F8.3

`vehicle/mod.rs:8` says drafts and parameter edits "live with the features that
edit them"; `parameter.rs:7-8` says the operator model lives in the frontend.
Both are in `groundhog-logic`. Fix the sentences.

### API-9 · The vendored pane-grid lacks module docs
**P3** · Effort S · From REVIEW cross-cutting

Ten of the eleven vendored files start with `use`/items, not `//!`; one has a
`//` header. Add a one-line `//!` to each, or record the exemption in
`AGENTS.md`.

### API-10 · Test harnesses are triplicated
**P3** · Effort S · From CODE-F10

`mission/tests.rs:25`, `parameter_panel/tests.rs:30`, `vehicles/tests.rs:23`
each define `Env { fleet, handle, notices, settings }` and `step`. Extract a
`#[cfg(test)] features/test_support.rs` when adding the next feature test.

### API-11 · Missing test coverage for command features
**P3** · Effort M · From CODE-F10

No panel test covers arm/disarm confirmation, calibration start/finish, a
finished-failure notice in mission, or save/load/Cancel/Progress. Add one
Context/Effect test per command feature.

### API-12 · Gallery is the only render check and is not committed
**P3** · Effort M · From REVIEW cross-cutting

`app/gallery.rs:268-285` is `#[ignore]`, deletes old images and compares no
committed snapshots; `XDG_*` is checked for presence, not for being throwaway.
Either commit reference images or state that the gallery is a manual tool.

### API-13 · Config schema tolerance still has one hard edge
**P3** · Effort S · From REVIEW area 14

`size`, `panes` and the `(Panel, f32)` tuples are now defaulted, but the tuple
shape is still fixed. If the layout schema must evolve, a custom deserializer
is the next step.

---

## Appendix A — Fixed since the reviews

Verified at `5389934`; do not re-open without new evidence.

- **CODE_REVIEW:** F8.1 (module docs), F8.2 (`ui::either`), F8.4 (dead code),
  F11.1 (copy-on-write before change), F11.5 (battery doc); F3.2's regression
  (`Invalid` heartbeat no longer resets firmware, with a test); F4.1's
  parameter Cancel; F4.3's delete-link confirmation; F5's
  `CalibrationFinished` payload; F9.1's mission checkbox.
- **LOGIC_REVIEW:** 2 (GCS heartbeat), 3 (mode picker confirmation), 5
  (parameter echo preference), 9 (`MISSION_CURRENT.total`), 14.3 (recording
  names), 14.4 (`reached_seq`).
- **REVIEW.md:** H4–H16 (parameter idle loop, persist/set-aside/fsync, class
  calibration offering, runtime stop, calibration drop guard, empty-upload
  confirmation, Start gate, 0,0 waypoint placement, parameter-load/upload
  identity, back behind confirmation, map drag cancel, pane minimums, adaptive
  menu); H17's `unreachable_pub` and `ParamEncoding` (the test seam remains,
  SVC-13); the parameter-specific part of H18 (the `COMMAND_CANCEL` test
  remains, PROTO-6).
- **Medium fixes:** persist temp names/permissions, link schema defaults and
  renames, `wire::latlon`, battery summary merge, zero quaternion, mission
  `with_command`/`next_altitude`, compass `id+1` overflow, same-second
  recording names, settings save-on-close, gallery data dir, config
  version/migration, link save preserving a connection, parameter text input,
  changed badge, link-name validation, map Escape, tiled/full-screen insets
  (partly), Android CI job, docs fork correction.

## Appendix B — Rejected claims

- **`REVIEW.md` C1:** `select-stream` receives `(GstElement *object,
  guint arg0, GstCaps *arg1)`; glib passes the instance as `values[0]`, so
  `values.get(2)` is correct. `gst-inspect-1.0 rtspsrc` confirms the signature.
  `rtspt://` is a supported `rtspsrc` scheme and narrows `protocols=udp+tcp`
  to TCP, so it does not fail. Do not apply either "fix"; add the missing RTSP
  test instead (VIDEO-1).
- **`LOGIC_REVIEW` 4's trigger:** ArduPilot and PX4 do not answer a GCS-sent
  `MISSION_ACK` with another ACK. The structural lack of transfer identity is
  still real (PROTO-3).
- **`LOGIC_REVIEW` 6's impact:** ArduPilot ignores writes to mission slot 0 and
  reads home from AHRS (`AP_Mission::replace_cmd`, `read_cmd_from_storage`), so
  home is not moved to 0,0. The empty-mission append case is real and is
  MISSION-1.
- **`REVIEW.md` area 15 "short plane":** the viewer skips a plane shorter than
  `rows * stride` and `frame_of` clamps; it was already correct at the reviewed
  revision.
- **`CODE_REVIEW` F2's `snapshot`:** `Fleet::snapshot` is read-only; it is not
  a state-advancing operation. The `observe`/`set_onboard_mission` seam is real
  (SVC-13).
- **`CODE_REVIEW` F9.4:** both shells title a pane from `Panel::title`, so the
  missing in-panel header is the "say it in AGENTS.md" option the review
  offered, not a defect.
- **`REVIEW.md` X1/X2:** already refuted inside the document; the guard
  temporary lives to the end of the block, and `MISSION_CURRENT.total`
  excludes home. Do not repeat.

## Appendix C — Parked

Real observations that are not worth a dedicated change; fix them only while
already editing the file.

- Mechanical: derive ordering, `Default` vs `empty()`, module declarations at
  the bottom of `app.rs`, banner comments, constructor naming, adjacent literal
  styles, history comments in tests.
- Naming: `armed`/`is_armed`, `fetch_parameters`/`download_mission`,
  `FleetUpdate`/`LinkEvent` suffixes, `is_modified`/`status` (now documented),
  `LinkManager::save` → `upsert_by_name`.
- Documentation-level: `Side::ALL` is ArduPilot's order used as a display
  order; `ArduPilot`/`PX4` interpret `modes(class)` differently; the
  `cancel == false` wording is covered by CAL-4.
- Low-value implementation: `LockMap::entry`'s retain sweep, the
  `debug_assert` guard check (constructor is private), the per-view
  lowercase, the `take()`-based one-shot in `operations.rs`, the identical
  `Scope` match arms, `Vec` vs `impl Iterator` reads, `component_mut_if`
  inserting a default entry, `Health::failing` being test-only, the
  `Idle`/`Panning` release redraw, the unused virtualized measured mode, and
  the inherited `padded_width` sign in the pane-grid fork.
- Domain design: a component's `Invalid` heartbeat can clear its
  `param_encoding` while `Vehicle::firmware()` still says ArduPilot (writes are
  refused, so it fails safe). A comment or one resolved value would close it.
