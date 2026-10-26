//! The workspace layout: three columns of panes around the map.
//!
//! The left and right columns stack panes top to bottom. The middle column
//! holds the map, with a row of panes beneath it, left to right. A panel
//! opens at the end of the stack it is opened in (the bottom of a column,
//! the right of the row) and takes exactly `1/(n + 1)` of a stack that held
//! `n` panes, the others keeping their proportions. A panel can also be
//! dragged in from its rail button, and lands where a dragged pane would.
//! The map is fixed: it cannot be moved or closed.
//!
//! The side columns' widths and the row's height are kept in pixels, not
//! fractions: when the window is resized, the map gives and takes the
//! space. A stack that empties keeps its size, so it comes back as it was.
//!
//! The arrangement is kept in a [`Workspace`], which is also what is
//! persisted. The pane grid's state is derived from it, by
//! [`pane_grid::State::rearrange`], whenever its structure or the grid's
//! size changes; that keeps every pane's identity, and so its widget state.
//! A dragged divider changes only what it bounds in the workspace (a
//! column's width, the row's height, or the shares of the two panes beside
//! it), clamped to what the arrangement draws, and the grid is laid out
//! again from that while the drag goes on.

use std::collections::BTreeMap;

use iced::{Point, Rectangle, Size};

use crate::{
    ui::{self, Icon},
    widget::pane_grid::{self, Axis, Configuration, DropHint, Edge, Region, Target},
};

/// Space between panes, in logical pixels.
pub const SPACING: f32 = ui::space::S;
/// The smallest a pane gets when the operator resizes its neighbours.
pub const MIN_PANE: f32 = 120.0;
/// The smallest the map gets, along either axis, when the window shrinks.
const MIN_MAP: f32 = 200.0;
/// How close to a divider a drop counts as on it, in logical pixels from
/// its centre line.
const SEAM_ZONE: f32 = SPACING / 2.0 + 6.0;
/// The middle of a pane where a drop swaps the two, as a fraction of the
/// pane's width and height.
const SWAP_ZONE: f32 = 1.0 / 3.0;
/// The share of the map, from its left and right edges, where a drop goes
/// to that side column.
const MAP_SIDE_ZONE: f32 = 0.25;
/// The grid's size before it is first measured.
const ASSUMED_SIZE: Size = Size::new(1600.0, 900.0);

/// A view that can occupy a pane in the workspace grid.
///
/// The variant names are the on-disk identity of a pane: they are pinned with
/// explicit serde renames so renaming a Rust variant cannot silently invalidate
/// saved layouts.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum Panel {
    #[serde(rename = "Map")]
    Map,
    #[serde(rename = "Vehicles")]
    Vehicles,
    #[serde(rename = "Mission")]
    Mission,
    #[serde(rename = "Parameters")]
    Parameters,
    #[serde(rename = "Calibration")]
    Calibration,
    #[serde(rename = "Links")]
    Links,
    #[serde(rename = "Logs")]
    Logs,
    #[serde(rename = "Settings")]
    Settings,
    #[serde(rename = "Video")]
    Video,
}

impl Panel {
    pub fn title(self) -> &'static str {
        match self {
            Panel::Map => "Map",
            Panel::Vehicles => "Vehicles",
            Panel::Mission => "Mission",
            Panel::Parameters => "Parameters",
            Panel::Calibration => "Calibration",
            Panel::Links => "Links",
            Panel::Logs => "Log",
            Panel::Settings => "Settings",
            Panel::Video => "Video",
        }
    }

    /// The panel's icon on the rail.
    pub fn icon(self) -> Icon {
        match self {
            Panel::Map => Icon::Map,
            Panel::Vehicles => Icon::Vehicle,
            Panel::Mission => Icon::Mission,
            Panel::Parameters => Icon::Parameters,
            Panel::Calibration => Icon::Calibration,
            Panel::Links => Icon::Links,
            Panel::Logs => Icon::Log,
            Panel::Settings => Icon::Settings,
            Panel::Video => Icon::Video,
        }
    }

    /// What the panel is for, in a line: shown where it is listed among
    /// others (the floating shell's Setup and More pages).
    pub fn summary(self) -> &'static str {
        match self {
            Panel::Map => "The vehicles and the mission, on a map",
            Panel::Vehicles => "The selected vehicle: its state, commands and read-outs",
            Panel::Mission => "The mission: its items, and transfers to the vehicle",
            Panel::Parameters => "Read and change what the autopilot stores",
            Panel::Calibration => "Calibrate the accelerometer, compass and other sensors",
            Panel::Links => "Connections to vehicles",
            Panel::Logs => "Status messages from the vehicles, and notices",
            Panel::Settings => "Appearance, planning and recording",
            Panel::Video => "The camera's stream, and where it comes from",
        }
    }

    /// Fixed panels cannot be dragged or closed.
    pub fn is_fixed(self) -> bool {
        matches!(self, Panel::Map)
    }
}

/// One of the three places a panel can open in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Left,
    /// The row beneath the map.
    Middle,
    Right,
}

impl Slot {
    const ALL: [Slot; 3] = [Slot::Left, Slot::Middle, Slot::Right];
}

/// The size a stack is given when nothing says otherwise, in logical pixels.
const DEFAULT_STACK_SIZE: f32 = 300.0;

/// The panes of one slot, in order, each with its share of the stack.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Stack {
    /// The width of a side column, or the height of the row beneath the map,
    /// in logical pixels. Kept while the stack is empty.
    #[serde(default = "default_stack_size")]
    pub size: f32,
    /// The panes and their shares of the stack's length, relative to each
    /// other. A panel this build does not know is dropped on loading.
    #[serde(default, deserialize_with = "known_panes")]
    pub panes: Vec<(Panel, f32)>,
}

fn default_stack_size() -> f32 {
    DEFAULT_STACK_SIZE
}

/// The panes of a stack as saved, without those whose panel this build does
/// not know (one removed since the file was written): one unknown panel must
/// not make the whole configuration unreadable.
fn known_panes<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<(Panel, f32)>, D::Error> {
    use serde::Deserialize;

    let panes = Vec::<(KnownPanel, f32)>::deserialize(deserializer)?;
    Ok(panes
        .into_iter()
        .filter_map(|(KnownPanel(panel), share)| Some((panel?, share)))
        .collect())
}

/// A panel as saved: `None` for a name this build does not know.
struct KnownPanel(Option<Panel>);

impl<'de> serde::Deserialize<'de> for KnownPanel {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::{EnumAccess, IntoDeserializer, VariantAccess, Visitor, value};

        /// A variant's name, whatever it is.
        struct Name(String);

        impl<'de> serde::Deserialize<'de> for Name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct Identifier;
                impl Visitor<'_> for Identifier {
                    type Value = Name;
                    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                        f.write_str("a panel name")
                    }
                    fn visit_str<E: serde::de::Error>(self, name: &str) -> Result<Name, E> {
                        Ok(Name(name.to_owned()))
                    }
                }
                deserializer.deserialize_identifier(Identifier)
            }
        }

        struct Variant;
        impl<'de> Visitor<'de> for Variant {
            type Value = String;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a panel")
            }
            fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<String, A::Error> {
                let (Name(name), variant) = data.variant::<Name>()?;
                variant.unit_variant()?;
                Ok(name)
            }
            fn visit_str<E: serde::de::Error>(self, name: &str) -> Result<String, E> {
                Ok(name.to_owned())
            }
        }

        let name = deserializer.deserialize_enum("Panel", &[], Variant)?;
        let deserializer: value::StrDeserializer<'_, value::Error> =
            name.as_str().into_deserializer();
        Ok(KnownPanel(Panel::deserialize(deserializer).ok()))
    }
}

impl Stack {
    fn empty(size: f32) -> Self {
        Self {
            size,
            panes: Vec::new(),
        }
    }

    fn position(&self, panel: Panel) -> Option<usize> {
        self.panes.iter().position(|(kept, _)| *kept == panel)
    }

    /// Insert `panel` beside the pane at `index`, before or `after` it,
    /// taking half of that pane's share; the others keep theirs.
    fn split(&mut self, index: usize, panel: Panel, after: bool) {
        split_share(&mut self.panes, index, panel, after);
    }

    /// Insert `panel` at `index` with `1/(n + 1)` of the stack, where `n` is
    /// how many panes it held; the others keep their proportions.
    fn insert(&mut self, index: usize, panel: Panel) {
        insert_even(&mut self.panes, index, panel);
    }
}

/// Insert `item` beside the entry at `index`, before or `after` it, taking
/// half of that entry's share; the others keep theirs.
fn split_share<T>(entries: &mut Vec<(T, f32)>, index: usize, item: T, after: bool) {
    let Some((_, share)) = entries.get_mut(index) else {
        return;
    };
    *share /= 2.0;
    let half = *share;
    entries.insert(index + usize::from(after), (item, half));
}

/// Insert `item` at `index` with `1/(n + 1)` of the entries' total, where
/// `n` is how many there were; the others keep their proportions.
fn insert_even<T>(entries: &mut Vec<(T, f32)>, index: usize, item: T) {
    let total: f32 = entries.iter().map(|(_, share)| share).sum();
    let share = if entries.is_empty() {
        1.0
    } else {
        total / entries.len() as f32
    };
    let index = index.min(entries.len());
    entries.insert(index, (item, share));
}

