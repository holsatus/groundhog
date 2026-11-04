# Open issues

A single living list for Groundhog, merged from `CODE_REVIEW.md`,
`LOGIC_REVIEW.md` and `REVIEW.md` and verified against `5389934` on
2026-10-25, then extended with a review of `groundhog-logic` at `757d6f4` on
2026-11-04 (PROTO-7…13, MISSION-12…19, PARAM-11…15, LINK-8…9, CAL-10…14,
SVC-18…21, API-14…20; tagged "From R-1104"). Most R-1104 entries were
reproduced with a test; the rest are marked *by reading*. Only open work is
here. Fixed items are in
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
| **P1** | [MISSION-17](#mission-17--our-own-upload-is-announced-as-changed-elsewhere) — every upload to PX4 / ArduPilot 4.5+ warns "changed elsewhere" |

---

## Protocol & middleware

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

### PROTO-10 · The tolerant decoder patches only one unknown enum
**P2** · Effort S · From R-1104

`decode` re-decodes once after patching the field `InvalidEnumValue` named
(`decode.rs:117,145,159`). A message with two unknown enum values (a
`HEARTBEAT` whose `type` and `autopilot` are both newer than the dialect, a
`CAMERA_TRACKING_IMAGE_STATUS` with a new status and mode) still fails, and
the vehicle never registers.

**Fix:** loop: patch the named field in one working payload buffer and decode
again, at most once per fallback field of the message, until it decodes or
names a field without a fallback.

**Test:** a heartbeat with bytes 4 and 5 both 200 decodes, generic type and
generic autopilot.

### PROTO-13 · The parser does not resynchronise after a false start
**P3** · Effort M · From R-1104 · Upstream (mavio fork)

mavio's `FrameParser` takes any `0xFD`/`0xFE` as a frame start and never
checks the checksum, so a false start inside a payload swallows the following
real frames up to its bogus length. Fix in the fork: on a checksum failure,
restart the search one byte after the false start.

## Mission

### MISSION-17 · Our own upload is announced as "changed elsewhere"
**P1** · Effort M · From R-1104

The upload's result reaches the runtime through the `Input` channel
(`vehicle.rs` `upload_mission`), frames through the router's channel, and
nothing orders the two (`runtime.rs:170-200`). A vehicle that reports
`mission_id` (PX4, ArduPilot 4.5+) sends `MISSION_CURRENT` with the new id
right after the ACK; when it is folded before the result, `onboard_is_stale`
compares it with the old id and the fleet emits `OnboardMissionStale`. On a
multi-thread runtime this happened 30 times in 30. `clear_mission` races the
same way.

**Fix:** the runtime knows a transfer is running: the service reports its start
before the first frame and its end (with or without a result) from a drop
guard. `onboard_is_stale` is false while a transfer runs.

**Test:** multi-thread runtime, PX4 mock: upload, ACK with id 11, then
`MISSION_CURRENT` id 11, twenty times: never `OnboardMissionStale`.

### MISSION-18 · "Changed elsewhere" needs mission ids
**P2** · Effort S · From R-1104 · *by reading*

`onboard_is_stale` decides only when both ids are known (`vehicle/mission.rs:51-58`).
Older firmware reports none, so another GCS's upload leaves the draft
"Synced", and neither Start nor Upload warns as `AGENTS.md` promises.

**Fix:** when no id is reported, a reported `MISSION_CURRENT.total` that
differs from the onboard mission's item count also means stale. A partial
check; say so in the doc.

### MISSION-19 · `MissionDraft::transferred` drops edits made meanwhile
**P3** · Effort S · From R-1104

`transferred` replaces the plan with what was uploaded (`draft.rs:286`). The
iced frontend disables editing during a transfer (WP12), so it cannot happen
there, but the domain model leaves the rule to each frontend. Either document
on `transferred` that the frontend must block edits while a transfer runs, or
after an upload only rebase (`base = items`) and keep the plan.

## Parameters

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

### PARAM-14 · The INI reader accepts inf and NaN
**P2** · Effort S · From R-1104

`.param` and `.params` filter `is_finite` (`storage/parameter.rs:181`); the
INI reader does not (`:268-276`), so `f32 = inf` loads as an uploadable
value. Part of PARAM-5's one policy; listed apart because it lets a
non-finite value reach a vehicle.

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

### LINK-4 · A serial open cannot be cancelled
**P3** · Effort M · From REVIEW area 3

`link/transport.rs:70-83` runs the open in `spawn_blocking`; `OPEN_TIMEOUT`
stops waiting but the thread lives on and retries can contend for the device.

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

### LINK-8 · A reopened link races its own closing transport
**P2** · Effort M · From R-1104

`disconnect` only aborts the supervisor (`services/link.rs:248-263`); the
transport closes later, when the router's reader and writer tasks wind down.
`save` → reconnect, or Disconnect then Connect, opens the new transport
before that. A fixed-port UDP link fails with `EADDRINUSE` (28 of 50 on a
multi-thread runtime) and, with `auto_reconnect: false`, ends in `Failed`.
A serial port on Windows (exclusive handles) would do the same.

**Fix:** a new session waits for the previous one's transport to be dropped
before it opens (see WP13 for the shape).

**Test:** a fixed-port UDP link, `auto_reconnect: false`: save it with a
changed transport setting; it reaches `Connected`, 50 times in a row.

### LINK-9 · An aborted supervisor can publish after `Disconnected`
**P2** · Effort S · From R-1104

`disconnect` publishes `Disconnected` itself, while the aborted supervisor,
mid-poll on another worker, can still publish `Connected`
(`link.rs:248-263` vs `:376-414`). The last event says connected (and the
app toasts it) while `status()` says disconnected; 26 of 200 runs.

**Fix:** the supervisor is the only publisher (see WP13).

## Firmware & calibration

### CAL-14 · PX4 sub-modes outside AUTO are dropped
**P2** · Effort S · From R-1104

`decode_mode` zeroes the sub byte unless the main mode is AUTO
(`firmware/px4.rs:153-173`). PX4 reports Orbit as POSCTL with sub-mode 1 (and
newer PX4 Position Slow as sub-mode 2), so an orbiting vehicle reads
"Position", and entering or leaving Orbit raises no `ModeChanged`. Verify the
values against PX4's `px4_custom_mode.h` before adding them.

## Services & vehicle state

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
final flush (the test sleeps to hide it). R-1104 adds: `stop()` leaves the
unbiased `select!` without draining frames already queued, and the timestamp
and the frame are two `write_all`s, so a failed flush can leave a timestamp
without its frame. Write each record with one `write_all`.

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

### SVC-12 · Invalid UTF-8 drops a whole name
**P3** · Effort S · From REVIEW area 4

`string_from_cstring` (`vehicle/mod.rs:606-616`) rejects the whole name, while
statustexts decode lossily. One stray byte loses vendor/model.

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

### SVC-18 · Background tasks outlive the `Backend`
**P3** · Effort S · From R-1104

The runtime starts each bootstrap and each `describe_camera` with a bare
`tokio::spawn` (`runtime.rs:187-195`). Aborting the runtime does not abort
them: they keep a `FleetHandle` and run their transactions until those fail.
`AGENTS.md` says dropping the `Backend` stops all of it.

### SVC-19 · A vehicle is bootstrapped once per process
**P2** · Effort M · From R-1104 · *by reading*

`Registered` fires only for a new system id, and vehicles are never removed
(`fleet.rs`, `runtime.rs:185-196`). A bootstrap that failed over a lossy link
is never retried, and after a reboot (or a reflash) parameters, home and the
onboard mission stay as they were before it.

### SVC-20 · Disabled ArduPilot arming checks read "Not ready" forever
**P2** · Effort S · From R-1104

ArduPilot always marks `PREARM_CHECK` present, enabled only while arming checks
are enabled, and healthy only when enabled and passing
(`GCS_MAVLink/GCS.cpp`). `prearm_passes` ignores `enabled`
(`health.rs:166-170`), so `ARMING_CHECK = 0` shows a red "Not ready" forever.
Present but not enabled means "checks disabled": say that, not "failing".

### SVC-21 · Two `persist` edges
**P3** · Effort S · From R-1104

`write_durably` creates the temporary file with default permissions and only
then copies the target's (`persist.rs:79-88`), so a `0600` file's contents
are briefly readable by others: create it with the target's mode
(`OpenOptions` + `mode()` on unix). `read_or_set_aside` renames onto
`<name>.corrupt` (`:139-147`), destroying an earlier set-aside file: pick a
free name as `recording.rs` does.

## UI & shells

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

The marked input is still built in a feature (`parameter_panel.rs:649`); move
it into `ui` as `text_input_marked`. The "More" trigger is permanently
`Active` (`ui/tool.rs:130`), icons do not scale with density
(`ui/style.rs:86,95`), and the attitude docs say ±30° while
`MAX_ATTITUDE_DEG` is 40 (`widget/attitude.rs:4,16,27`).

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
display-boundary passthrough and say so in `AGENTS.md`. R-1104 adds:
`set_current_mission_item` reports an out-of-range index as
`Error::Protocol(format!(..))` (`vehicle.rs` `set_current_mission_item`);
give it `Error::NoSuchItem { index }`. `DownloadOutcome::Failed(String)` can
hold the services `Error`, which is `Clone`.

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
Components are a bare `u8` throughout (`component_state(u8)`,
`target_for(u8)`); a `ComponentId` would match `SystemId`.

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

### API-14 · A component client sends vehicle commands to the autopilot
**P2** · Effort M · From R-1104 · Latent

`FleetHandle::component(address)` returns a `VehicleClient` whose component
only parameters honour: `arm`, `set_mode`, mission and calibration operations
target `Vehicle::target()` (`vehicle.rs` `run`, `upload_mission`, …). So
`fleet.component(gimbal).arm(true)` arms the autopilot. The GUI uses
`component` only for parameters today (`parameter_panel.rs:154,391`).

**Fix:** a `ComponentClient` with the per-component operations (parameters),
and a `VehicleClient` with the vehicle's; neither can address the other's
target.

### API-15 · One reader, unbounded queues, nothing to await
**P2** · Effort M · From R-1104

`FleetUpdates` and `LinkEvents` each have one consumer and an unbounded queue
(`runtime.rs:104-105`, `services/link.rs:186`): a frontend that does not
drain them grows without bound, and a second reader (a logger beside the UI)
cannot subscribe. `FleetHandle` offers `snapshot()` and
`wait_for(system, timeout) -> bool`; "wait for any vehicle" or "wait until
parameters are downloaded" means polling.

### API-16 · A vehicle's readiness is invisible
**P2** · Effort M · From R-1104

`VehicleClient::bootstrap` is `pub` although the runtime runs it
(`vehicle.rs:177`). Nothing says when it is done, so a headless client that
sets a parameter right after the vehicle appears gets
`ParamEncodingUnknown`, and its own `fetch_parameters` races bootstrap's.

### API-17 · Operator safety rules live in the frontend
**P2** · Effort M · From R-1104

`AGENTS.md` puts rules every frontend would share in the logic crate, as
`MissionDraft::check` does for missions. These are in the GUI: whether a mode
change asks (`features/vehicles/mod.rs:305-309`), what blocks a calibration
(armed, lost; `features/calibration.rs:129-135`), and the heading shown (yaw,
else course over ground above 0.5 m/s; `features/vehicles/map.rs:367`).
`start_calibration` itself does not refuse an armed vehicle.

### API-18 · Public enums are not `#[non_exhaustive]`
**P3** · Effort S · From R-1104

No item in the crate is. `FleetEvent`, `services::Error`, `Rejection`,
`MissionRejection`, `LinkStatus`, `DownloadOutcome` and
`calibration::Outcome` will grow, and a new variant breaks every frontend's
exhaustive match. `FleetEvent` also mixes tuple variants
(`ArmedChanged(SystemId, bool)`) with struct variants
(`LivenessChanged { system, .. }`), and has no `system()` accessor for
per-vehicle filtering.

### API-19 · Internals and dependency types in the public API
**P3** · Effort M · From R-1104

- `Firmware` and `CalibrationProtocol` are public, implementable traits, but
  an implementation cannot be registered (`for_autopilot` is crate-private),
  and they expose wire builders (`start/confirm/cancel -> CommandLong`,
  `set_mode -> SetMode`) and a fold (`observe`), against `AGENTS.md`'s "instead
  of the firmware's command builders".
- `DownloadUpdate` (a runtime input), `Vehicle::new`, `Vehicle::is_authoritative`,
  `target`/`target_for`, and `MissionProgress::current_seq` with
  `current_index(layout)` (which invites the by-hand seq/index conversion
  `Vehicle::current_mission_index` exists to avoid) are public.
- nalgebra 0.34 is in public fields (`Attitude::attitude: UnitQuaternion`,
  `velocity_ned: Vector3`); the GUI calls `euler_angles()` without depending
  on nalgebra.
- `mav_param::{Ident, Value}` are needed to call `set_parameter` but not
  re-exported, so every frontend pins the same git revision.

### API-20 · Units and sentinels break the crate's own conventions
**P3** · Effort M · From R-1104

- No unit in the name: `BatteryState { voltage, current, remaining }`,
  `Home::altitude_msl`, `climb_rate()`, `ground_speed()`.
- `GlobalPosition::alt_agl` is documented "above home": that is relative
  altitude, not AGL.
- Sentinels instead of `Option`: `OnboardMission::opaque_id` (0 = none) next
  to `MissionProgress::mission_id: Option<u32>`; `ParamDownload::expected()`
  (0 = unknown).
- `status::rank(MavSeverity)` is inverted (lower is worse), and the GUI
  compares ranks by hand (`app.rs:622`).
- `arm(bool)` and `pause_mission(bool)` take a flag for two operations.
- Mission transfers report progress only through a `&dyn Fn` callback
  (callers write `&|_| {}`), while parameter downloads show theirs in the
  snapshot.

---

## Appendix A — Fixed since the reviews

Verified at `5389934`; do not re-open without new evidence.

- **CODE_REVIEW:** F8.1 (module docs), F8.2 (`ui::either`), F8.4 (dead code),
  F10's shared feature test harness; F11.1 (copy-on-write before change),
  F11.2 (calibration loss through one ending path), F11.4 (a declined draft
  follows once reverted), F11.5 (battery doc); F3.2's regression (`Invalid` heartbeat no longer resets
  firmware, with a test); F4.1's parameter Cancel and one `Progress` type;
  F4.3's delete-link confirmation; F5's `CalibrationFinished` payload; F9.1's
  mission checkbox and `ui::dialog`/`ui::toast`.
