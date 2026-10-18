# One interface, from desktop to phone

How Groundhog's interface adapts to a phone without turning into a second
application, and without feeling like the desktop squeezed onto glass. This
is a plan: what is wrong with the floating shell as it stands, the idioms a
phone user expects, how a ground control station fits them, and the work in
order.

## What the first pass got wrong

The floating shell moved the desktop's panes onto a map. That proved the
plumbing (insets, touch, a map that fills the window), but not the design:

- **Navigation is the desktop's rail, turned sideways.** Seven unlabelled
  icons, one per panel, in a bar that grows with every panel added. Phones
  give a navigation bar three to five labelled destinations; past that,
  content belongs inside a destination, not beside it.
- **A sheet is a pane in a box.** Title bar, maximise and close buttons,
  a fixed half of the screen. Phone sheets are dragged by a handle between
  a few heights, and the map stays usable above them.
- **The panels are laid out for a pane 400–600 pixels wide, with a mouse.**
  `ui::field` puts a 140-pixel label beside every control, sliders share a
  line with their value, read-outs come three to a row, features hard-code
  widths (the mode picker 160, Arm 96, log columns 180), icon buttons are 32
  pixels and list rows 26. On a phone at a raised scale, these collide.
- **The vehicle's state is hidden behind a tap.** On the desktop the
  vehicles panel is always open. On a phone, mode, arming, battery and GPS
  disappear as soon as another panel is chosen, and those are what an
  operator glances at most.
- **Interactions are pointer interactions.** Hints that only show on
  hover, a centred dialog for confirmations, middle and right clicks, no
  back button.

## What a phone user expects

Taken from Material 3 (including Material 3 Expressive) and Apple's
conventions, minus Liquid Glass:

- **Window size classes, not device types.** Width: compact below 600 dp
  (phones in portrait), medium 600–840 (tablets in portrait, foldables),
  expanded 840–1200, large to 1600, extra-large beyond. Height: compact
  below 480 dp, which is nearly every phone in landscape. A phone in
  landscape is medium wide but compact high, so two panes side by side do
  not fit there either.
- **Navigation follows the class.** A bottom navigation bar of three to
  five labelled destinations on compact widths; a navigation rail (three to
  seven) on medium and wider windows and on short ones. Material 3
  Expressive retires the navigation drawer in favour of an expanded rail,
  and the bottom app bar in favour of toolbars: a docked toolbar for
  actions common to a screen, a floating toolbar for actions on its
  content. A navigation bar and a toolbar are not shown together: the bar
  on top-level screens, toolbars on the screens below them.