/// What the layout arithmetic arranges: a panel, or, for a drop hint, the
/// place a dragged pane would take.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Tile {
    Panel(Panel),
    Ghost,
}

/// The entries along `axis` in `length` pixels: a chain of splits, each
/// cutting off one entry at its share.
fn chain<T: Copy>(entries: &[(T, f32)], axis: Axis, length: f32) -> Configuration<T> {
    fn link<T: Copy>(
        entries: &[(T, f32)],
        total: f32,
        available: f32,
        axis: Axis,
        length: f32,
    ) -> Configuration<T> {
        match entries {
            [] => unreachable!("only non-empty stacks are laid out"),
            [(item, _)] => Configuration::Pane(*item),
            [(item, share), rest @ ..] => {
                let first = share / total * available;
                Configuration::Split {
                    axis,
                    ratio: ratio(first, length),
                    a: Box::new(Configuration::Pane(*item)),
                    b: Box::new(link(
                        rest,
                        total - share,
                        available - first,
                        axis,
                        length - first - SPACING,
                    )),
                }
            }
        }
    }

    let total: f32 = entries.iter().map(|(_, share)| share).sum();
    let available = length - SPACING * (entries.len() - 1) as f32;
    link(entries, total, available, axis, length)
}

/// The pane grid's arrangement of three stacks around the map, in a grid of
/// `size`. Each stack is its entries and its size: the width of a side
/// column, or the height of the row beneath the map.
///
/// The side columns and the row get their sizes in pixels, as far as the
/// map keeps [`MIN_MAP`]; the map takes the rest.
fn arrange<T: Copy>(
    [left, middle, right]: [(&[(T, f32)], f32); 3],
    map: T,
    size: Size,
) -> Configuration<T> {
    let open = |stack: &(&[(T, f32)], f32)| !stack.0.is_empty();

    let Widths {
        left: left_width,
        middle: middle_width,
        ..
    } = widths(
        open(&left).then_some(left.1),
        open(&right).then_some(right.1),
        size.width,
    );

    let centre = if open(&middle) {
        let row = middle.1.min(max_row(size)).max(0.0);
        Configuration::Split {
            axis: Axis::Horizontal,
            ratio: ratio(size.height - row - SPACING, size.height),
            a: Box::new(Configuration::Pane(map)),
            b: Box::new(chain(middle.0, Axis::Vertical, middle_width)),
        }
    } else {
        Configuration::Pane(map)
    };

    let width = size.width
        - if open(&left) {
            left_width + SPACING
        } else {
            0.0
        };
    let centre_and_right = if open(&right) {
        Configuration::Split {
            axis: Axis::Vertical,
            ratio: ratio(middle_width, width),
            a: Box::new(centre),
            b: Box::new(chain(right.0, Axis::Horizontal, size.height)),
        }
    } else {
        centre
    };

    if open(&left) {
        Configuration::Split {
            axis: Axis::Vertical,
            ratio: ratio(left_width, size.width),
            a: Box::new(chain(left.0, Axis::Horizontal, size.height)),
            b: Box::new(centre_and_right),
        }
    } else {
        centre_and_right
    }
}

/// How wide the left column and the middle are drawn (the right column takes
/// what is left).
struct Widths {
    left: f32,
    middle: f32,
    /// The side columns were shrunk below their sizes to keep [`MIN_MAP`].
    scaled: bool,
}

/// The widths of the open side columns (`None`: closed) and the middle in a
/// grid `width` wide: the columns get their sizes, shrunk together where the
/// map would otherwise get less than [`MIN_MAP`].
fn widths(left: Option<f32>, right: Option<f32>, width: f32) -> Widths {
    let (mut left, mut right) = (left.unwrap_or(0.0), right.unwrap_or(0.0));
    let gaps = [left, right].iter().filter(|width| **width > 0.0).count() as f32 * SPACING;
    let room = (width - MIN_MAP - gaps).max(0.0);
    let scaled = left + right > room;
    if scaled {
        let scale = room / (left + right);
        left *= scale;
        right *= scale;
    }
    Widths {
        left,
        middle: width - left - right - gaps,
        scaled,
    }
}

/// The tallest the row beneath the map gets in a grid of `size`.
fn max_row(size: Size) -> f32 {
    size.height - MIN_MAP - SPACING
}

/// How many panes of at least [`MIN_PANE`] fit along `length`; at least one,
/// so an empty stack can always take a pane.
fn capacity(length: f32) -> usize {
    (((length + SPACING) / (MIN_PANE + SPACING)).floor() as usize).max(1)
}

/// The ratio of a split of `length` whose first side is `first` long: the
/// pane grid gives the first side `ratio * length - SPACING / 2`.
fn ratio(first: f32, length: f32) -> f32 {
    ((first + SPACING / 2.0) / length).clamp(0.01, 0.99)
}

/// The persisted arrangement of the panels.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Workspace {
    pub left: Stack,
    /// The row of panes beneath the map.
    pub middle: Stack,
    pub right: Stack,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            left: Stack {
                size: 360.0,
                panes: vec![(Panel::Vehicles, 1.0)],
            },
            middle: Stack::empty(260.0),
            right: Stack::empty(360.0),
        }
    }
}

impl Workspace {
    fn stack(&self, slot: Slot) -> &Stack {
        match slot {
            Slot::Left => &self.left,
            Slot::Middle => &self.middle,
            Slot::Right => &self.right,
        }
    }

    fn stack_mut(&mut self, slot: Slot) -> &mut Stack {
        match slot {
            Slot::Left => &mut self.left,
            Slot::Middle => &mut self.middle,
            Slot::Right => &mut self.right,
        }
    }

    /// Where `panel` is: its slot and its index there.
    fn find(&self, panel: Panel) -> Option<(Slot, usize)> {
        Slot::ALL
            .into_iter()
            .find_map(|slot| Some((slot, self.stack(slot).position(panel)?)))
    }

    fn contains(&self, panel: Panel) -> bool {
        panel == Panel::Map || self.find(panel).is_some()
    }

    fn remove(&mut self, panel: Panel) -> bool {
        match self.find(panel) {
            Some((slot, index)) => {
                self.stack_mut(slot).panes.remove(index);
                true
            }
            None => false,
        }
    }

    /// Move `panel` to `destination`, or open it there if it is closed.
    /// Returns whether anything changed.
    fn place(&mut self, panel: Panel, destination: Destination) -> bool {
        if panel.is_fixed() {
            return false;
        }

        if let Destination::Swap(other) = destination {
            let (Some((slot, index)), Some((other_slot, other_index))) =
                (self.find(panel), self.find(other))
            else {
                return false;
            };
            if panel == other {
                return false;
            }
            // Each takes the other's place, and its size there.
            self.stack_mut(slot).panes[index].0 = other;
            self.stack_mut(other_slot).panes[other_index].0 = panel;
            return true;
        }

        let neighbour = match destination {
            Destination::Split { neighbour, .. } | Destination::Between(neighbour) => {
                Some(neighbour)
            }
            Destination::End(_) | Destination::Swap(_) => None,
        };
        if neighbour == Some(panel) {
            return false;
        }
        // An open panel moves; a closed one opens.
        self.remove(panel);

        match destination {
            Destination::End(slot) => {
                let stack = self.stack_mut(slot);
                stack.insert(stack.panes.len(), panel);
            }
            Destination::Between(neighbour) | Destination::Split { neighbour, .. } => {
                let Some((slot, index)) = self.find(neighbour) else {
                    return false;
                };
                let stack = self.stack_mut(slot);
                match destination {
                    Destination::Split { after, .. } => stack.split(index, panel, after),
                    _ => stack.insert(index + 1, panel),
                }
            }
            Destination::Swap(_) => unreachable!("handled above"),
        }
        true
    }

    /// A workspace as loaded from disk, made sound: the map and duplicate
    /// panels are dropped, nonsense shares become even ones, and sizes are
    /// kept above the smallest pane.
    fn sanitized(mut self) -> Self {
        let mut seen = Vec::new();
        for slot in Slot::ALL {
            let stack = self.stack_mut(slot);
            stack.panes.retain(|(panel, _)| {
                let fresh = *panel != Panel::Map && !seen.contains(panel);
                seen.push(*panel);
                fresh
            });
            for (_, share) in &mut stack.panes {
                if !share.is_finite() || *share <= 0.0 {
                    *share = 1.0;
                }
            }
            if !stack.size.is_finite() {
                stack.size = DEFAULT_STACK_SIZE;
            }
            stack.size = stack.size.max(MIN_PANE);
        }
        self
    }

    /// How the side columns and the middle are drawn in a grid of `size`.
    fn widths(&self, size: Size) -> Widths {
        let open = |stack: &Stack| (!stack.panes.is_empty()).then_some(stack.size);
        widths(open(&self.left), open(&self.right), size.width)
    }

    /// The length each stack lays its panes along, in a grid of `size`: a
    /// column's height, the row's width.
    fn length(&self, slot: Slot, size: Size) -> f32 {
        match slot {
            Slot::Left | Slot::Right => size.height,
            Slot::Middle => self.widths(size).middle,
        }
    }