- **LOGIC_REVIEW:** 1 (newer enum values: one tolerant decoder for heartbeats,
  ACKs and status messages, and raw mission items), 2 (GCS heartbeat), 3 (mode
  picker confirmation), 4 (mission transfer identity), 5 (parameter echo
  preference), 9
  (`MISSION_CURRENT.total`), 12 (a component that reports no parameters),
  14.3 (recording names), 14.4 (`reached_seq`).
- **REVIEW.md:** H1 (parameter writes wait for a list download and share the
  channel only with other sets); areas 3, 4, 6 and 8 (C-cast saturation and
  64-bit parameter types, repeat `PARAM_VALUE` copies, one parameter-download
  model with stale values removed); area 1 (transaction cancellations reach a
  congested queue); area 5 (a mission's home, frames, validation and jump
  targets at the boundaries); area 10 (selection, edits and file loads around a
  transfer); areas 7 and 8 (a calibration that waits,
  times out, reboots and explains cancelling; one way to end a session);
  areas 9 and 11 (an overlay that fits the window and keeps clear of the
  system bars); H4–H16 (parameter idle loop, persist/set-aside/fsync, class
  calibration offering, runtime stop, calibration drop guard, empty-upload
  confirmation, Start gate, 0,0 waypoint placement, parameter-load/upload
  identity, back behind confirmation, map drag cancel, pane minimums, adaptive
  menu); H17's `unreachable_pub` and `ParamEncoding` (the test seam is now
  behind `test-support`); the parameter-specific part of H18.