- **Sheets.** A *standard* bottom sheet sits beside the main content,
  which stays usable (a map app's place details); a *modal* one covers it
  with a scrim and is dismissed like a dialog. Sheets rest at a few heights
  (a peek, about half, nearly full) and are dragged between them by a
  handle; on a wide window the same content becomes a side pane.
- **Canonical layouts.** *Supporting pane* (main content and a pane about
  it, stacked into a sheet on compact widths) and *list-detail* (a list and
  the selected item, one at a time on compact widths, with back between
  them) cover almost everything Groundhog shows.
- **Touch targets of 48 dp,** list rows of 48–56 dp, labels above fields
  on narrow screens, primary actions where the thumb reaches (the bottom
  half), and the system back gesture undoing the last step of navigation.
- **Ground control stations** add their own idioms. QGroundControl
  confirms consequential guided actions (takeoff, land, arm) with a
  *slide-to-confirm* control, cancellable, and keeps a toolbar of
  flight-readiness indicators along the top. DJI Fly keeps the camera view
  full-screen, with a status strip (safety status, signal) along the top.

## Principles

1. **One interface.** Features describe what a panel holds; the toolkit
   decides how a part is drawn in the space it gets; the shell decides
   where the parts go. Nothing in `features/` asks whether it runs on a
   phone.
2. **Adapt to the space and the input, separately.** Components respond to
   the width they are given, like container queries, so a narrow desktop
   pane and a phone get the same layout. Touch makes targets larger
   (density) whatever the window's size. A touch-screen laptop gets the
   desktop layout with touch-sized targets.
3. **Navigate by task, not by panel.** An operator flies, plans or sets up
   a vehicle. Panels become the content of those destinations, and new
   panels join a destination instead of widening the bar.
4. **The map is the canvas for spatial work, pages for the rest.** Flying
   and planning happen on the map with a standard sheet over it. Parameters,
   calibration, links, logs and settings are full-screen pages, which is
   what a phone does with forms and lists.
5. **The vehicle's state is always on screen.** A status strip, the same
   component on every shell.
6. **The same design language.** Tokens, tones, badges, overline section
   headings, one flat surface per pane, and the anatomy of a pane (header,
   actions, banners, activity, body) carry over unchanged; only where each
   part sits differs.

## Shells by size class

| Window | Typical device | Shell | Navigation | Secondary content |
|---|---|---|---|---|
| Compact width (< 600) | Phone, portrait | Mobile | Bottom bar, five labelled destinations | Bottom sheet with detents, or a page |
| Compact height (< 480) | Phone, landscape | Mobile | Rail at the left (behind the cutout's inset) | Side sheet beside the rail, or a page |
| Medium (600–840) | Tablet in portrait, foldable | Mobile, wide | Rail | Supporting pane beside the map instead of over it |
| Expanded and wider, not compact height | Tablet landscape, desktop | Tiled | Rail, grouped by destination | The workspace's panes |

`ShellMode::Automatic` resolves from these classes instead of from a
touch flag and the shortest side. A setting still overrides it.

Density follows the shell: the mobile shell is touch-sized, the tiled one
pointer-sized, and a setting (*Touch-sized controls*) makes every shell
touch-sized, for a touch-screen laptop or a tablet in the tiled layout.

The floating shell becomes the *mobile* shell. The tiled shell keeps its
workspace and gains the same status strip and destination grouping, so
the two read as one application.

## Navigation

Five destinations, the same in every shell:

| Destination | Holds | On a phone |
|---|---|---|
| **Fly** | The selected vehicle: status, flight actions, telemetry (today's Vehicles panel) | Map, status strip, standard sheet |
| **Plan** | The mission editor | Map, standard sheet (list, then the selected item) |
| **Setup** | Parameters, calibration; later radio, motors, failsafes, frame | A page listing them, each a page of its own |
| **Links** | The links, their status and the link editor | A page |
| **More** | Log, settings | A page listing them |

Links has a destination of its own because operators switch radios and
SITL often, and a link's state is what to check first when a vehicle goes
quiet. New vehicle configuration goes under Setup, and new ground station
features under More. The bar stays at five, the most a navigation bar
holds.

The link indicator in the status strip also opens Links, and an alert in
the strip opens the log.

On the desktop, the rail keeps a button per panel (there is room, and
opening several panels at once is the point of the workspace) but groups
them by destination with a gap between groups, in the same order. Fly and
Plan share one map instance, so switching between them on a phone keeps
the map where it was.

## Screens on a phone

### Fly

```
┌───────────────────────────────┐
│▓▓▓▓▓▓▓▓ system status ▓▓▓▓▓▓▓▓│  scrim over the map
│ ╭───────────────────────────╮ │
│ │Copter 1 ▾ LOITER ARMED 76%│ │  status strip: vehicle chip, mode,
│ ╰───────────────────────────╯ │  arming, battery, GPS, link
│ ⚠ Radio failsafe            › │  a banner while an alert is current
│                         (◎)   │  map controls: follow, locate
│            map                │
│                   (compass)   │
│ ╭───────────── ═══ ─────────╮ │  drag handle
│ │ [  Return ] [ Land ] [Mode]│ │  peek: flight actions
│ │ Alt 12.5 m  Spd 2.5  Bat 76│ │  and the four numbers that matter
│ ├ ─ ─ ─ ─ ─ half ─ ─ ─ ─ ─ ─ ┤ │
│ │ FLIGHT · HEALTH · NAV …    │ │  the rest of today's panel
│ ╰────────────────────────────╯ │
│ ✈ Fly ⌖ Plan ⚙ Setup 🔗 ⋯     │  navigation bar
└───────────────────────────────┘
```

- **Status strip.** Always visible, floating below the status bar. Tapping
  the vehicle chip picks a vehicle (a modal sheet listing the fleet);
  tapping the rest raises the sheet to half.
- **Sheet** with three detents: *peek* (the flight actions and the few
  read-outs that matter in flight), *half*, and *full*. It is a standard
  sheet: the map stays live above it, and the map centres the vehicle in
  the part the sheet leaves visible (`map::Placement` already does this).
- **Flight actions** in the peek, within thumb reach. Arm, takeoff, land,
  return and starting a mission are confirmed by sliding (below); the
  desktop keeps its dialog.
- **Alerts** become a banner under the strip while they are current, as on
  the desktop's vehicle panel; transient notices become snackbars above
  the sheet.

### Plan

The same map and strip, with the mission in the sheet:

- *Peek:* the plan's state (Synced, Not uploaded), item count, and Upload
  or Download.
- *Half and full:* the item list. Tapping an item, or its marker on the
  map, replaces the list with that item's editor, with a back arrow in the
  sheet's header: list-detail inside the sheet.
- A floating action button toggles *add waypoints*: while it is on, a tap
  on the map adds one. A tap never adds a waypoint by surprise.
- Long-pressing a marker opens its actions (delete, insert before, jump
  to), which is what right-click and hover would have done.

### Setup and More

Pages, not sheets: a top app bar (back, title, at most two actions, the
rest in an overflow menu), then the content, scrolling.

- **Parameters:** search in the top bar, a "changed only" filter chip,
  rows two lines high (name; value and unit), tapping a row opening an
  editor with a numeric field. While edits are pending, a docked toolbar
  at the bottom offers Upload and Revert.
- **Calibration:** a card per calibration with its state, each opening the
  guided run as a page of its own.
- **Links, log, settings:** as they are, laid out by the adaptive
  components below.

### Landscape and medium widths

A phone in landscape uses the rail at the left, the status strip across the
top of the map, and the sheet as a side sheet beside the rail. A medium
window (tablet portrait) puts the same content in a supporting pane beside
the map instead of over it.

## Components that adapt

The toolkit takes the space it is given and the density, and the panels
stop fixing sizes themselves:

- **`ui::field`:** the label beside the control while there is room for
  both (label column plus a usable control, about 360 logical pixels),
  above it otherwise. A slider's field puts the label and the value on one
  line and the slider full width beneath.
- **Read-outs:** a grid with as many columns as fit a minimum cell width,
  so three on the desktop and two on a phone, instead of `stats` always
  making three.
- **`ui::header`:** icon actions that do not fit move into an overflow
  menu.
- **`ui::actions`:** groups already wrap; on a narrow width the primary
  action takes the full width.
- **Lists:** row height and line count from the density: a single 26-pixel
  line for the pointer, two lines at 48–56 for touch.
- **No fixed widths in features.** The widths in the mode picker, the Arm
  button, the log's columns and the settings' slider value become toolkit
  tokens or minimums, so the toolkit can give way.
- **Density:** `ui::size` tokens become a function of a density
  (`Pointer`, `Touch`): controls and icon buttons 32 → 44, list rows
  26 → 44, larger checkboxes and slider handles. Density is not the scale
  factor. Scaling enlarges text and everything with it, while density
  makes targets reachable at the same text size.

iced has no context to pass the density through, so the toolkit reads it
from one place the application sets before each view. The width-adaptive
parts are layout widgets (`widget::adaptive`) that choose an arrangement
by the width they are offered. They are not `responsive`, which rebuilds
its content on every layout and would do so about 30 times a second while
snapshots arrive.

## Interaction idioms

- **Back.** The system back gesture (Android's back key) collapses the
  sheet a detent, then closes it, then pops a page, and only then leaves
  the application. On the desktop, Escape does the same where it means
  something.
- **Confirmations.** In the mobile shell, a command that changes what the
  vehicle does is confirmed by sliding a thumb across a control. That is
  harder to trigger by accident than a tap in a dialog, and it is how
  QGroundControl confirms guided actions. The tiled shell keeps its dialog,
  which is quicker with a mouse. A destructive change to stored data
  (clearing a mission, restoring defaults) asks in a modal sheet with a
  scrim and large buttons in the mobile shell, and in the dialog in the
  tiled one.
- **Long press** stands in for right-click and hover: a marker's actions,
  a parameter's description.
- **No hover-only information.** Without tooltips on touch, an icon button
  on a phone either has a self-evident icon or becomes a labelled button.
  Toolbars and sheets on a phone favour labels.
- **Notices.** Snackbars, one at a time, above the sheet and the
  navigation bar, for transient results. Vehicle alerts are a banner under
  the status strip while current. The log keeps the history, as now.
- **Text entry** stays short (search, a numeric value), in a field large
  enough to tap, because the iced fork's IME support is basic.

## How the code changes

1. **Size classes and density** (`shell.rs`, `ui`): a `WindowClass` from the
   window's width and height, `Density` from the platform, both resolved
   in `Application` and handed to the shell and the toolkit.
2. **Destinations** (`layout::Panel` gains a destination; a `Destination`
   enum): the navigation bar and the rail's grouping read from it.
3. **Panels return their anatomy instead of composing it.** A feature's
   `view` returns its parts (header subject and status, action groups,
   banners, activity, body, and a short *summary* for a sheet's peek or a
   status strip) and `ui::panel_with` becomes one way of laying those parts
   out. The mobile shell puts the header in a sheet's or page's top bar and
   the actions in a peek or a docked toolbar. This is what lets one feature
   serve both shells without asking which one it is in.
4. **New widgets:** a sheet with detents and a handle (its content's
   scrolling separate from the drag at first), slide-to-confirm, the status
   strip, a navigation bar and a top app bar, built on `widget::surface`
   for whatever floats over the map.
5. **A page stack** for the mobile shell (Setup → Parameters → an edit),
   popped by back.

The tiled shell needs only the status strip, the rail's grouping and the
adaptive components; its workspace is unchanged.

## Order of work

Each step leaves both shells working, and each changes the gallery, which
should render every panel at 360 logical pixels with touch density
alongside today's renders.

1. **Adaptive components and density** (done). `ui::field` stacks its
   label, `ui::stats` drops columns, headers fold their tools into a menu
   (`ui::tool`, `widget::menu`), parameter rows narrow (`ui::either`), and
   controls, rows, checkboxes and sliders are touch-sized at touch
   density. The gallery renders every panel at a phone's width, touch-sized.
2. **Destinations and pages.** The five-item navigation bar with labels,
   the rail grouped the same way, the page stack with a top app bar, and
   the back key.
3. **Fly.** The status strip on both shells, the sheet with detents, alert
   banners and snackbars, map controls.
4. **Plan.** List-detail in the sheet, the add-waypoint button, long press
   on markers.
5. **Confirmations.** Slide-to-confirm for flight commands, modal sheets
   for destructive stored changes on touch.
6. **Medium windows.** The supporting pane beside the map for tablets in
   portrait.

## Decisions

- **Five destinations:** Fly, Plan, Setup, Links and More.
- **Slide-to-confirm only in the mobile shell;** the tiled shell keeps its
  dialog.
- **Density follows the shell,** and a setting makes it touch-sized
  everywhere.

## Sources

- Window size classes: [Android Developers, use window size classes](https://developer.android.com/develop/ui/compose/layouts/adaptive/use-window-size-classes)
- Canonical layouts: [Android Developers, canonical layouts](https://developer.android.com/develop/ui/compose/layouts/adaptive/canonical-layouts)
- Navigation bar and rail counts: [SAP Fiori for Android, navigation bar (Material 3)](https://www.sap.com/design-system/fiori-design-android/v25-8/components/m3-standard-components/navigation-bar/usage), [navigation rail](https://www.sap.com/design-system/fiori-design-android/v26-4/components/m3-standard-components/navigation-rail/usage)
- Material 3 Expressive navigation and toolbars: [9to5Google on navigation](https://9to5google.com/2025/05/14/material-3-expressive-navigation/), [on toolbars](https://9to5google.com/2025/05/18/material-3-expressive-toolbars/), [Material Components: docked and floating toolbars](https://gitea.com/784622644/material-components-android/src/branch/master/docs/components/DockedFloatingToolbars.md)
- Bottom sheets: [Material Components for Android, bottom sheet](https://gitea.com/784622644/material-components-android/src/branch/master/docs/components/BottomSheet.md)
- Touch targets: [Android Developers, Compose accessibility](https://developer.android.com/develop/ui/compose/accessibility/key-steps)
- Ground control idioms: [QGroundControl, Fly View](https://docs.qgroundcontrol.com/v4.4.3/en/qgc-user-guide/fly_view/fly_view.html)