    /// Whether `after`, a rearrangement of this workspace, puts no more panes
    /// in a stack than fit at [`MIN_PANE`] in a grid of `size`. A stack that
    /// holds more already (the window shrank) keeps them, but takes no more.
    fn room_for(&self, after: &Workspace, size: Size) -> bool {
        Slot::ALL.into_iter().all(|slot| {
            let (before, now) = (self.stack(slot).panes.len(), after.stack(slot).panes.len());
            now <= before.max(capacity(after.length(slot, size)))
        })
    }

    /// The pane grid's arrangement in a grid of `size`; see [`arrange`].
    fn configuration(&self, size: Size) -> Configuration<Panel> {
        arrange(
            [
                (&self.left.panes, self.left.size),
                (&self.middle.panes, self.middle.size),
                (&self.right.panes, self.right.size),
            ],
            Panel::Map,
            size,
        )
    }

    /// Apply the operator's drag of `seam`, given where the grid draws each
    /// panel with that divider where it was dragged, in a grid of `size`.
    ///
    /// Only what the divider bounds changes, so a size [`arrange`] squeezed
    /// to keep the map (the columns in a narrow window, the row in a short
    /// one) stays as the operator chose it unless its own divider is
    /// dragged. What is stored is what [`arrange`] will draw: the map keeps
    /// [`MIN_MAP`], so the divider stops where the map would get less.
    fn resize(&mut self, seam: Seam, regions: &BTreeMap<Panel, Rectangle>, size: Size) {
        let first = |stack: &Stack| regions.get(&stack.panes.first()?.0).copied();

        match seam {
            Seam::Row => {
                if let Some(row) = first(&self.middle) {
                    self.middle.size = row.height.min(max_row(size)).max(MIN_PANE);
                }
            }
            Seam::Column(slot) => {
                let other = match slot {
                    Slot::Left => Slot::Right,
                    Slot::Middle | Slot::Right => Slot::Left,
                };
                let Some(column) = first(self.stack(slot)) else {
                    return;
                };
                let other_width = first(self.stack(other)).map_or(0.0, |region| region.width);
                // Columns drawn narrower than their sizes shrink together.
                // The other column is kept at the width it is drawn, so only
                // the dragged divider moves on screen.
                if self.widths(size).scaled && !self.stack(other).panes.is_empty() {
                    self.stack_mut(other).size = other_width;
                }
                let gaps = [slot, other]
                    .into_iter()
                    .filter(|slot| !self.stack(*slot).panes.is_empty())
                    .count() as f32
                    * SPACING;
                let room = size.width - MIN_MAP - gaps - other_width;
                self.stack_mut(slot).size = column.width.min(room).max(MIN_PANE);
            }
            Seam::Divider { slot, index } => {
                let along = |region: &Rectangle| match slot {
                    Slot::Middle => region.width,
                    Slot::Left | Slot::Right => region.height,
                };
                let stack = self.stack_mut(slot);
                let length = |index: usize| {
                    let (panel, _) = stack.panes.get(index)?;
                    regions.get(panel).map(along)
                };
                let (Some(before), Some(after)) = (length(index), length(index + 1)) else {
                    return;
                };
                if before + after <= 0.0 {
                    return;
                }
                // The two trade what they hold between them; every other
                // divider stays where it is.
                let total = stack.panes[index].1 + stack.panes[index + 1].1;
                let share = total * before / (before + after);
                stack.panes[index].1 = share;
                stack.panes[index + 1].1 = total - share;
            }
        }
    }
}

/// A divider of the arrangement, named by what it bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Seam {
    /// The inner edge of a side column: dragging it sets the column's width.
    Column(Slot),
    /// The top of the row beneath the map: dragging it sets the row's
    /// height.
    Row,
    /// Between the pane at `index` of a stack and the next: dragging it
    /// trades their shares.
    Divider { slot: Slot, index: usize },
}

/// Where a dragged panel goes, named by its neighbour rather than an index,
/// so removing it from its old place cannot shift the new one.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Destination {
    /// Beside a pane, before or `after` it, splitting that pane's share.
    Split { neighbour: Panel, after: bool },
    /// On the divider after a pane, taking an even share of its stack.
    Between(Panel),
    /// At the end of a stack, taking an even share.
    End(Slot),
    /// In a pane's place, which takes the dragged pane's place in turn.
    Swap(Panel),
}

#[derive(Debug, Clone)]
pub enum Message {
    Clicked(pane_grid::Pane),
    Dragged(pane_grid::DragEvent),
    /// A panel dragged from outside the grid (a rail button) was dropped on
    /// `Target`: it opens there, or moves there if it is open.
    Placed(Panel, Target),
    Resized(pane_grid::ResizeEvent),
    /// Open the panel at the end of `slot`, or close it if it is open.
    Toggle(Panel, Slot),
    Close(pane_grid::Pane),
    Maximize(pane_grid::Pane),
    Restore,
    /// The grid was laid out at this size (when first shown, and on every
    /// change of the window's size).
    Sized(Size),
}

impl Message {
    /// Whether this message can change the persisted arrangement.
    ///
    /// Maximization is view state the [`Workspace`] does not record, and a
    /// new grid size changes how the workspace is laid out but not the
    /// workspace itself, so neither triggers a save.
    pub fn changes_layout(&self) -> bool {
        matches!(
            self,
            Message::Dragged(pane_grid::DragEvent::Dropped { .. })
                | Message::Placed(..)
                | Message::Resized(_)
                | Message::Toggle(..)
                | Message::Close(_)
        )
    }
}

pub struct Layout {
    workspace: Workspace,
    panes: pane_grid::State<Panel>,
    /// The grid's size, once measured.
    size: Option<Size>,
    focus: Option<pane_grid::Pane>,
    dirty: bool,
    revision: u64,
}

impl Layout {
    pub fn new() -> Self {
        Self::from_workspace(Workspace::default())
    }

    /// The layout of a persisted workspace.
    pub fn from_workspace(workspace: Workspace) -> Self {
        let mut layout = Self {
            workspace: workspace.sanitized(),
            panes: pane_grid::State::new(Panel::Map).0,
            size: None,
            focus: None,
            dirty: false,
            revision: 0,
        };
        layout.arrange();
        layout
    }

    pub fn panes(&self) -> &pane_grid::State<Panel> {
        &self.panes
    }

    /// The arrangement, for persistence.
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// The arrangement revision, bumped on every change that can affect the
    /// persisted arrangement. Part of the debounce subscription's identity, so
    /// the timer restarts on each change rather than firing mid-drag.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether the arrangement has changed since the last persistence.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Clear the dirty flag once the arrangement has been persisted.
    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    pub fn is_focused(&self, pane: pane_grid::Pane) -> bool {
        self.focus == Some(pane)
    }

    /// Whether a panel is currently present in the grid.
    pub fn is_open(&self, panel: Panel) -> bool {
        self.workspace.contains(panel)
    }

    fn pane_of(&self, panel: Panel) -> Option<pane_grid::Pane> {
        self.panes
            .iter()
            .find(|(_, p)| **p == panel)
            .map(|(pane, _)| *pane)
    }

    /// Apply a layout message, reporting whether it may have changed the
    /// persisted arrangement.
    ///
    /// The dirty flag and revision are maintained here, so the root never
    /// writes layout bookkeeping directly. The root keeps only the debounce
    /// policy: when to snapshot and persist a dirty arrangement.
    pub fn update(&mut self, message: Message) -> bool {
        let changed = message.changes_layout();
        self.apply(message);

        if changed {
            self.dirty = true;
            self.revision = self.revision.wrapping_add(1);
        }

        changed
    }

    fn apply(&mut self, message: Message) {
        match message {
            Message::Clicked(pane) => self.focus = Some(pane),
            Message::Dragged(pane_grid::DragEvent::Dropped { pane, target }) => {
                if let Some(panel) = self.panes.get(pane).copied()
                    && let Some(destination) = self.destination(target)
                {
                    self.move_to(panel, destination);
                }
            }
            Message::Dragged(_) => {}
            Message::Placed(panel, target) => {
                if let Some(destination) = self.destination(target) {
                    self.move_to(panel, destination);
                }
            }
            Message::Resized(event) => {
                // The workspace takes what the drag asks of the divider's
                // seam, and the grid is laid out from the workspace. The
                // rearrangement keeps the split, so the drag goes on.
                let Some(seam) = self.seam(event.split) else {
                    return;
                };
                let size = self.size.unwrap_or(ASSUMED_SIZE);
                let mut dragged = self.panes.layout().clone();
                dragged.resize(event.split, event.ratio);
                let regions = dragged
                    .pane_regions(SPACING, MIN_PANE, size)
                    .into_iter()
                    .filter_map(|(pane, region)| Some((*self.panes.get(pane)?, region)))
                    .collect();
                self.workspace.resize(seam, &regions, size);
                self.arrange();
            }
            Message::Toggle(panel, slot) => {
                if self.is_open(panel) {
                    self.close(panel);
                } else {
                    self.open(panel, slot);
                }
            }
            Message::Close(pane) => {
                if let Some(panel) = self.panes.get(pane).copied() {
                    self.close(panel);
                }
            }
            Message::Maximize(pane) => self.panes.maximize(pane),
            Message::Restore => self.panes.restore(),
            Message::Sized(size) => {
                if self.size != Some(size) {
                    self.size = Some(size);
                    self.arrange();
                }
            }
        }
    }