- **Medium fixes:** persist temp names/permissions, link schema defaults and
  renames, `wire::latlon`, battery summary merge, zero quaternion, mission
  `with_command`/`next_altitude`, compass `id+1` overflow, same-second
  recording names, settings save-on-close, gallery data dir, config
  version/migration, link save preserving a connection, parameter text input,
  changed badge, link-name validation, map Escape, tiled/full-screen insets
  (partly), Android CI job, docs fork correction.

## Appendix B — Rejected claims

- **PROTO-12's fix ("`mission_clear` should set `not_before`"):**
  `not_before` only excludes what arrived before the transfer began, and a
  listener installed for the clear never sees those anyway. A late
  `MISSION_ACK` that arrives *during* the clear cannot be told from its
  answer: it carries no transfer id. That is PROTO-9's limitation for
  missions, documented with it (WP33).
- **R-1104's "undefined behaviour in `mav-param`'s `into_bytewise`":** the
  pinned revision (`04034d9`) zero-initialises the union before writing a
  value narrower than four bytes; the claim was read from an older checkout.
- **R-1104's "never cancel a command the transaction gives up on":** the
  caller is told the command failed, so the vehicle must not go on with it;
  only the premature give-up is a bug (PROTO-8).
- **`PARAM-2`'s "empty complete list":** the protocol cannot tell a component
  without parameters from one that never answered, so nothing-arrived is its
  own `NoParameters` outcome with a reason and a retry, not `Complete (0, 0)`.