    /// What `split` bounds, by the panels on either side of it: the last
    /// before it and the first after it.
    fn seam(&self, split: pane_grid::Split) -> Option<Seam> {
        let (before, after) = self.panes.neighbours(split)?;
        // `None` for the map, the one panel in no stack.
        let (before, after) = (self.workspace.find(*before), self.workspace.find(*after));
        let slot = |place: Option<(Slot, usize)>| place.map(|(slot, _)| slot);

        Some(match (slot(before), slot(after)) {
            (Some(Slot::Left), after) if after != Some(Slot::Left) => Seam::Column(Slot::Left),
            (before, Some(Slot::Right)) if before != Some(Slot::Right) => Seam::Column(Slot::Right),
            (None, Some(Slot::Middle)) => Seam::Row,
            (Some(slot), Some(next)) if slot == next => Seam::Divider {
                slot,
                index: before?.1,
            },
            _ => return None,
        })
    }

    /// Lay the grid out from the workspace, at the grid's size.
    fn arrange(&mut self) {
        let size = self.size.unwrap_or(ASSUMED_SIZE);
        self.panes.rearrange(self.workspace.configuration(size));
        if let Some(focus) = self.focus
            && self.panes.get(focus).is_none()
        {
            self.focus = None;
        }
    }

    /// Open `panel` at the end of `slot`, or, if that stack is full, of the
    /// first other with room (the right column, the left, the row). Nothing
    /// opens where no stack has room ([`Layout::has_room`]).
    fn open(&mut self, panel: Panel, slot: Slot) {
        if panel.is_fixed() || self.is_open(panel) {
            return;
        }
        let Some(opened) = self.opened(panel, slot) else {
            return;
        };
        self.workspace = opened;
        self.arrange();
        self.focus = self.pane_of(panel);
    }

    /// The workspace with `panel` opened at the end of `slot` or the first
    /// other stack with room for it.
    fn opened(&self, panel: Panel, slot: Slot) -> Option<Workspace> {
        let size = self.size.unwrap_or(ASSUMED_SIZE);
        [slot, Slot::Right, Slot::Left, Slot::Middle]
            .into_iter()
            .find_map(|slot| {
                let mut opened = self.workspace.clone();
                let stack = opened.stack_mut(slot);
                stack.insert(stack.panes.len(), panel);
                self.workspace.room_for(&opened, size).then_some(opened)
            })
    }

    /// Whether `panel` is open, or some stack has room to open it in.
    pub fn has_room(&self, panel: Panel) -> bool {
        self.is_open(panel) || panel.is_fixed() || self.opened(panel, Slot::Left).is_some()
    }

    /// Close `panel`, unless it is fixed.
    fn close(&mut self, panel: Panel) {
        if !panel.is_fixed() && self.workspace.remove(panel) {
            self.arrange();
        }
    }

    /// Where a pane dropped on `target` goes; see [`Layout::drop_hint`].
    fn destination(&self, target: Target) -> Option<Destination> {
        Some(match target {
            // The grid's own edges are not drop targets.
            Target::Edge(_) => return None,
            Target::Between(before, _) => Destination::Between(*self.panes.get(before)?),
            Target::Pane(pane, region) => match (*self.panes.get(pane)?, region) {
                (Panel::Map, Region::Edge(Edge::Left)) => Destination::End(Slot::Left),
                (Panel::Map, Region::Edge(Edge::Right)) => Destination::End(Slot::Right),
                (Panel::Map, _) => Destination::End(Slot::Middle),
                (other, Region::Center) => Destination::Swap(other),
                (neighbour, Region::Edge(edge)) => Destination::Split {
                    neighbour,
                    after: matches!(edge, Edge::Bottom | Edge::Right),
                },
            },
        })
    }

    /// Where a pane being dragged would land with the cursor at `cursor`,
    /// given every pane's region, and the area that shows it, drawn against
    /// the layout on screen (which still holds the pane where it was during
    /// the drag): the other pane's place for a swap, half of it (less half
    /// a gap) for a split, the size the pane will have centred on the
    /// divider for a drop on one, and for a new stack where the pane would
    /// be laid out if it were added there.
    ///
    /// - On a divider between two panes of a stack (within [`SEAM_ZONE`]
    ///   of it): between them, taking an even share of the stack.
    /// - In the middle of a pane ([`SWAP_ZONE`]): in its place, the two
    ///   swapping.
    /// - On the rest of a pane, by the half (the top or bottom half in a
    ///   column, the left or right half in the row): beside it on that
    ///   side, splitting that pane's share in two.
    /// - On the map's left or right quarter, while that side column is
    ///   empty: in it. Anywhere else on the map, while the row is empty: in
    ///   it. A stack with panes is joined beside them instead.
    ///
    /// Nothing else is a target: not the grid's edges, the gaps between
    /// columns, the dragged pane itself, or a drop that would change
    /// nothing.
    pub fn drop_hint(
        &self,
        dragged: pane_grid::Pane,
        cursor: Point,
        regions: &BTreeMap<pane_grid::Pane, Rectangle>,
    ) -> Option<DropHint> {
        self.hint_for(*self.panes.get(dragged)?, cursor, regions)
    }

    /// Where `panel`, dragged onto the grid from its rail button, would
    /// land; see [`Layout::drop_hint`]. An open panel moves as if its pane
    /// were dragged. A closed one opens, and has no pane to swap, so the
    /// middle of a pane splits it like the rest of its half.
    pub fn incoming_hint(
        &self,
        panel: Panel,
        cursor: Point,
        regions: &BTreeMap<pane_grid::Pane, Rectangle>,
    ) -> Option<DropHint> {
        self.hint_for(panel, cursor, regions)
    }

    fn hint_for(
        &self,
        dragging: Panel,
        cursor: Point,
        regions: &BTreeMap<pane_grid::Pane, Rectangle>,
    ) -> Option<DropHint> {
        let target = self.drop_target_at(dragging, cursor, regions)?;
        let destination = self.destination(target)?;
        let mut moved = self.workspace.clone();
        if !moved.place(dragging, destination)
            || !self
                .workspace
                .room_for(&moved, self.size.unwrap_or(ASSUMED_SIZE))
        {
            return None;
        }

        // A swap takes the other pane's place as it is.
        if let Destination::Swap(other) = destination {
            let area = *regions.get(&self.pane_of(other)?)?;
            return Some(DropHint { target, area });
        }

        // A split halves the other pane as it is drawn: the half on the
        // drop's side, less half the gap the new divider takes.
        if let Destination::Split { neighbour, after } = destination {
            let region = *regions.get(&self.pane_of(neighbour)?)?;
            let (slot, _) = self.workspace.find(neighbour)?;
            let area = if slot == Slot::Middle {
                let half = (region.width - SPACING) / 2.0;
                Rectangle {
                    x: if after {
                        region.x + region.width - half
                    } else {
                        region.x
                    },
                    width: half,
                    ..region
                }
            } else {
                let half = (region.height - SPACING) / 2.0;
                Rectangle {
                    y: if after {
                        region.y + region.height - half
                    } else {
                        region.y
                    },
                    height: half,
                    ..region
                }
            };
            return Some(DropHint { target, area });
        }

        let size = self.size.unwrap_or(ASSUMED_SIZE);
        let origin = regions.values().fold(
            Point::new(f32::INFINITY, f32::INFINITY),
            |origin, region| Point::new(origin.x.min(region.x), origin.y.min(region.y)),
        );
        let offset = iced::Vector::new(origin.x, origin.y);

        // On a divider: the size the pane will have, centred on the divider
        // as it is drawn. Making room for it reflows the whole stack, which
        // the screen does not show during the drag, so a reflowed place would
        // not line up with the panes beside it.
        if let (Destination::Between(neighbour), Target::Between(before, after)) =
            (destination, target)
        {
            let (slot, _) = self.workspace.find(neighbour)?;
            let mut moved = self.workspace.clone();
            moved.place(dragging, destination);
            let laid_out = pane_grid::State::with_configuration(moved.configuration(size));
            let landed = laid_out
                .layout()
                .pane_regions(SPACING, MIN_PANE, size)
                .into_iter()
                .find(|(pane, _)| laid_out.get(*pane) == Some(&dragging))
                .map(|(_, region)| region + offset)?;

            let (before, after) = (*regions.get(&before)?, *regions.get(&after)?);
            let stack: Vec<Rectangle> = self
                .workspace
                .stack(slot)
                .panes
                .iter()
                .filter_map(|(panel, _)| regions.get(&self.pane_of(*panel)?).copied())
                .collect();
            let area = if slot == Slot::Middle {
                let seam = (before.x + before.width + after.x) / 2.0;
                let start = stack.iter().map(|r| r.x).fold(f32::INFINITY, f32::min);
                let end = stack.iter().map(|r| r.x + r.width).fold(0.0, f32::max);
                let x = (seam - landed.width / 2.0).clamp(start, (end - landed.width).max(start));
                Rectangle {
                    x,
                    width: landed.width,
                    ..before
                }
            } else {
                let seam = (before.y + before.height + after.y) / 2.0;
                let start = stack.iter().map(|r| r.y).fold(f32::INFINITY, f32::min);
                let end = stack.iter().map(|r| r.y + r.height).fold(0.0, f32::max);
                let y = (seam - landed.height / 2.0).clamp(start, (end - landed.height).max(start));
                Rectangle {
                    y,
                    height: landed.height,
                    ..before
                }
            };
            return Some(DropHint { target, area });
        }

        // Opening an empty stack: the place the pane would take in the
        // layout on screen, which still has the pane where it was. A ghost
        // opens the stack, which takes its room from the map as it will; the
        // rest stays put, as the grid shows it.
        let workspace = &self.workspace;
        let tiles = |stack: &Stack| -> Vec<(Tile, f32)> {
            stack
                .panes
                .iter()
                .map(|(panel, share)| (Tile::Panel(*panel), *share))
                .collect()
        };
        let mut stacks = [
            tiles(&workspace.left),
            tiles(&workspace.middle),
            tiles(&workspace.right),
        ];
        let index_of = |slot| Slot::ALL.iter().position(|kept| *kept == slot);
        match destination {
            Destination::End(slot) => {
                let stack = &mut stacks[index_of(slot)?];
                insert_even(stack, stack.len(), Tile::Ghost);
            }
            Destination::Split { .. } | Destination::Between(_) | Destination::Swap(_) => {
                unreachable!("handled above")
            }
        }

        // Laid out at the grid's size, in the grid's place: its top-left
        // pane starts at the grid's origin.
        let [left, middle, right] = &stacks;
        let laid_out = pane_grid::State::with_configuration(arrange(
            [
                (left, workspace.left.size),
                (middle, workspace.middle.size),
                (right, workspace.right.size),
            ],
            Tile::Panel(Panel::Map),
            size,
        ));
        let area = laid_out
            .layout()
            .pane_regions(SPACING, MIN_PANE, size)
            .into_iter()
            .find(|(pane, _)| laid_out.get(*pane) == Some(&Tile::Ghost))
            .map(|(_, region)| region + offset)?;

        Some(DropHint { target, area })
    }

    /// The grid's drop target under `cursor`; see [`Layout::drop_hint`].
    fn drop_target_at(
        &self,
        dragging: Panel,
        cursor: Point,
        regions: &BTreeMap<pane_grid::Pane, Rectangle>,
    ) -> Option<Target> {
        if dragging.is_fixed() {
            return None;
        }
        let pane_of = |panel| {
            self.pane_of(panel)
                .filter(|pane| regions.contains_key(pane))
        };

        // The dividers, except those beside the dragged pane: dropping it
        // there would leave it where it is.
        for slot in Slot::ALL {
            for pair in self.workspace.stack(slot).panes.windows(2) {
                let ((a, _), (b, _)) = (pair[0], pair[1]);
                if a == dragging || b == dragging {
                    continue;
                }
                let (Some(pane_a), Some(pane_b)) = (pane_of(a), pane_of(b)) else {
                    continue;
                };
                let (first, second) = (regions[&pane_a], regions[&pane_b]);
                let on = if slot == Slot::Middle {
                    let seam = (first.x + first.width + second.x) / 2.0;
                    (cursor.x - seam).abs() <= SEAM_ZONE
                        && (first.y..=first.y + first.height).contains(&cursor.y)
                } else {
                    let seam = (first.y + first.height + second.y) / 2.0;
                    (cursor.y - seam).abs() <= SEAM_ZONE
                        && (first.x..=first.x + first.width).contains(&cursor.x)
                };
                if on {
                    return Some(Target::Between(pane_a, pane_b));
                }
            }
        }

        let (pane, region) = regions
            .iter()
            .find(|(_, region)| region.contains(cursor))
            .map(|(pane, region)| (*pane, *region))?;
        if self.pane_of(dragging) == Some(pane) {
            return None;
        }
        let relative = Point::new(
            (cursor.x - region.x) / region.width,
            (cursor.y - region.y) / region.height,
        );

        let region = if *self.panes.get(pane)? == Panel::Map {
            // The map opens a stack only while it is empty: a stack with
            // panes is joined beside them. (A stack holding just the dragged
            // pane is not empty either: dropping it there again would change
            // nothing.)
            let empty = |slot| self.workspace.stack(slot).panes.is_empty();
            if relative.x < MAP_SIDE_ZONE && empty(Slot::Left) {
                Region::Edge(Edge::Left)
            } else if relative.x > 1.0 - MAP_SIDE_ZONE && empty(Slot::Right) {
                Region::Edge(Edge::Right)
            } else if empty(Slot::Middle) {
                Region::Edge(Edge::Bottom)
            } else {
                return None;
            }
        } else {
            let middle = (0.5 - SWAP_ZONE / 2.0)..=(0.5 + SWAP_ZONE / 2.0);
            let (slot, _) = self.workspace.find(*self.panes.get(pane)?)?;
            if self.is_open(dragging)
                && middle.contains(&relative.x)
                && middle.contains(&relative.y)
            {
                Region::Center
            } else if slot == Slot::Middle {
                Region::Edge(if relative.x < 0.5 {
                    Edge::Left
                } else {
                    Edge::Right
                })
            } else {
                Region::Edge(if relative.y < 0.5 {
                    Edge::Top
                } else {
                    Edge::Bottom
                })
            }
        };

        Some(Target::Pane(pane, region))
    }