- **`PARAM-1`'s one lock per reply channel:** making every `PARAM_VALUE` of a
  target one exclusive lock would serialise every set. Sets conflict only with
  list downloads and with sets of the same parameter, so the channel is
  shared by sets and held exclusively by a download.
- **`PROTO-1`'s mission-item patch:** substituting a known command for an
  unknown one (the suggested fixed-offset patch) would upload a different
  command on the next round trip. Mission items are stored raw instead.
- **`REVIEW.md` C1:** `select-stream` receives `(GstElement *object,
  guint arg0, GstCaps *arg1)`; glib passes the instance as `values[0]`, so
  `values.get(2)` is correct. `gst-inspect-1.0 rtspsrc` confirms the signature.
  `rtspt://` is a supported `rtspsrc` scheme and narrows `protocols=udp+tcp`
  to TCP, so it does not fail. Do not apply either "fix"; add the missing RTSP
  test instead (VIDEO-1).
- **`LOGIC_REVIEW` 4's trigger:** ArduPilot and PX4 do not answer a GCS-sent
  `MISSION_ACK` with another ACK. The structural lack of transfer identity was
  real, and is fixed by correlating an accepted ACK with the served items and
  a reply with its own request.
- **`LOGIC_REVIEW` 6's impact:** ArduPilot ignores writes to mission slot 0 and
  reads home from AHRS (`AP_Mission::replace_cmd`, `read_cmd_from_storage`), so
  home is not moved to 0,0. The empty-mission append case is real and is
  MISSION-1.