    /// Move `panel` to `destination`, if the stack it joins has room.
    fn move_to(&mut self, panel: Panel, destination: Destination) {
        let mut moved = self.workspace.clone();
        let size = self.size.unwrap_or(ASSUMED_SIZE);
        if moved.place(panel, destination) && self.workspace.room_for(&moved, size) {
            self.workspace = moved;
            self.arrange();
            self.focus = self.pane_of(panel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Size = Size::new(1600.0, 900.0);

    /// A layout measured at `size`.
    fn layout_at(workspace: Workspace, size: Size) -> Layout {
        let mut layout = Layout::from_workspace(workspace);
        layout.update(Message::Sized(size));
        layout
    }

    /// Where each panel is drawn, at the layout's size.
    fn regions(layout: &Layout) -> BTreeMap<Panel, Rectangle> {
        layout
            .panes
            .layout()
            .pane_regions(SPACING, MIN_PANE, layout.size.unwrap())
            .into_iter()
            .map(|(pane, region)| (*layout.panes.get(pane).unwrap(), region))
            .collect()
    }

    /// Drag the divider at `seam` so the side before it is `first` long:
    /// wide across a vertical divider, tall across a horizontal one.
    fn drag(layout: &mut Layout, seam: Seam, first: f32) {
        let (split, (axis, region, _)) = layout
            .panes
            .layout()
            .split_regions(SPACING, MIN_PANE, layout.size.unwrap())
            .into_iter()
            .find(|(split, _)| layout.seam(*split) == Some(seam))
            .expect("the seam is in the grid");
        let length = match axis {
            Axis::Horizontal => region.height,
            Axis::Vertical => region.width,
        };
        layout.update(Message::Resized(pane_grid::ResizeEvent {
            split,
            ratio: ratio(first, length),
        }));
    }

    fn panels(stack: &Stack) -> Vec<Panel> {
        stack.panes.iter().map(|(panel, _)| *panel).collect()
    }

    fn shares(stack: &Stack) -> Vec<f32> {
        let total: f32 = stack.panes.iter().map(|(_, share)| share).sum();
        stack.panes.iter().map(|(_, share)| share / total).collect()
    }

    fn close_to(a: f32, b: f32) -> bool {
        (a - b).abs() < 1.5
    }

    /// A grid short enough that a column holds three panes of [`MIN_PANE`].
    const SHORT: Size = Size::new(1200.0, 400.0);

    fn stack_of(panels: &[Panel]) -> Stack {
        Stack {
            size: 300.0,
            panes: panels.iter().map(|panel| (*panel, 1.0)).collect(),
        }
    }

    #[test]
    fn crowded_regions_are_never_negative() {
        let workspace = Workspace {
            left: stack_of(&[
                Panel::Vehicles,
                Panel::Mission,
                Panel::Parameters,
                Panel::Calibration,
                Panel::Links,
                Panel::Logs,
            ]),
            ..Workspace::default()
        };
        let layout = layout_at(workspace, SHORT);

        for (panel, region) in regions(&layout) {
            assert!(
                region.width >= 0.0 && region.height >= 0.0,
                "{panel:?}: {region:?}"
            );
            assert!(
                region.y + region.height <= SHORT.height + 0.5,
                "{panel:?}: {region:?}"
            );
        }
    }

    #[test]
    fn a_full_stack_takes_no_more_panes() {
        let full = [Panel::Vehicles, Panel::Mission, Panel::Parameters];
        assert_eq!(capacity(SHORT.height), full.len());
        let layout = layout_at(
            Workspace {
                left: stack_of(&full),
                ..Workspace::default()
            },
            SHORT,
        );

        // Over the lower half of the bottom pane, a drop would split it.
        let regions: BTreeMap<_, _> = layout
            .panes
            .layout()
            .pane_regions(SPACING, MIN_PANE, SHORT)
            .into_iter()
            .collect();
        let bottom = regions[&layout.pane_of(Panel::Parameters).unwrap()];
        let cursor = Point::new(
            bottom.x + bottom.width / 2.0,
            bottom.y + bottom.height * 0.9,
        );
        assert!(
            layout
                .incoming_hint(Panel::Logs, cursor, &regions)
                .is_none()
        );

        // A pane already in the stack still moves within it.
        let top = regions[&layout.pane_of(Panel::Vehicles).unwrap()];
        let swap = Point::new(top.x + top.width / 2.0, top.y + top.height / 2.0);
        assert!(
            layout
                .drop_hint(layout.pane_of(Panel::Mission).unwrap(), swap, &regions)
                .is_some()
        );
    }

    #[test]
    fn a_rail_click_into_a_full_stack_opens_in_the_next_with_room() {
        let mut layout = layout_at(
            Workspace {
                left: stack_of(&[Panel::Vehicles, Panel::Mission, Panel::Parameters]),
                ..Workspace::default()
            },
            SHORT,
        );
        assert!(layout.has_room(Panel::Logs));

        layout.update(Message::Toggle(Panel::Logs, Slot::Left));

        assert_eq!(layout.workspace.left.panes.len(), 3, "still full");
        assert_eq!(panels(&layout.workspace.right), [Panel::Logs]);
    }

    #[test]
    fn a_column_dragged_in_a_narrow_window_leaves_the_other_where_it_is() {
        let size = Size::new(700.0, 900.0);
        let mut layout = layout_at(
            Workspace {
                left: stack_of(&[Panel::Vehicles]),
                right: stack_of(&[Panel::Settings]),
                ..Workspace::default()
            },
            size,
        );
        assert!(layout.workspace.widths(size).scaled);
        let right = regions(&layout)[&Panel::Settings];

        drag(&mut layout, Seam::Column(Slot::Left), 200.0);

        let regions = regions(&layout);
        assert!(close_to(regions[&Panel::Vehicles].width, 200.0));
        assert!(
            close_to(regions[&Panel::Settings].x, right.x)
                && close_to(regions[&Panel::Settings].width, right.width),
            "the other column stays put: {right:?} became {:?}",
            regions[&Panel::Settings]
        );
        assert!(close_to(layout.workspace.left.size, 200.0));
        assert!(close_to(layout.workspace.right.size, right.width));
    }

    #[test]
    fn the_row_keeps_the_height_it_was_last_dragged_to() {
        let mut layout = layout_at(
            Workspace {
                middle: stack_of(&[Panel::Logs]),
                ..Workspace::default()
            },
            SIZE,
        );

        // Up until the map would be shorter than it gets: the row stops
        // where the map keeps its least.
        drag(&mut layout, Seam::Row, MIN_PANE);
        assert!(close_to(layout.workspace.middle.size, max_row(SIZE)));
        assert!(close_to(regions(&layout)[&Panel::Map].height, MIN_MAP));

        // Then down again: the row is what was dragged last.
        drag(&mut layout, Seam::Row, SIZE.height - 300.0 - SPACING);
        assert!(close_to(layout.workspace.middle.size, 300.0));

        // Rearranging, or emptying the row and filling it again, keeps it.
        layout.update(Message::Toggle(Panel::Mission, Slot::Left));
        assert!(close_to(regions(&layout)[&Panel::Logs].height, 300.0));
        layout.update(Message::Toggle(Panel::Logs, Slot::Middle));
        layout.update(Message::Toggle(Panel::Links, Slot::Middle));
        assert!(close_to(regions(&layout)[&Panel::Links].height, 300.0));
    }

    #[test]
    fn a_column_keeps_the_width_it_was_last_dragged_to() {
        let mut layout = layout_at(Workspace::default(), SIZE);

        // Over the map, which keeps its least.
        drag(&mut layout, Seam::Column(Slot::Left), SIZE.width - MIN_PANE);
        assert!(close_to(regions(&layout)[&Panel::Map].width, MIN_MAP));

        drag(&mut layout, Seam::Column(Slot::Left), 500.0);
        assert!(close_to(layout.workspace.left.size, 500.0));
        layout.update(Message::Toggle(Panel::Mission, Slot::Left));
        assert!(close_to(regions(&layout)[&Panel::Vehicles].width, 500.0));
    }

    #[test]
    fn a_drag_keeps_what_a_short_window_squeezes_elsewhere() {
        let short = Size::new(1600.0, 600.0);
        let mut layout = layout_at(
            Workspace {
                left: stack_of(&[Panel::Vehicles, Panel::Mission]),
                middle: Stack {
                    size: 500.0,
                    panes: vec![(Panel::Logs, 1.0)],
                },
                ..Workspace::default()
            },
            short,
        );
        assert!(
            regions(&layout)[&Panel::Logs].height < 500.0,
            "the row is squeezed"
        );

        drag(
            &mut layout,
            Seam::Divider {
                slot: Slot::Left,
                index: 0,
            },
            200.0,
        );
        drag(&mut layout, Seam::Column(Slot::Left), 400.0);

        assert_eq!(layout.workspace.middle.size, 500.0, "not the row's divider");
        layout.update(Message::Sized(SIZE));
        assert!(close_to(regions(&layout)[&Panel::Logs].height, 500.0));
    }

    #[test]
    fn a_divider_in_a_stack_trades_between_its_two_panes() {
        let mut layout = layout_at(
            Workspace {
                left: stack_of(&[Panel::Vehicles, Panel::Mission, Panel::Parameters]),
                ..Workspace::default()
            },
            SIZE,
        );
        let before = regions(&layout);
        let height = |regions: &BTreeMap<Panel, Rectangle>, panel| regions[&panel].height;

        drag(
            &mut layout,
            Seam::Divider {
                slot: Slot::Left,
                index: 1,
            },
            200.0,
        );

        let after = regions(&layout);
        assert!(close_to(height(&after, Panel::Mission), 200.0));
        assert!(close_to(
            height(&after, Panel::Vehicles),
            height(&before, Panel::Vehicles)
        ));
        assert!(close_to(
            after[&Panel::Parameters].y + height(&after, Panel::Parameters),
            before[&Panel::Parameters].y + height(&before, Panel::Parameters)
        ));
    }

    #[test]
    fn a_new_pane_takes_its_share_and_the_others_keep_their_proportions() {
        let mut layout = layout_at(Workspace::default(), SIZE);
        layout.workspace.left.panes = vec![(Panel::Vehicles, 3.0), (Panel::Mission, 1.0)];
        layout.arrange();

        layout.update(Message::Toggle(Panel::Logs, Slot::Left));

        assert_eq!(
            panels(&layout.workspace.left),
            [Panel::Vehicles, Panel::Mission, Panel::Logs],
            "at the bottom"
        );
        let shares = shares(&layout.workspace.left);
        assert!((shares[2] - 1.0 / 3.0).abs() < 1e-6, "{shares:?}");
        assert!(
            (shares[0] / shares[1] - 3.0).abs() < 1e-6,
            "proportions kept"
        );

        // The grid draws those shares.
        let regions = regions(&layout);
        let height = |panel| regions[&panel].height;
        let column = SIZE.height - 2.0 * SPACING;
        assert!(close_to(height(Panel::Logs), column / 3.0));
        assert!(close_to(height(Panel::Vehicles), column * 0.5));
    }

    #[test]
    fn the_row_beneath_the_map_tiles_sideways() {
        let mut layout = layout_at(Workspace::default(), SIZE);
        layout.update(Message::Toggle(Panel::Logs, Slot::Middle));
        layout.update(Message::Toggle(Panel::Links, Slot::Middle));

        let regions = regions(&layout);
        let (map, logs, links) = (
            regions[&Panel::Map],
            regions[&Panel::Logs],
            regions[&Panel::Links],
        );
        assert!(logs.y > map.y + map.height, "beneath the map");
        assert_eq!(logs.y, links.y, "side by side");
        assert!(links.x > logs.x, "new panes on the right");
        assert!(close_to(logs.width, links.width));
        assert!(close_to(logs.height, Workspace::default().middle.size));
    }

    #[test]
    fn side_columns_keep_their_pixels_when_the_window_is_resized() {
        let mut layout = layout_at(Workspace::default(), SIZE);
        layout.update(Message::Toggle(Panel::Settings, Slot::Right));

        for width in [1600.0, 1200.0, 2400.0] {
            layout.update(Message::Sized(Size::new(width, 900.0)));
            let regions = regions(&layout);
            assert!(close_to(regions[&Panel::Vehicles].width, 360.0), "{width}");
            assert!(close_to(regions[&Panel::Settings].width, 360.0), "{width}");
            assert!(
                close_to(regions[&Panel::Map].width, width - 720.0 - 2.0 * SPACING),
                "the map gives the space"
            );
        }
    }

    #[test]
    fn panes_keep_their_identity_when_others_open_and_close() {
        let mut layout = layout_at(Workspace::default(), SIZE);
        let vehicles = layout.pane_of(Panel::Vehicles).unwrap();
        let map = layout.pane_of(Panel::Map).unwrap();

        layout.update(Message::Toggle(Panel::Logs, Slot::Left));
        layout.update(Message::Toggle(Panel::Links, Slot::Right));
        layout.update(Message::Toggle(Panel::Logs, Slot::Left));

        assert_eq!(layout.pane_of(Panel::Vehicles), Some(vehicles));
        assert_eq!(layout.pane_of(Panel::Map), Some(map));
        assert!(!layout.is_open(Panel::Logs), "toggled closed");
    }

    #[test]
    fn a_dragged_divider_is_kept_in_pixels() {
        let mut layout = layout_at(Workspace::default(), SIZE);
        let (split, _) = layout
            .panes
            .layout()
            .split_regions(SPACING, MIN_PANE, SIZE)
            .into_iter()
            .find(|(_, (axis, _, _))| *axis == Axis::Vertical)
            .expect("the left column's divider");

        // Drag the divider to 500 px.
        layout.update(Message::Resized(pane_grid::ResizeEvent {
            split,
            ratio: ratio(500.0, SIZE.width),
        }));
        assert!(close_to(layout.workspace.left.size, 500.0));

        // A wider window keeps the dragged width.
        layout.update(Message::Sized(Size::new(2000.0, 900.0)));
        assert!(close_to(regions(&layout)[&Panel::Vehicles].width, 500.0));
    }

    #[test]
    fn an_emptied_column_comes_back_at_its_size() {
        let mut layout = layout_at(Workspace::default(), SIZE);
        layout.workspace.left.size = 420.0;
        layout.update(Message::Toggle(Panel::Vehicles, Slot::Left));
        assert!(
            regions(&layout)[&Panel::Map].width > SIZE.width - 1.0,
            "collapsed"
        );

        layout.update(Message::Toggle(Panel::Mission, Slot::Left));
        assert!(close_to(regions(&layout)[&Panel::Mission].width, 420.0));
    }

    #[test]
    fn dropped_panes_move_between_stacks() {
        let mut layout = layout_at(Workspace::default(), SIZE);
        layout.update(Message::Toggle(Panel::Logs, Slot::Left));
        let logs = layout.pane_of(Panel::Logs).unwrap();
        let vehicles = layout.pane_of(Panel::Vehicles).unwrap();
        let map = layout.pane_of(Panel::Map).unwrap();

        // Onto the top half of the vehicles pane: above it.
        layout.update(Message::Dragged(pane_grid::DragEvent::Dropped {
            pane: logs,
            target: Target::Pane(vehicles, Region::Edge(Edge::Top)),
        }));
        assert_eq!(
            panels(&layout.workspace.left),
            [Panel::Logs, Panel::Vehicles]
        );

        // Onto the map: into the row beneath it.
        let logs = layout.pane_of(Panel::Logs).unwrap();
        layout.update(Message::Dragged(pane_grid::DragEvent::Dropped {
            pane: logs,
            target: Target::Pane(map, Region::Center),
        }));
        assert_eq!(panels(&layout.workspace.middle), [Panel::Logs]);
        assert_eq!(panels(&layout.workspace.left), [Panel::Vehicles]);

        // The map itself never moves.
        layout.update(Message::Dragged(pane_grid::DragEvent::Dropped {
            pane: map,
            target: Target::Edge(Edge::Left),
        }));
        assert!(layout.workspace.find(Panel::Map).is_none());
    }

    /// A layout of two panes stacked in the left column, sharing it evenly,
    /// and the regions the grid gives every pane.
    fn two_in_the_left_column() -> (Layout, BTreeMap<pane_grid::Pane, Rectangle>) {
        let mut layout = layout_at(Workspace::default(), SIZE);
        layout.update(Message::Toggle(Panel::Mission, Slot::Left));
        let regions = layout.panes.layout().pane_regions(SPACING, MIN_PANE, SIZE);
        (layout, regions)
    }

    /// Open the log in the right column and drop it where `at` hints.
    /// `regions` are from before it opened: the left column's are the same.
    fn drop_at(layout: &mut Layout, regions: &BTreeMap<pane_grid::Pane, Rectangle>, at: Point) {
        layout.update(Message::Toggle(Panel::Logs, Slot::Right));
        let logs = layout.pane_of(Panel::Logs).unwrap();
        let hint = layout.drop_hint(logs, at, regions).expect("a target");
        layout.update(Message::Dragged(pane_grid::DragEvent::Dropped {
            pane: logs,
            target: hint.target,
        }));
    }

    #[test]
    fn a_panel_dragged_in_from_the_rail_opens_where_it_is_dropped() {
        let (mut layout, regions) = two_in_the_left_column();
        let vehicles = regions[&layout.pane_of(Panel::Vehicles).unwrap()];

        // A closed panel has nothing to swap with: the middle of a pane
        // splits it by the half the cursor is on.
        let below_middle = Point::new(vehicles.center_x(), vehicles.center_y() + 1.0);
        let hint = layout
            .incoming_hint(Panel::Logs, below_middle, &regions)
            .expect("a target");
        assert!(matches!(
            hint.target,
            Target::Pane(_, Region::Edge(Edge::Bottom))
        ));
        layout.update(Message::Placed(Panel::Logs, hint.target));
        assert_eq!(
            panels(&layout.workspace.left),
            [Panel::Vehicles, Panel::Logs, Panel::Mission]
        );
        assert!(layout.is_dirty(), "a placed panel is saved");

        // An open panel dragged in moves as its pane would: the middle of
        // another swaps the two.
        let regions = pane_regions(&layout);
        let vehicles = regions[&layout.pane_of(Panel::Vehicles).unwrap()];
        let hint = layout
            .incoming_hint(Panel::Mission, vehicles.center(), &regions)
            .expect("a target");
        assert!(matches!(hint.target, Target::Pane(_, Region::Center)));
        layout.update(Message::Placed(Panel::Mission, hint.target));
        assert_eq!(
            panels(&layout.workspace.left),
            [Panel::Mission, Panel::Logs, Panel::Vehicles]
        );

        // Over its own pane, an open panel has nowhere to go.
        let logs = pane_regions(&layout)[&layout.pane_of(Panel::Logs).unwrap()];
        assert!(
            layout
                .incoming_hint(Panel::Logs, logs.center(), &pane_regions(&layout))
                .is_none()
        );
    }

    #[test]
    fn a_drop_on_half_a_pane_splits_that_pane() {
        let (mut layout, regions) = two_in_the_left_column();
        let first = regions[&layout.pane_of(Panel::Vehicles).unwrap()];

        // The bottom half of the first pane, clear of the divider.
        let at = Point::new(first.center_x(), first.y + first.height * 0.75);
        drop_at(&mut layout, &regions, at);

        assert_eq!(
            panels(&layout.workspace.left),
            [Panel::Vehicles, Panel::Logs, Panel::Mission]
        );
        let shares = shares(&layout.workspace.left);
        for (share, expected) in shares.iter().zip([0.25, 0.25, 0.5]) {
            assert!((share - expected).abs() < 1e-6, "{shares:?}");
        }
    }

    #[test]
    fn a_drop_on_a_divider_shares_the_stack_evenly() {
        let (mut layout, regions) = two_in_the_left_column();
        let first = regions[&layout.pane_of(Panel::Vehicles).unwrap()];

        // Just inside the first pane, but within reach of the divider.
        let at = Point::new(first.center_x(), first.y + first.height - 2.0);
        drop_at(&mut layout, &regions, at);

        assert_eq!(
            panels(&layout.workspace.left),
            [Panel::Vehicles, Panel::Logs, Panel::Mission]
        );
        for share in shares(&layout.workspace.left) {
            assert!((share - 1.0 / 3.0).abs() < 1e-6);
        }
    }

    #[test]
    fn hints_offer_only_what_the_columns_allow() {
        let (mut layout, _) = two_in_the_left_column();
        layout.update(Message::Toggle(Panel::Links, Slot::Middle));
        let regions_now = layout.panes.layout().pane_regions(SPACING, MIN_PANE, SIZE);
        let dragged = layout.pane_of(Panel::Mission).unwrap();
        let vehicles = regions_now[&layout.pane_of(Panel::Vehicles).unwrap()];
        let links = regions_now[&layout.pane_of(Panel::Links).unwrap()];
        let map = regions_now[&layout.pane_of(Panel::Map).unwrap()];

        let edge_of = |at: Point| match layout.drop_hint(dragged, at, &regions_now) {
            Some(DropHint {
                target: Target::Pane(_, Region::Edge(edge)),
                ..
            }) => Some(edge),
            Some(_) => panic!("not a pane target at {at:?}"),
            None => None,
        };

        // A column's pane offers its top and bottom halves, even at its
        // sides.
        let left_side = Point::new(vehicles.x + 2.0, vehicles.center_y() - 40.0);
        assert!(matches!(edge_of(left_side), Some(Edge::Top)));
        // The row's panes offer their left and right halves.
        let row_top = Point::new(links.x + links.width - 20.0, links.y + 2.0);
        assert!(matches!(edge_of(row_top), Some(Edge::Right)));
        // The map opens only the empty right column: the left column and
        // the row already have panes, which are joined beside them.
        assert!(edge_of(Point::new(map.x + 10.0, map.center_y())).is_none());
        assert!(edge_of(Point::new(map.center_x(), map.y + 10.0)).is_none());
        assert!(matches!(
            edge_of(Point::new(map.x + map.width - 10.0, map.center_y())),
            Some(Edge::Right)
        ));
        // The dragged pane itself, and the gap between columns, are not.
        let mission = regions_now[&dragged];
        assert!(
            layout
                .drop_hint(dragged, mission.center(), &regions_now)
                .is_none()
        );
        let gap = Point::new(vehicles.x + vehicles.width + SPACING / 2.0, 100.0);
        assert!(layout.drop_hint(dragged, gap, &regions_now).is_none());

        // The grid's edges are no longer targets.
        let before = layout.workspace.clone();
        layout.update(Message::Dragged(pane_grid::DragEvent::Dropped {
            pane: dragged,
            target: Target::Edge(Edge::Right),
        }));
        assert_eq!(layout.workspace, before);
    }

    /// Every pane's region in `layout`, by pane.
    fn pane_regions(layout: &Layout) -> BTreeMap<pane_grid::Pane, Rectangle> {
        layout
            .panes
            .layout()
            .pane_regions(SPACING, MIN_PANE, layout.size.unwrap())
    }

    #[test]
    fn a_divider_hint_is_centred_on_the_divider_at_the_size_to_come() {
        // Three panes side by side in the row; the last dropped on the
        // divider between the first two.
        let mut layout = layout_at(Workspace::default(), SIZE);
        for panel in [Panel::Mission, Panel::Logs, Panel::Links] {
            layout.update(Message::Toggle(panel, Slot::Middle));
        }
        let regions = pane_regions(&layout);
        let region = |panel| regions[&layout.pane_of(panel).unwrap()];
        let (first, second) = (region(Panel::Mission), region(Panel::Logs));
        let seam = (first.x + first.width + second.x) / 2.0;

        let links = layout.pane_of(Panel::Links).unwrap();
        let hint = layout
            .drop_hint(links, Point::new(seam, first.center_y()), &regions)
            .expect("the divider");
        assert!(matches!(hint.target, Target::Between(..)));
        assert!(
            close_to(hint.area.center_x(), seam),
            "centred on the divider"
        );
        assert_eq!((hint.area.y, hint.area.height), (first.y, first.height));

        // As wide as it will be: the row, shared by the same three.
        layout.update(Message::Dragged(pane_grid::DragEvent::Dropped {
            pane: links,
            target: hint.target,
        }));
        assert!(close_to(
            hint.area.width,
            self::regions(&layout)[&Panel::Links].width
        ));
        assert_eq!(
            panels(&layout.workspace.middle),
            [Panel::Mission, Panel::Links, Panel::Logs]
        );
    }

    #[test]
    fn from_elsewhere_a_divider_hint_is_where_the_pane_lands() {
        let (mut layout, _) = two_in_the_left_column();
        layout.update(Message::Toggle(Panel::Logs, Slot::Right));
        layout.update(Message::Toggle(Panel::Links, Slot::Right));
        let regions = pane_regions(&layout);
        let vehicles = regions[&layout.pane_of(Panel::Vehicles).unwrap()];
        let dragged = layout.pane_of(Panel::Logs).unwrap();

        // The divider beneath a pane of the other column: an even share. (A
        // split's hint is half the pane as drawn instead, which can differ
        // from the result by the share of a gap; see the next test.)
        let at = Point::new(vehicles.center_x(), vehicles.y + vehicles.height + 2.0);
        let hint = layout.drop_hint(dragged, at, &regions).expect("a target");

        let mut dropped = Layout::from_workspace(layout.workspace.clone());
        dropped.update(Message::Sized(SIZE));
        let target = match hint.target {
            // The same panes, by their ids in the copy.
            Target::Pane(pane, region) => Target::Pane(
                dropped.pane_of(*layout.panes.get(pane).unwrap()).unwrap(),
                region,
            ),
            Target::Between(a, b) => Target::Between(
                dropped.pane_of(*layout.panes.get(a).unwrap()).unwrap(),
                dropped.pane_of(*layout.panes.get(b).unwrap()).unwrap(),
            ),
            other => other,
        };
        dropped.update(Message::Dragged(pane_grid::DragEvent::Dropped {
            pane: dropped.pane_of(Panel::Logs).unwrap(),
            target,
        }));
        assert_eq!(
            hint.area,
            self::regions(&dropped)[&Panel::Logs],
            "at {at:?}"
        );
    }

    #[test]
    fn the_hint_fits_the_layout_on_screen() {
        // Within its own column: half of the pane it splits, as that pane
        // is drawn now, although the move frees the dragged pane's place.
        let (layout, regions) = two_in_the_left_column();
        let vehicles = regions[&layout.pane_of(Panel::Vehicles).unwrap()];
        let mission = layout.pane_of(Panel::Mission).unwrap();
        let hint = layout
            .drop_hint(
                mission,
                Point::new(vehicles.center_x(), vehicles.y + 20.0),
                &regions,
            )
            .expect("a target");
        assert_eq!(hint.area.y, vehicles.y);
        // Exactly half the pane, ending with it: the new divider's gap is
        // taken out of the middle.
        assert_eq!(hint.area.height, (vehicles.height - SPACING) / 2.0);
        assert_eq!(hint.area.width, vehicles.width);

        // The only pane of a column, to the empty row: a row as wide as the
        // map is now, although the move collapses the column.
        let mut layout = layout_at(Workspace::default(), SIZE);
        let regions = pane_regions(&layout);
        let map = regions[&layout.pane_of(Panel::Map).unwrap()];
        let vehicles = layout.pane_of(Panel::Vehicles).unwrap();
        let hint = layout
            .drop_hint(vehicles, map.center(), &regions)
            .expect("the empty row");
        assert_eq!((hint.area.x, hint.area.width), (map.x, map.width));
        assert!(close_to(hint.area.height, layout.workspace.middle.size));
        assert!(close_to(hint.area.y + hint.area.height, map.y + map.height));
        layout.update(Message::Dragged(pane_grid::DragEvent::Dropped {
            pane: vehicles,
            target: hint.target,
        }));
        assert_eq!(panels(&layout.workspace.middle), [Panel::Vehicles]);
    }

    #[test]
    fn a_drop_in_the_middle_of_a_pane_swaps_the_two() {
        let mut layout = layout_at(Workspace::default(), SIZE);
        layout.update(Message::Toggle(Panel::Logs, Slot::Right));
        layout.workspace.left.panes = vec![(Panel::Vehicles, 3.0), (Panel::Mission, 1.0)];
        layout.arrange();
        let regions = layout.panes.layout().pane_regions(SPACING, MIN_PANE, SIZE);
        let logs = layout.pane_of(Panel::Logs).unwrap();
        let vehicles = regions[&layout.pane_of(Panel::Vehicles).unwrap()];

        let hint = layout
            .drop_hint(logs, vehicles.center(), &regions)
            .expect("a target");
        assert!(matches!(hint.target, Target::Pane(_, Region::Center)));
        layout.update(Message::Dragged(pane_grid::DragEvent::Dropped {
            pane: logs,
            target: hint.target,
        }));

        // Each took the other's place, and its size there.
        assert_eq!(
            layout.workspace.left.panes,
            [(Panel::Logs, 3.0), (Panel::Mission, 1.0)]
        );
        assert_eq!(panels(&layout.workspace.right), [Panel::Vehicles]);
    }

    #[test]
    fn the_map_opens_an_empty_row() {
        let (layout, regions) = two_in_the_left_column();
        let dragged = layout.pane_of(Panel::Mission).unwrap();
        let map = regions[&layout.pane_of(Panel::Map).unwrap()];

        let hint = layout
            .drop_hint(dragged, map.center(), &regions)
            .expect("the empty row is offered");
        assert!(matches!(
            hint.target,
            Target::Pane(_, Region::Edge(Edge::Bottom))
        ));
        assert!(hint.area.y > map.center_y(), "the band beneath");
    }

    #[test]
    fn the_map_cannot_be_toggled() {
        let mut layout = layout_at(Workspace::default(), SIZE);
        layout.update(Message::Toggle(Panel::Map, Slot::Left));
        assert!(layout.is_open(Panel::Map));
        assert!(layout.workspace.find(Panel::Map).is_none());
    }

    #[test]
    fn workspaces_round_trip_and_are_sanitized_on_load() {
        let workspace = Workspace::default();
        let text = ron::to_string(&workspace).expect("serialize");
        let restored: Workspace = ron::from_str(&text).expect("deserialize");
        assert_eq!(restored, workspace);

        let broken = Workspace {
            left: Stack {
                size: f32::NAN,
                panes: vec![(Panel::Map, 1.0), (Panel::Logs, -2.0)],
            },
            middle: Stack {
                size: 10.0,
                panes: vec![(Panel::Logs, 1.0)],
            },
            right: Stack::empty(360.0),
        };
        let layout = Layout::from_workspace(broken);
        assert_eq!(layout.workspace.left.panes, [(Panel::Logs, 1.0)]);
        assert!(layout.workspace.middle.panes.is_empty(), "no duplicates");
        assert!(layout.workspace.left.size.is_finite());
        assert_eq!(layout.workspace.middle.size, MIN_PANE);
    }
}