- **`REVIEW.md` area 15 "short plane":** the viewer skips a plane shorter than
  `rows * stride` and `frame_of` clamps; it was already correct at the reviewed
  revision.
- **`CODE_REVIEW` F2's `snapshot`:** `Fleet::snapshot` is read-only; it is not
  a state-advancing operation. The `observe`/`set_onboard_mission` seam the
  frontend tests need is behind the `test-support` feature, not public API.
- **`CODE_REVIEW` F9.4:** both shells title a pane from `Panel::title`, so the
  missing in-panel header is the "say it in AGENTS.md" option the review
  offered, not a defect.
- **`REVIEW.md` X1/X2:** already refuted inside the document; the guard
  temporary lives to the end of the block, and `MISSION_CURRENT.total`
  excludes home. Do not repeat.

## Appendix C — Parked

Real observations that are not worth a dedicated change; fix them only while
already editing the file.

- **A WPL file without a home row (MISSION-7):** row 0 stays home. Detecting
  the difference from the `current`/`index` columns risks dropping a real item
  or inventing a waypoint at home.

- **A late ACK of a resent command (PROTO-9):** documented at
  `CommandRequest` and in `AGENTS.md`; a drain after resends was declined.

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
  Re-observed by R-1104: a device heartbeating as component 1 with `Invalid`
  would flip the encoding, and re-decode every parameter, each second. Still
  parked: component 1 is the autopilot's.
