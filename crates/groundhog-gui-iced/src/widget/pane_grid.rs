// Vendored fork of `iced_widget::pane_grid` (iced 0.15.0-dev, rev a8231fd).
// Keep edits minimal and list them so the fork can be re-vendored or upstreamed:
//   - pointer events are gated while a split is resized, and captured on release
//   - a resize stops when a neighbour reaches its minimum size (`clamp_resize`)
//   - a split resize only trades space between adjacent leaves (`Node::resize`,
//     `grow_first_leaf` / `grow_last_leaf`)
//   - a resize can grab the crossing of a horizontal and a vertical seam and
//     resize both axes at once (`ResizeTargets`, `hovered_splits`)
//   - grid chrome is drawn in its own layer, with divider lines between panes
//   - the picked-pane overlay is clipped to its translated bounds
//   - `Content`/`TitleBar` take a `resizing` flag and hide controls while resizing
//   - `TitleBar::is_over_pick_area` treats the whole bar (apart from controls)
//     as a pick area
//   - `State::configuration()` snapshots the current arrangement back into a
//     `Configuration<T>`, the inverse of `with_configuration`
//   - `TitleBar` centres its title and controls vertically
//   - `PaneGrid::drop_target` lets the application decide where a dragged
//     pane lands and what is highlighted (`DropHint`, `Target::Between`); the
//     hint is drawn over the dragged pane, whose preview shrinks to its title
//     bar (`Content::draw_title_bar`)
//   - `Content::preview` gives a pane a label to drag instead: the pane stays
//     in its place and the label follows the cursor over the drop hint, as a
//     `drag_source`'s does (`widget::drag_preview`), clear of a finger
//     (`Memory::by_finger`); the pane fades (`Style::picked_pane`) and its
//     body keeps every event but the pointer's
//   - `PaneGrid::incoming` takes a drag that started outside the grid: the
//     grid draws its drop hint and reports where it was dropped, by the mouse
//     or by a lifted finger
//   - a lost finger (`FingerLost`) cancels a pane drag instead of dropping it
//   - `State::rearrange` replaces the arrangement, keeping the `Pane` of every
//     kept state (so pane widget state survives a rearrangement) and the
//     `Split` of every split between the same two panes along the same axis
//     (so a resize in progress survives one)
//   - `State::neighbours` names a split by the panes on either side of it
//     (`Node::neighbours`, `Node::seams`)
//   - `Axis::split` never gives a side a negative length, nor overflows: where
//     the region cannot hold both minimum sizes, it splits at the ratio
//
// Re-vendoring: copy the upstream `iced_widget/src/pane_grid.rs` and
// `iced_widget/src/pane_grid/` tree at the target iced revision, then re-apply the
// deltas above. This fork spans `src/widget/pane_grid.rs` + `src/widget/pane_grid/`.
// Update the pinned rev in this header (and any iced `rev` in `Cargo.toml`) in the
// same change, so the header never lies about which upstream it was based on.
// The multi-axis (corner) resize delta is a good upstream candidate; upstreaming it
// would reduce the fork.
//
// `dead_code` is allowed because this is a binary crate: much of the vendored API
// surface is intentionally unused. `unwrap_used` is allowed because the vendored
// code indexes and unwraps on invariants upstream maintains (a `Tree`'s element
// count matching its widgets); changing it would fork further from upstream.
//! Pane grids let your users split regions of your application and organize layout dynamically.
//!
//! ![Pane grid - Iced](https://iced.rs/examples/pane_grid.gif)
//!
//! This distribution of space is common in tiling window managers (like
//! [`awesome`](https://awesomewm.org/), [`i3`](https://i3wm.org/), or even
//! [`tmux`](https://github.com/tmux/tmux)).
//!
//! A [`PaneGrid`] supports:
//!
//! * Vertical and horizontal splits
//! * Tracking of the last active pane
//! * Mouse-based resizing
//! * Drag and drop to reorganize panes
//! * Hotkey support
//! * Configurable modifier keys
//! * [`State`] API to perform actions programmatically (`split`, `swap`, `resize`, etc.)
//!
//! # Example
//! ```ignore
//! // Iced's upstream example, kept for reference: it calls the upstream
//! // `pane_grid` constructor, while this fork builds `PaneGrid::new`
//! // directly (see `layout.rs` for the workspace's use).
//! # mod iced { pub mod widget { pub use iced_widget::*; } pub use iced_widget::Renderer; pub use iced_widget::core::*; }
//! # use iced::widget::Widget;
//! #
//! use iced::widget::{pane_grid, text};
//!
//! struct State {
//!     panes: pane_grid::State<Pane>,
//! }
//!
//! enum Pane {
//!     SomePane,
//!     AnotherKindOfPane,
//! }
//!
//! enum Message {
//!     PaneDragged(pane_grid::DragEvent),
//!     PaneResized(pane_grid::ResizeEvent),
//! }
//!
//! fn view(state: &State) -> impl Widget<Message> {
//!     pane_grid(&state.panes, |pane, state, is_maximized| {
//!         pane_grid::Content::new(match state {
//!             Pane::SomePane => text("This is some pane"),
//!             Pane::AnotherKindOfPane => text("This is another kind of pane"),
//!         })
//!     })
//!     .on_drag(Message::PaneDragged)
//!     .on_resize(10, Message::PaneResized)
//! }
//! ```
//! The [`pane_grid` example] showcases how to use a [`PaneGrid`] with resizing,
//! drag and drop, and hotkey support.
//!
//! [`pane_grid` example]: https://github.com/iced-rs/iced/tree/master/examples/pane_grid

#![allow(
    dead_code,
    reason = "the vendored API surface is larger than this fork uses"
)]
#![allow(
    clippy::unwrap_used,
    reason = "upstream relies on invariants this fork keeps unchanged"
)]
#![allow(
    let_underscore_drop,
    reason = "upstream discards container returns it knows are irrelevant"
)]

mod axis;
mod configuration;
mod content;
mod controls;
mod direction;
mod draggable;
mod node;
mod pane;
mod split;
mod title_bar;

pub mod state;

pub use axis::Axis;
pub use configuration::Configuration;
pub use content::Content;
pub use controls::Controls;
pub use direction::Direction;
pub use draggable::Draggable;
pub use node::Node;
pub use pane::Pane;
pub use split::Split;
pub use state::State;
pub use title_bar::TitleBar;

use iced::Theme;
use iced::advanced::layout;
use iced::advanced::mouse;
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget;
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::{self as core, Layout, Shell, Widget};
use iced::touch;
use iced::widget::container;
use iced::window;
use iced::{Background, Border, Color, Event, Length, Pixels, Point, Rectangle, Size, Vector};

use super::drag_preview;
use std::collections::BTreeMap;

/// Where a dragged pane would land, and the area to highlight for it.
#[derive(Debug, Clone, Copy)]
pub struct DropHint {
    pub target: Target,
    /// In the grid's coordinates, like the pane regions it was found in.
    pub area: Rectangle,
}

/// Decides where a dragged [`Pane`] lands, given the cursor and every
/// pane's region; `None` where it cannot be dropped.
type DropTarget<'a> = Box<dyn Fn(Pane, Point, &BTreeMap<Pane, Rectangle>) -> Option<DropHint> + 'a>;

/// Where something dragged from outside the grid lands, given the cursor
/// and every pane's region; `None` where it cannot be dropped.
type IncomingHint<'a> = Box<dyn Fn(Point, &BTreeMap<Pane, Rectangle>) -> Option<DropHint> + 'a>;

/// A drag from outside the grid: where it would land, and the message a
/// drop there produces.
struct Incoming<'a, Message> {
    hint: IncomingHint<'a>,
    on_drop: Box<dyn Fn(Target) -> Message + 'a>,
}

const DRAG_DEADBAND_DISTANCE: f32 = 10.0;
const THICKNESS_RATIO: f32 = 25.0;

/// A collection of panes distributed using either vertical or horizontal splits
/// to completely fill the space available.
///
/// ![Pane grid - Iced](https://iced.rs/examples/pane_grid.gif)
///
/// This distribution of space is common in tiling window managers (like
/// [`awesome`](https://awesomewm.org/), [`i3`](https://i3wm.org/), or even
/// [`tmux`](https://github.com/tmux/tmux)).
///
/// A [`PaneGrid`] supports:
///
/// * Vertical and horizontal splits
/// * Tracking of the last active pane
/// * Mouse-based resizing
/// * Drag and drop to reorganize panes
/// * Hotkey support
/// * Configurable modifier keys
/// * [`State`] API to perform actions programmatically (`split`, `swap`, `resize`, etc.)
///
/// # Example
/// ```ignore
/// // Iced's upstream example; this fork builds `PaneGrid::new` directly.
/// # mod iced { pub mod widget { pub use iced_widget::*; } pub use iced_widget::Renderer; pub use iced_widget::core::*; }
/// # use iced::widget::Widget;
/// #
/// use iced::widget::{pane_grid, text};
///
/// struct State {
///     panes: pane_grid::State<Pane>,
/// }
///
/// enum Pane {
///     SomePane,
///     AnotherKindOfPane,
/// }
///
/// enum Message {
///     PaneDragged(pane_grid::DragEvent),
///     PaneResized(pane_grid::ResizeEvent),
/// }
///
/// fn view(state: &State) -> impl Widget<Message> {
///     pane_grid(&state.panes, |pane, state, is_maximized| {
///         pane_grid::Content::new(match state {
///             Pane::SomePane => text("This is some pane"),
///             Pane::AnotherKindOfPane => text("This is another kind of pane"),
///         })
///     })
///     .on_drag(Message::PaneDragged)
///     .on_resize(10, Message::PaneResized)
/// }
/// ```
pub struct PaneGrid<'a, Message, T, W, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Theme: Catalog,
    Renderer: core::Renderer,
{
    internal: &'a state::Internal,
    panes: Vec<Pane>,
    contents: Vec<Content<'a, Message, T, W, Theme, Renderer>>,
    width: Length,
    height: Length,
    spacing: f32,
    min_size: f32,
    on_click: Option<Box<dyn Fn(Pane) -> Message + 'a>>,
    on_drag: Option<Box<dyn Fn(DragEvent) -> Message + 'a>>,
    drop_target: Option<DropTarget<'a>>,
    incoming: Option<Incoming<'a, Message>>,
    #[allow(clippy::type_complexity, reason = "a match of iced's own signature")]
    on_resize: Option<(f32, Box<dyn Fn(ResizeEvent) -> Message + 'a>)>,
    class: <Theme as Catalog>::Class<'a>,
    last_mouse_interaction: Option<mouse::Interaction>,
}

impl<'a, Message, T, W, Theme, Renderer> PaneGrid<'a, Message, T, W, Theme, Renderer>
where
    Theme: Catalog,
    Renderer: core::Renderer,
{
    /// Creates a [`PaneGrid`] with the given [`State`] and view function.
    ///
    /// The view function will be called to display each [`Pane`] present in the
    /// [`State`]. [`bool`] is set if the pane is maximized.
    pub fn new<S>(
        state: &'a State<S>,
        view: impl Fn(Pane, &'a S, bool) -> Content<'a, Message, T, W, Theme, Renderer>,
    ) -> Self {
        let panes = state.panes.keys().copied().collect();
        let contents = state
            .panes
            .iter()
            .map(|(pane, pane_state)| match state.maximized() {
                Some(p) if *pane == p => view(*pane, pane_state, true),
                _ => view(*pane, pane_state, false),
            })
            .collect();

        Self {
            internal: &state.internal,
            panes,
            contents,
            width: Length::Fill,
            height: Length::Fill,
            spacing: 0.0,
            min_size: 50.0,
            on_click: None,
            on_drag: None,
            drop_target: None,
            incoming: None,
            on_resize: None,
            class: <Theme as Catalog>::default(),
            last_mouse_interaction: None,
        }
    }

    /// Sets the width of the [`PaneGrid`].
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Sets the height of the [`PaneGrid`].
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Sets the spacing _between_ the panes of the [`PaneGrid`].
    pub fn spacing(mut self, amount: impl Into<Pixels>) -> Self {
        self.spacing = amount.into().0;
        self
    }

    /// Sets the minimum size of a [`Pane`] in the [`PaneGrid`] on both axes.
    pub fn min_size(mut self, min_size: impl Into<Pixels>) -> Self {
        self.min_size = min_size.into().0;
        self
    }

    /// Sets the message that will be produced when a [`Pane`] of the
    /// [`PaneGrid`] is clicked.
    pub fn on_click<F>(mut self, f: F) -> Self
    where
        F: 'a + Fn(Pane) -> Message,
    {
        self.on_click = Some(Box::new(f));
        self
    }

    /// Enables the drag and drop interactions of the [`PaneGrid`], which will
    /// use the provided function to produce messages.
    pub fn on_drag<F>(mut self, f: F) -> Self
    where
        F: 'a + Fn(DragEvent) -> Message,
    {
        if self.internal.maximized().is_none() {
            self.on_drag = Some(Box::new(f));
        }
        self
    }

    /// Decide where dragged panes land: `target` gets the dragged pane, the
    /// cursor and every pane's region, and returns the target and the area
    /// to highlight, or `None` where nothing may be dropped. Without it, a
    /// pane can be dropped on any part of another pane, or the grid's edges.
    pub fn drop_target<F>(mut self, target: F) -> Self
    where
        F: 'a + Fn(Pane, Point, &BTreeMap<Pane, Rectangle>) -> Option<DropHint>,
    {
        self.drop_target = Some(Box::new(target));
        self
    }

    /// Take a drag that started outside the grid, such as a button dragged
    /// onto it, for as long as the application keeps this set: `hint` gets
    /// the cursor and every pane's region and returns where a drop there
    /// lands and the area to highlight, or `None`. A release over a hint
    /// publishes `on_drop` with its target; ending the drag anywhere else is
    /// the application's to notice.
    pub fn incoming<H, D>(mut self, hint: H, on_drop: D) -> Self
    where
        H: 'a + Fn(Point, &BTreeMap<Pane, Rectangle>) -> Option<DropHint>,
        D: 'a + Fn(Target) -> Message,
    {
        self.incoming = Some(Incoming {
            hint: Box::new(hint),
            on_drop: Box::new(on_drop),
        });
        self
    }

    /// The incoming drag's hint under `cursor`, in a grid laid out at
    /// `bounds`.
    fn incoming_hint(&self, cursor: mouse::Cursor, bounds: Rectangle) -> Option<DropHint> {
        let incoming = self.incoming.as_ref()?;
        (incoming.hint)(cursor.position()?, &self.regions(bounds))
    }

    /// The custom drop target under `cursor` while `dragged` is dragged,
    /// in a grid laid out at `bounds`; `None` without a custom target.
    fn custom_drop(
        &self,
        dragged: Pane,
        cursor: Point,
        bounds: Rectangle,
    ) -> Option<Option<DropHint>> {
        let target = self.drop_target.as_ref()?;
        let regions = self.regions(bounds);
        Some(target(dragged, cursor, &regions))
    }

    /// Every pane's region, in a grid laid out at `bounds`.
    fn regions(&self, bounds: Rectangle) -> BTreeMap<Pane, Rectangle> {
        let offset = Vector::new(bounds.x, bounds.y);
        self.internal
            .layout()
            .pane_regions(self.spacing, self.min_size, bounds.size())
            .into_iter()
            .map(|(pane, region)| (pane, region + offset))
            .collect()
    }

    /// Enables the resize interactions of the [`PaneGrid`], which will
    /// use the provided function to produce messages.
    ///
    /// The `leeway` describes the amount of space around a split that can be
    /// used to grab it.
    ///
    /// The grabbable area of a split will have a length of `spacing + leeway`,
    /// properly centered. In other words, a length of
    /// `(spacing + leeway) / 2.0` on either side of the split line.
    pub fn on_resize<F>(mut self, leeway: impl Into<Pixels>, f: F) -> Self
    where
        F: 'a + Fn(ResizeEvent) -> Message,
    {
        if self.internal.maximized().is_none() {
            self.on_resize = Some((leeway.into().0, Box::new(f)));
        }
        self
    }

    /// Sets the style of the [`PaneGrid`].
    #[must_use]
    pub fn style(mut self, style: impl Fn(&Theme) -> Style + 'a) -> Self
    where
        <Theme as Catalog>::Class<'a>: From<StyleFn<'a, Theme>>,
    {
        self.class = (Box::new(style) as StyleFn<'a, Theme>).into();
        self
    }

    /// Sets the style class of the [`PaneGrid`].
    #[must_use]
    pub fn class(mut self, class: impl Into<<Theme as Catalog>::Class<'a>>) -> Self {
        self.class = class.into();
        self
    }

    fn drag_enabled(&self) -> bool {
        if self.internal.maximized().is_none() {
            self.on_drag.is_some()
        } else {
            Default::default()
        }
    }

    fn grid_interaction(
        &self,
        action: &state::Action,
        layout: Layout,
        cursor: mouse::Cursor,
    ) -> Option<mouse::Interaction> {
        if action.picked_pane().is_some() {
            return Some(mouse::Interaction::Grabbing);
        }

        let mut targets = action.picked_targets();

        if targets.is_empty()
            && let Some((leeway, _)) = &self.on_resize
            && let Some(cursor_position) = cursor.position()
        {
            let bounds = layout.bounds();
            let splits =
                self.internal
                    .layout()
                    .split_regions(self.spacing, self.min_size, bounds.size());

            let relative_cursor =
                Point::new(cursor_position.x - bounds.x, cursor_position.y - bounds.y);

            targets = hovered_splits(splits.iter(), self.spacing + leeway, relative_cursor);
        }

        // A corner grab moves both seams, so it gets the four-way move cursor.
        if targets.is_corner() {
            return Some(mouse::Interaction::AllScroll);
        }

        if targets.horizontal.is_some() {
            return Some(mouse::Interaction::ResizingVertically);
        }

        if targets.vertical.is_some() {
            return Some(mouse::Interaction::ResizingHorizontally);
        }

        None
    }

    /// Every seam to highlight: all picked seams while resizing, or the hovered
    /// seam(s) otherwise. Each entry is `(axis, bounds, is_picked)`.
    fn split_highlights(
        &self,
        action: &state::Action,
        node: &Node,
        layout: Layout,
        cursor: mouse::Cursor,
        resize_leeway: Option<f32>,
    ) -> Vec<(Axis, Rectangle, bool)> {
        let bounds = layout.bounds();
        let splits = node.split_regions(self.spacing, self.min_size, bounds.size());

        let picked = action.picked_targets();

        let (targets, is_picked) = if !picked.is_empty() {
            (picked, true)
        } else if let (Some(leeway), Some(cursor_position)) = (resize_leeway, cursor.position()) {
            let relative_cursor =
                Point::new(cursor_position.x - bounds.x, cursor_position.y - bounds.y);

            (
                hovered_splits(splits.iter(), self.spacing + leeway, relative_cursor),
                false,
            )
        } else {
            (state::ResizeTargets::default(), false)
        };

        targets
            .iter()
            .filter_map(|(split, axis)| {
                let (_axis, region, ratio) = splits.get(&split)?;

                // Draw the highlight from the *real* seam, not the widened grab
                // zone: centring on the leeway-thick strip pushed the hover
                // indicator past the split.
                let region = axis.split_line_bounds(*region, *ratio, self.spacing);

                Some((axis, region + Vector::new(bounds.x, bounds.y), is_picked))
            })
            .collect()
    }
}

#[derive(Default)]
struct Memory {
    action: state::Action,
    order: Vec<Pane>,
    /// Whether the last press was a finger's, so a pane it drags shows its
    /// preview clear of the finger.
    by_finger: bool,
}

impl<Message, T, W, Theme, Renderer> widget::Meta for PaneGrid<'_, Message, T, W, Theme, Renderer>
where
    Theme: Catalog,
    Renderer: core::Renderer,
{
}

impl<Message, T, W, Theme, Renderer> Widget<Message, Theme, Renderer>
    for PaneGrid<'_, Message, T, W, Theme, Renderer>
where
    Theme: Catalog,
    Renderer: core::Renderer,
    T: Widget<Message, Theme, Renderer>,
    W: Widget<Message, Theme, Renderer>,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Memory>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(Memory::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        let Memory { order, .. } = tree.state.downcast_ref();

        // `Pane` always increments and is iterated by Ord so new
        // states are always added at the end. We can simply remove
        // states which no longer exist and `diff_children` will
        // diff the remaining values in the correct order and
        // add new states at the end

        let mut i = 0;
        let mut j = 0;
        tree.children.retain(|_| {
            let retain = self.panes.get(i) == order.get(j);

            if retain {
                i += 1;
            }
            j += 1;

            retain
        });

        tree.diff_children_custom(
            &mut self.contents,
            |state, content| content.diff(state),
            Content::state,
        );

        let Memory { order, .. } = tree.state.downcast_mut();
        order.clone_from(&self.panes);
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let bounds = limits.resolve(self.width, self.height, Size::ZERO);
        let regions = self
            .internal
            .layout()
            .pane_regions(self.spacing, self.min_size, bounds);

        for ((pane, content), child) in self
            .panes
            .iter()
            .zip(&mut self.contents)
            .zip(tree.children.iter_mut())
        {
            if self
                .internal
                .maximized()
                .is_some_and(|maximized| maximized != *pane)
            {
                child.size = Size::ZERO;
                continue;
            }

            let Some(region) = regions.get(pane) else {
                child.size = Size::ZERO;
                continue;
            };

            let size = Size::new(region.width, region.height);

            content.layout(child, renderer, &layout::Limits::new(size, size));

            child.translation = Vector::new(region.x, region.y);
        }

        tree.size = bounds;
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            self.panes
                .iter()
                .zip(&mut self.contents)
                .zip(layout.iter_mut(&mut tree.children))
                .filter(|((pane, _), _)| {
                    self.internal
                        .maximized()
                        .is_none_or(|maximized| **pane == maximized)
                })
                .for_each(|((_pane, content), (layout, state))| {
                    content.operate(state, layout, viewport, renderer, operation);
                });
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let node = self.internal.layout();

        let on_drag = if self.drag_enabled() {
            &self.on_drag
        } else {
            &None
        };

        // A drag from outside: its hint follows the cursor, and a release
        // over one drops it. A finger is dropped where it is lifted; one the
        // system takes away drops nothing.
        if let Some(incoming) = &self.incoming {
            let dropped = match event {
                Event::Mouse(mouse::Event::CursorMoved { .. })
                | Event::Touch(touch::Event::FingerMoved { .. }) => {
                    shell.request_redraw();
                    None
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => Some(cursor),
                Event::Touch(touch::Event::FingerLifted { position, .. }) => {
                    Some(mouse::Cursor::Available(*position))
                }
                _ => None,
            };

            if let Some(hint) = dropped.and_then(|at| self.incoming_hint(at, layout.bounds())) {
                shell.publish((incoming.on_drop)(hint.target));
            }
        }

        let Memory {
            action, by_finger, ..
        } = tree.state.downcast_mut();
        let picked_pane = action.picked_pane().map(|(pane, _)| pane);

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => *by_finger = false,
            Event::Touch(touch::Event::FingerPressed { .. }) => *by_finger = true,
            _ => {}
        }

        // A left-press on a split starts a resize, and while a resize is in
        // progress the grid owns the pointer. In both cases the panes underneath
        // must not receive pointer events, otherwise they react to a drag that is
        // resizing a split (e.g. the map panning while you resize the pane beside
        // it).
        let is_pointer_event = matches!(event, Event::Mouse(_) | Event::Touch(_));

        let starting_resize = matches!(
            event,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                | Event::Touch(touch::Event::FingerPressed { .. })
        ) && matches!(action, state::Action::Idle)
            && self.on_resize.as_ref().is_some_and(|(leeway, _)| {
                cursor
                    .position_over(layout.bounds())
                    .is_some_and(|position| {
                        let relative = Point::new(
                            position.x - layout.bounds().x,
                            position.y - layout.bounds().y,
                        );
                        let splits =
                            node.split_regions(self.spacing, self.min_size, layout.bounds().size());

                        !hovered_splits(splits.iter(), self.spacing + *leeway, relative).is_empty()
                    })
            });

        let resizing = matches!(action, state::Action::Resizing(_));
        let grid_owns_pointer = resizing || starting_resize;

        for ((pane, content), (layout, child)) in self
            .panes
            .iter()
            .copied()
            .zip(&mut self.contents)
            .zip(layout.iter_mut(&mut tree.children))
            .filter(|((pane, _), _)| {
                self.internal
                    .maximized()
                    .is_none_or(|maximized| *pane == maximized)
            })
        {
            // While the grid is resizing, keep pointer events away from panes.
            if is_pointer_event && grid_owns_pointer {
                continue;
            }

            let is_picked = picked_pane == Some(pane);

            content.update(
                child, event, layout, cursor, renderer, shell, viewport, is_picked,
            );
        }

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
            | Event::Touch(touch::Event::FingerPressed { .. }) => {
                let bounds = layout.bounds();

                if let Some(cursor_position) = cursor.position_over(bounds) {
                    shell.capture_event();

                    match &self.on_resize {
                        Some((leeway, _)) => {
                            let relative_cursor = Point::new(
                                cursor_position.x - bounds.x,
                                cursor_position.y - bounds.y,
                            );

                            let splits =
                                node.split_regions(self.spacing, self.min_size, bounds.size());

                            let clicked_split = hovered_splits(
                                splits.iter(),
                                self.spacing + leeway,
                                relative_cursor,
                            );

                            if !clicked_split.is_empty() {
                                if action.picked_pane().is_none() {
                                    *action = state::Action::Resizing(clicked_split);
                                }
                            } else {
                                click_pane(
                                    action,
                                    cursor_position,
                                    shell,
                                    self.panes
                                        .iter()
                                        .copied()
                                        .zip(&self.contents)
                                        .zip(layout.iter(&tree.children))
                                        .map(|((pane, content), (layout, tree))| {
                                            (pane, content, layout, tree)
                                        }),
                                    &self.on_click,
                                    on_drag,
                                );
                            }
                        }
                        None => {
                            click_pane(
                                action,
                                cursor_position,
                                shell,
                                self.panes
                                    .iter()
                                    .copied()
                                    .zip(&self.contents)
                                    .zip(layout.iter(&tree.children))
                                    .map(|((pane, content), (layout, tree))| {
                                        (pane, content, layout, tree)
                                    }),
                                &self.on_click,
                                on_drag,
                            );
                        }
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
            | Event::Touch(touch::Event::FingerLifted { .. })
            | Event::Touch(touch::Event::FingerLost { .. }) => {
                // A finger the system takes away drops nothing.
                let lost = matches!(event, Event::Touch(touch::Event::FingerLost { .. }));

                if let Some((pane, origin)) = action.picked_pane()
                    && let Some(on_drag) = on_drag
                    && let Some(cursor_position) = cursor.position()
                {
                    if lost {
                        shell.publish(on_drag(DragEvent::Canceled { pane }));
                    } else if cursor_position.distance(origin) > DRAG_DEADBAND_DISTANCE {
                        let event = if let Some(hint) =
                            self.custom_drop(pane, cursor_position, layout.bounds())
                        {
                            match hint {
                                Some(hint) => DragEvent::Dropped {
                                    pane,
                                    target: hint.target,
                                },
                                None => DragEvent::Canceled { pane },
                            }
                        } else if let Some(edge) = in_edge(layout, cursor_position) {
                            DragEvent::Dropped {
                                pane,
                                target: Target::Edge(edge),
                            }
                        } else {
                            let dropped_region = self
                                .panes
                                .iter()
                                .copied()
                                .zip(&self.contents)
                                .zip(layout.iter(&tree.children))
                                .find_map(|(target, (layout, _))| {
                                    layout_region(layout, cursor_position)
                                        .map(|region| (target, region))
                                });

                            match dropped_region {
                                Some(((target, _), region)) if pane != target => {
                                    DragEvent::Dropped {
                                        pane,
                                        target: Target::Pane(target, region),
                                    }
                                }
                                _ => DragEvent::Canceled { pane },
                            }
                        };

                        shell.publish(on_drag(event));
                    } else {
                        shell.publish(on_drag(DragEvent::Canceled { pane }));
                    }
                }

                if !action.picked_targets().is_empty() {
                    shell.capture_event();
                }

                *action = state::Action::Idle;
            }
            Event::Mouse(mouse::Event::CursorMoved { .. })
            | Event::Touch(touch::Event::FingerMoved { .. }) => {
                let targets = action.picked_targets();

                if let Some((_, on_resize)) = &self.on_resize {
                    if !targets.is_empty() {
                        let bounds = layout.bounds();

                        let splits = node.split_regions(self.spacing, self.min_size, bounds.size());

                        if let Some(cursor_position) = cursor.position() {
                            for (split, axis) in targets.iter() {
                                let Some((_, rectangle, current)) = splits.get(&split) else {
                                    continue;
                                };

                                let ratio = resize_ratio(axis, *rectangle, cursor_position, bounds);

                                // Stop the drag once a neighbouring pane reaches its
                                // minimum, instead of letting the layout clamp spill the
                                // change onto the next pane. Clamping each axis on its
                                // own is exact: a vertical seam only affects widths and
                                // a horizontal one only heights.
                                let ratio = clamp_resize(
                                    node.as_ref(),
                                    split,
                                    ratio,
                                    bounds.size(),
                                    self.spacing,
                                    self.min_size,
                                    *current,
                                );

                                shell.publish(on_resize(ResizeEvent { split, ratio }));
                            }
                        }

                        shell.capture_event();
                    } else if action.picked_pane().is_some() {
                        shell.request_redraw();
                    }
                }
            }
            _ => {}
        }

        if shell.redraw_request() != window::RedrawRequest::NextFrame {
            let interaction = self
                .grid_interaction(action, layout, cursor)
                .or_else(|| {
                    self.panes
                        .iter()
                        .zip(&self.contents)
                        .zip(layout.iter(&tree.children))
                        .filter(|((pane, _content), (_layout, _tree))| {
                            self.internal
                                .maximized()
                                .is_none_or(|maximized| **pane == maximized)
                        })
                        .find_map(|((_pane, content), (layout, tree))| {
                            content.grid_interaction(tree, layout, cursor, on_drag.is_some())
                        })
                })
                .unwrap_or(mouse::Interaction::None);

            if let Event::Window(window::Event::RedrawRequested(_now)) = event {
                self.last_mouse_interaction = Some(interaction);
            } else if self
                .last_mouse_interaction
                .is_some_and(|last_mouse_interaction| last_mouse_interaction != interaction)
            {
                shell.request_redraw();
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let Memory { action, .. } = tree.state.downcast_ref();

        if let Some(grid_interaction) = self.grid_interaction(action, layout, cursor) {
            return grid_interaction;
        }

        self.panes
            .iter()
            .copied()
            .zip(&self.contents)
            .zip(layout.iter(&tree.children))
            .filter(|((pane, _), _)| {
                self.internal
                    .maximized()
                    .is_none_or(|maximized| *pane == maximized)
            })
            .map(|((_pane, content), (layout, tree))| {
                content.mouse_interaction(
                    tree,
                    layout,
                    cursor,
                    viewport,
                    renderer,
                    self.drag_enabled(),
                )
            })
            .max()
            .unwrap_or_default()
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        defaults: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let Memory { action, .. } = tree.state.downcast_ref();
        let resizing = matches!(*action, state::Action::Resizing(_));
        let node = self.internal.layout();
        let resize_leeway = self.on_resize.as_ref().map(|(leeway, _)| *leeway);

        let picked_pane = action.picked_pane();
        let dragged_pane = picked_pane
            .filter(|(_, origin)| is_dragging(*origin, cursor.position().unwrap_or_default()));

        // Every seam to highlight: all picked seams while resizing, or the
        // hovered seam(s) otherwise. A corner grab highlights both seams.
        let split_highlights = if dragged_pane.is_none() && self.incoming.is_none() {
            self.split_highlights(action, node.as_ref(), layout, cursor, resize_leeway)
        } else {
            Vec::new()
        };

        let pane_cursor = if picked_pane.is_some() || self.incoming.is_some() {
            mouse::Cursor::Unavailable
        } else {
            cursor
        };

        // A custom drop target replaces the default highlights entirely; its
        // own is drawn over the picked pane (see `PickedPane`), which would
        // cover it otherwise.
        let custom_hint = match (dragged_pane, cursor.position()) {
            (Some((dragged, _)), Some(cursor_position)) => {
                self.custom_drop(dragged, cursor_position, layout.bounds())
            }
            _ => None,
        };

        let pane_in_edge = if picked_pane.is_some() && custom_hint.is_none() {
            cursor
                .position()
                .and_then(|cursor_position| in_edge(layout, cursor_position))
        } else {
            None
        };

        let style = Catalog::style(theme, &self.class);

        for ((id, content), (pane_layout, tree)) in self
            .panes
            .iter()
            .copied()
            .zip(&self.contents)
            .zip(layout.iter(&tree.children))
            .filter(|((pane, _), _)| {
                self.internal
                    .maximized()
                    .is_none_or(|maximized| maximized == *pane)
            })
        {
            match picked_pane {
                // A picked pane with a preview stays in its place; one
                // without is drawn as it moves (see `PickedPane`).
                Some((dragging, _)) if id == dragging && !content.has_preview() => {}
                Some((dragging, _)) => {
                    content.draw(
                        tree,
                        renderer,
                        theme,
                        defaults,
                        pane_layout,
                        pane_cursor,
                        viewport,
                        resizing,
                    );

                    // Once its preview is dragged away, the pane fades, so
                    // it reads as the place the drag left. In a layer of
                    // its own, to cover the layers its body pushes.
                    if id == dragging && dragged_pane.is_some() {
                        let bounds = pane_layout.bounds();
                        renderer.with_layer(bounds, |renderer| {
                            renderer.fill_quad(
                                renderer::Quad {
                                    bounds,
                                    ..renderer::Quad::default()
                                },
                                style.picked_pane,
                            );
                        });
                    }

                    if let Some(cursor_position) = cursor.position()
                        && dragged_pane.is_some()
                        && custom_hint.is_none()
                        && pane_in_edge.is_none()
                        && let Some(region) = layout_region(pane_layout, cursor_position)
                    {
                        let bounds = layout_region_bounds(pane_layout, region);

                        renderer.fill_quad(
                            renderer::Quad {
                                bounds,
                                border: style.hovered_region.border,
                                ..renderer::Quad::default()
                            },
                            style.hovered_region.background,
                        );
                    }
                }
                _ => {
                    content.draw(
                        tree,
                        renderer,
                        theme,
                        defaults,
                        pane_layout,
                        pane_cursor,
                        viewport,
                        resizing,
                    );
                }
            }
        }

        // Draw all the grid chrome — dividers, drop highlights, and split
        // highlights — inside its own layer.
        //
        // `Renderer::with_layer` allocates a *new* layer (via `push_clip`) that is
        // composited after the layers created before it. Pane contents that clip
        // themselves with `with_layer` — notably the slippy map — therefore end up
        // in an earlier layer, and anything we draw afterwards at the base level
        // would be composited *underneath* them. Creating this layer last keeps the
        // chrome on top of the panes.
        renderer.with_layer(layout.bounds(), |renderer| {
            if dragged_pane.is_some()
                && let Some(edge) = pane_in_edge
            {
                let bounds = edge_bounds(layout, edge);

                renderer.fill_quad(
                    renderer::Quad {
                        bounds,
                        border: style.hovered_region.border,
                        ..renderer::Quad::default()
                    },
                    style.hovered_region.background,
                );
            }

            for (axis, split_region, is_picked) in &split_highlights {
                let highlight = if *is_picked {
                    style.picked_split
                } else {
                    style.hovered_split
                };

                // Draw a line of exactly `highlight.width`, centred on the seam.
                //
                // `split_region` is NOT always `spacing` thick: for a *hovered*
                // split it is widened by the resize leeway (the grab zone). So we
                // must not derive the thickness from it — only its centre.
                let thickness = highlight.width;

                let bounds = match axis {
                    Axis::Horizontal => {
                        let center = split_region.y + split_region.height / 2.0;

                        Rectangle {
                            x: split_region.x,
                            y: (center - thickness / 2.0).round(),
                            width: split_region.width,
                            height: thickness,
                        }
                    }
                    Axis::Vertical => {
                        let center = split_region.x + split_region.width / 2.0;

                        Rectangle {
                            x: (center - thickness / 2.0).round(),
                            y: split_region.y,
                            width: thickness,
                            height: split_region.height,
                        }
                    }
                };

                renderer.fill_quad(
                    renderer::Quad {
                        bounds,
                        ..renderer::Quad::default()
                    },
                    highlight.color,
                );
            }

            if let Some(hint) = self.incoming_hint(cursor, layout.bounds()) {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: hint.area,
                        border: style.hovered_region.border,
                        ..renderer::Quad::default()
                    },
                    style.hovered_region.background,
                );
            }
        });
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        let state = tree.state.downcast_ref::<Memory>();
        let picked_pane = state.action.picked_pane();
        let lift = if state.by_finger {
            drag_preview::TOUCH_LIFT
        } else {
            drag_preview::LIFT
        };
        // Only a picked pane draws a hint.
        let regions = picked_pane.map(|_| self.regions(layout.bounds()));
        let mut hint = regions.and_then(|regions| {
            self.drop_target.as_ref().map(|target| DropHintSource {
                target,
                regions,
                class: &self.class,
            })
        });

        self.panes
            .iter()
            .copied()
            .zip(&mut self.contents)
            .zip(layout.iter_mut(&mut tree.children))
            .flat_map(|((pane, content), (layout, tree))| {
                if self
                    .internal
                    .maximized()
                    .is_some_and(|maximized| maximized != pane)
                {
                    return Vec::new();
                }

                if let Some((picked_pane, origin)) = picked_pane
                    && picked_pane == pane
                {
                    return vec![overlay::Element::new(Box::new(PickedPane {
                        origin,
                        content,
                        tree,
                        layout,
                        hint: hint.take(),
                        dragged: pane,
                        lift,
                        window,
                    }))];
                }

                content.overlay(tree, layout, renderer, viewport, translation, window)
            })
            .collect()
    }
}

struct PickedPane<'a, 'b, Message, T, W, Theme, Renderer>
where
    Theme: container::Catalog + Catalog,
    Renderer: core::Renderer,
{
    content: &'a Content<'b, Message, T, W, Theme, Renderer>,
    origin: Point,
    tree: &'a mut Tree,
    layout: Layout,
    hint: Option<DropHintSource<'a, 'b, Theme>>,
    dragged: Pane,
    /// How far above the cursor a preview is drawn.
    lift: f32,
    window: Size,
}

/// What a picked pane needs to draw the custom drop hint over itself.
struct DropHintSource<'a, 'b, Theme: Catalog> {
    target: &'a DropTarget<'b>,
    regions: BTreeMap<Pane, Rectangle>,
    class: &'a <Theme as Catalog>::Class<'b>,
}

impl<'a, 'b, Message, T, W, Theme, Renderer> core::Overlay<Message, Theme, Renderer>
    for PickedPane<'a, 'b, Message, T, W, Theme, Renderer>
where
    Theme: container::Catalog + Catalog,
    Renderer: core::Renderer,
    T: Widget<Message, Theme, Renderer>,
    W: Widget<Message, Theme, Renderer>,
{
    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        cursor: mouse::Cursor,
    ) {
        let cursor_position = cursor.position().unwrap_or_default();
        let dragging = is_dragging(self.origin, cursor_position);

        // A pane with a preview stays in its place, drawn with the others;
        // only the drop hint and, over it, the preview follow the cursor.
        if self.content.has_preview() {
            if dragging {
                self.draw_hint(renderer, theme, cursor_position);
                self.content.draw_preview(
                    self.tree,
                    renderer,
                    theme,
                    style,
                    cursor_position,
                    self.lift,
                    self.window,
                );
            }
            return;
        }

        let translation = if dragging {
            cursor_position - self.origin
        } else {
            Vector::ZERO
        };

        // Clip to the *translated* pane so the floating preview is not cut off.
        // (Clipping to the grab origin, as before, discarded everything above and
        // to the left of the point where the drag started.)
        let bounds = self.layout.bounds() + translation;

        renderer.with_layer(bounds, |renderer| {
            renderer.with_translation(translation, |renderer| {
                // With a custom drop target, the preview is the title bar
                // alone, so it hides as little as possible of where the pane
                // would land.
                if self.hint.is_some()
                    && self.content.draw_title_bar(
                        self.tree,
                        renderer,
                        theme,
                        style,
                        self.layout,
                        &Rectangle::INFINITE,
                    )
                {
                    return;
                }

                self.content.draw(
                    self.tree,
                    renderer,
                    theme,
                    style,
                    self.layout,
                    mouse::Cursor::Unavailable,
                    &Rectangle::INFINITE,
                    false,
                );
            });
        });

        // The drop hint, over the preview.
        if dragging {
            self.draw_hint(renderer, theme, cursor_position);
        }
    }
}

impl<Message, T, W, Theme, Renderer> PickedPane<'_, '_, Message, T, W, Theme, Renderer>
where
    Theme: container::Catalog + Catalog,
    Renderer: core::Renderer,
{
    /// The custom drop hint for the cursor, if there is one.
    fn draw_hint(&self, renderer: &mut Renderer, theme: &Theme, cursor_position: Point) {
        if let Some(source) = &self.hint
            && let Some(hint) = (source.target)(self.dragged, cursor_position, &source.regions)
        {
            let style = <Theme as Catalog>::style(theme, source.class);
            renderer.with_layer(hint.area, |renderer| {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: hint.area,
                        border: style.hovered_region.border,
                        ..renderer::Quad::default()
                    },
                    style.hovered_region.background,
                );
            });
        }
    }
}

fn is_dragging(origin: Point, cursor: Point) -> bool {
    cursor.distance(origin) > DRAG_DEADBAND_DISTANCE
}

/// Clamps a resize ratio so the drag stops the moment a pane would drop below its
/// minimum size, instead of the layout's per-split clamp spilling the change onto
/// the next pane over.
fn clamp_resize(
    node: &Node,
    split: Split,
    target: f32,
    bounds: Size,
    spacing: f32,
    min_size: f32,
    current: f32,
) -> f32 {
    // Whether the minimum pane size would have to be enforced at this ratio. We
    // probe the *unclamped* layout (minimum = 0): if any pane would fall below the
    // minimum there, the layout would clamp it — which is the moment the resize
    // should stop. This works regardless of how many panes the split legitimately
    // resizes (e.g. a split whose side is a perpendicular stack moves all of them).
    let needs_clamp = |ratio: f32| -> bool {
        let mut candidate = node.clone();

        if !candidate.resize(split, ratio) {
            return false;
        }

        candidate
            .pane_regions(spacing, 0.0, bounds)
            .values()
            .any(|region| region.width < min_size - 0.5 || region.height < min_size - 0.5)
    };

    // Search outward from the current ratio, in the direction of the drag, for the
    // furthest ratio that does not need clamping.
    let direction = if target >= current { 1.0 } else { -1.0 };
    let mut low = 0.0_f32;
    let mut high = (target - current).abs();
    let mut best = current;

    for _ in 0..24 {
        let distance = (low + high) / 2.0;
        let ratio = (current + direction * distance).clamp(0.0, 1.0);

        if !needs_clamp(ratio) {
            best = ratio;
            low = distance;
        } else {
            high = distance;
        }
    }

    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: usize) -> Node {
        Node::Pane(Pane(id))
    }

    #[test]
    fn resize_stops_when_neighbour_reaches_its_minimum() {
        // Vertical `[A, [B, C]]`, 100px wide, minimum pane size 10px.
        let node = Node::Split {
            id: Split(0),
            axis: Axis::Vertical,
            ratio: 0.5,
            a: Box::new(pane(0)),
            b: Box::new(Node::Split {
                id: Split(1),
                axis: Axis::Vertical,
                ratio: 0.5,
                a: Box::new(pane(1)),
                b: Box::new(pane(2)),
            }),
        };

        let bounds = Size::new(100.0, 100.0);

        // Drag the outer split hard to the right.
        let clamped = clamp_resize(&node, Split(0), 0.9, bounds, 0.0, 10.0, 0.5);

        assert!(clamped < 0.9, "the drag should have been clamped");

        let mut candidate = node.clone();
        assert!(candidate.resize(Split(0), clamped));
        let regions = candidate.pane_regions(0.0, 10.0, bounds);

        // The far pane C stays put, and its neighbour B stops at its minimum.
        assert!((regions[&Pane(2)].width - 25.0).abs() < 1.0);
        assert!(regions[&Pane(1)].width >= 10.0);
    }

    #[test]
    fn resizing_a_split_whose_side_is_a_perpendicular_stack_works() {
        // `[A - [B | C]]`: a horizontal split whose bottom side is a vertical stack.
        // Resizing it moves all three panes, which must still be allowed.
        let node = Node::Split {
            id: Split(0),
            axis: Axis::Horizontal,
            ratio: 0.5,
            a: Box::new(pane(0)),
            b: Box::new(Node::Split {
                id: Split(1),
                axis: Axis::Vertical,
                ratio: 0.5,
                a: Box::new(pane(1)),
                b: Box::new(pane(2)),
            }),
        };

        let bounds = Size::new(100.0, 100.0);

        let clamped = clamp_resize(&node, Split(0), 0.8, bounds, 0.0, 10.0, 0.5);

        assert!(
            clamped > 0.5,
            "the split should be resizable, but was clamped to {clamped}"
        );
    }

    #[test]
    fn hovered_splits_detects_a_corner() {
        // `[A | [B / C]]`: a vertical seam crossing a horizontal seam in the
        // right half, so the two seams meet at (100, 50).
        let node = Node::Split {
            id: Split(0),
            axis: Axis::Vertical,
            ratio: 0.5,
            a: Box::new(pane(0)),
            b: Box::new(Node::Split {
                id: Split(1),
                axis: Axis::Horizontal,
                ratio: 0.5,
                a: Box::new(pane(1)),
                b: Box::new(pane(2)),
            }),
        };

        let splits = node.split_regions(0.0, 0.0, Size::new(200.0, 100.0));

        // On the vertical seam, away from the crossing: only the vertical seam.
        let vertical = hovered_splits(splits.iter(), 10.0, Point::new(100.0, 10.0));
        assert_eq!(vertical.vertical, Some(Split(0)));
        assert_eq!(vertical.horizontal, None);

        // At the crossing: both seams, so a grab resizes both axes at once.
        let corner = hovered_splits(splits.iter(), 10.0, Point::new(100.0, 50.0));
        assert!(corner.is_corner());
        assert_eq!(corner.vertical, Some(Split(0)));
        assert_eq!(corner.horizontal, Some(Split(1)));
    }

    /// A grid of two panes side by side, each with a title bar (and a
    /// drag preview, if `preview`), after a finger presses the left one's
    /// title bar, moves onto the right pane and ends with `end`; the drag
    /// events it published.
    fn pane_drag(preview: bool, end: fn(touch::Finger, Point) -> touch::Event) -> Vec<String> {
        use iced::widget::text;

        let (mut state, left) = State::new(());
        state.split(Axis::Vertical, left, ());

        let grid: PaneGrid<'_, String, _, _> = PaneGrid::new(&state, |_, _, _| {
            let content =
                Content::new(text("Body")).title_bar(TitleBar::new(text("Title")).padding(10));
            if preview {
                content.preview(iced::Widget::boxed(text("Label")))
            } else {
                content
            }
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .on_drag(|event| match event {
            DragEvent::Picked { .. } => "picked".to_string(),
            DragEvent::Dropped { .. } => "dropped".to_string(),
            DragEvent::Canceled { .. } => "canceled".to_string(),
        });

        let finger = |event: fn(touch::Finger, Point) -> touch::Event, at| {
            Event::Touch(event(touch::Finger(0), at))
        };
        let title = Point::new(100.0, 10.0);
        let right = Point::new(800.0, 400.0);

        let mut simulator = iced_test::Simulator::new(iced::Widget::boxed(grid));
        simulator.point_at(title);
        simulator.simulate([finger(
            |id, position| touch::Event::FingerPressed { id, position },
            title,
        )]);
        simulator.point_at(right);
        simulator.simulate([
            finger(
                |id, position| touch::Event::FingerMoved { id, position },
                right,
            ),
            finger(end, right),
        ]);
        simulator.into_messages().collect()
    }

    #[test]
    fn a_lost_finger_cancels_a_pane_drag() {
        assert_eq!(
            pane_drag(false, |id, position| touch::Event::FingerLifted {
                id,
                position
            }),
            ["picked", "dropped"]
        );
        assert_eq!(
            pane_drag(false, |id, position| touch::Event::FingerLost {
                id,
                position
            }),
            ["picked", "canceled"]
        );
    }

    #[test]
    fn a_pane_with_a_preview_is_dragged_by_its_title_bar() {
        assert_eq!(
            pane_drag(true, |id, position| touch::Event::FingerLifted {
                id,
                position
            }),
            ["picked", "dropped"]
        );
    }

    /// A grid of one pane taking a drag from outside, which lands on the
    /// pane wherever it is dropped; the targets it published after `event`.
    fn incoming_drop(event: Event) -> Vec<String> {
        use iced::widget::text;

        let (state, _) = State::new(());
        let grid: PaneGrid<'_, String, _, _> =
            PaneGrid::new(&state, |_, _, _| Content::new(text("Pane")))
                .width(Length::Fill)
                .height(Length::Fill)
                .incoming(
                    |at, regions| {
                        regions.iter().find(|(_, region)| region.contains(at)).map(
                            |(pane, region)| DropHint {
                                target: Target::Pane(*pane, Region::Center),
                                area: *region,
                            },
                        )
                    },
                    |target: Target| format!("{target:?}"),
                );

        let mut simulator = iced_test::Simulator::new(iced::Widget::boxed(grid));
        simulator.point_at(Point::new(100.0, 100.0));
        simulator.simulate([event]);
        simulator.into_messages().collect()
    }

    #[test]
    fn a_drag_from_outside_drops_on_release_or_lift() {
        let at = Point::new(100.0, 100.0);

        let released = incoming_drop(Event::Mouse(mouse::Event::ButtonReleased(
            mouse::Button::Left,
        )));
        assert_eq!(released.len(), 1);

        let lifted = incoming_drop(Event::Touch(touch::Event::FingerLifted {
            id: touch::Finger(0),
            position: at,
        }));
        assert_eq!(lifted, released, "a finger drops like the mouse");

        let lost = incoming_drop(Event::Touch(touch::Event::FingerLost {
            id: touch::Finger(0),
            position: at,
        }));
        assert!(lost.is_empty(), "a finger taken away drops nothing");
    }
}

fn layout_region(layout: Layout, cursor_position: Point) -> Option<Region> {
    let bounds = layout.bounds();

    if !bounds.contains(cursor_position) {
        return None;
    }

    let region = if cursor_position.x < (bounds.x + bounds.width / 3.0) {
        Region::Edge(Edge::Left)
    } else if cursor_position.x > (bounds.x + 2.0 * bounds.width / 3.0) {
        Region::Edge(Edge::Right)
    } else if cursor_position.y < (bounds.y + bounds.height / 3.0) {
        Region::Edge(Edge::Top)
    } else if cursor_position.y > (bounds.y + 2.0 * bounds.height / 3.0) {
        Region::Edge(Edge::Bottom)
    } else {
        Region::Center
    };

    Some(region)
}

fn click_pane<'a, 'b, Message, T>(
    action: &mut state::Action,
    cursor_position: Point,
    shell: &mut Shell<'_, Message>,
    contents: impl Iterator<Item = (Pane, T, Layout, &'b Tree)>,
    on_click: &Option<Box<dyn Fn(Pane) -> Message + 'a>>,
    on_drag: &Option<Box<dyn Fn(DragEvent) -> Message + 'a>>,
) where
    T: Draggable,
{
    let mut clicked_region =
        contents.filter(|(_, _, layout, _)| layout.bounds().contains(cursor_position));

    if let Some((pane, content, layout, tree)) = clicked_region.next() {
        if let Some(on_click) = &on_click {
            shell.publish(on_click(pane));
        }

        if let Some(on_drag) = &on_drag
            && content.can_be_dragged_at(tree, layout, cursor_position)
        {
            *action = state::Action::Dragging {
                pane,
                origin: cursor_position,
            };

            shell.publish(on_drag(DragEvent::Picked { pane }));
        }
    }
}

fn in_edge(layout: Layout, cursor: Point) -> Option<Edge> {
    let bounds = layout.bounds();

    let height_thickness = bounds.height / THICKNESS_RATIO;
    let width_thickness = bounds.width / THICKNESS_RATIO;
    let thickness = height_thickness.min(width_thickness);

    if cursor.x > bounds.x && cursor.x < bounds.x + thickness {
        Some(Edge::Left)
    } else if cursor.x > bounds.x + bounds.width - thickness && cursor.x < bounds.x + bounds.width {
        Some(Edge::Right)
    } else if cursor.y > bounds.y && cursor.y < bounds.y + thickness {
        Some(Edge::Top)
    } else if cursor.y > bounds.y + bounds.height - thickness && cursor.y < bounds.y + bounds.height
    {
        Some(Edge::Bottom)
    } else {
        None
    }
}

fn edge_bounds(layout: Layout, edge: Edge) -> Rectangle {
    let bounds = layout.bounds();

    let height_thickness = bounds.height / THICKNESS_RATIO;
    let width_thickness = bounds.width / THICKNESS_RATIO;
    let thickness = height_thickness.min(width_thickness);

    match edge {
        Edge::Top => Rectangle {
            height: thickness,
            ..bounds
        },
        Edge::Left => Rectangle {
            width: thickness,
            ..bounds
        },
        Edge::Right => Rectangle {
            x: bounds.x + bounds.width - thickness,
            width: thickness,
            ..bounds
        },
        Edge::Bottom => Rectangle {
            y: bounds.y + bounds.height - thickness,
            height: thickness,
            ..bounds
        },
    }
}

fn layout_region_bounds(layout: Layout, region: Region) -> Rectangle {
    let bounds = layout.bounds();

    match region {
        Region::Center => bounds,
        Region::Edge(edge) => match edge {
            Edge::Top => Rectangle {
                height: bounds.height / 2.0,
                ..bounds
            },
            Edge::Left => Rectangle {
                width: bounds.width / 2.0,
                ..bounds
            },
            Edge::Right => Rectangle {
                x: bounds.x + bounds.width / 2.0,
                width: bounds.width / 2.0,
                ..bounds
            },
            Edge::Bottom => Rectangle {
                y: bounds.y + bounds.height / 2.0,
                height: bounds.height / 2.0,
                ..bounds
            },
        },
    }
}

/// An event produced during a drag and drop interaction of a [`PaneGrid`].
#[derive(Debug, Clone, Copy)]
pub enum DragEvent {
    /// A [`Pane`] was picked for dragging.
    Picked {
        /// The picked [`Pane`].
        pane: Pane,
    },

    /// A [`Pane`] was dropped on top of another [`Pane`].
    Dropped {
        /// The picked [`Pane`].
        pane: Pane,

        /// The [`Target`] where the picked [`Pane`] was dropped on.
        target: Target,
    },

    /// A [`Pane`] was picked and then dropped outside of other [`Pane`]
    /// boundaries.
    Canceled {
        /// The picked [`Pane`].
        pane: Pane,
    },
}

/// The [`Target`] area a pane can be dropped on.
#[derive(Debug, Clone, Copy)]
pub enum Target {
    /// An [`Edge`] of the full [`PaneGrid`].
    Edge(Edge),
    /// A single [`Pane`] of the [`PaneGrid`].
    Pane(Pane, Region),
    /// The seam between two adjacent panes, the first before the second.
    /// Only a [custom drop target](PaneGrid::drop_target) produces it.
    Between(Pane, Pane),
}

/// The region of a [`Pane`].
#[derive(Debug, Clone, Copy, Default)]
pub enum Region {
    /// Center region.
    #[default]
    Center,
    /// Edge region.
    Edge(Edge),
}

/// The edges of an area.
#[derive(Debug, Clone, Copy)]
pub enum Edge {
    /// Top edge.
    Top,
    /// Left edge.
    Left,
    /// Right edge.
    Right,
    /// Bottom edge.
    Bottom,
}

/// An event produced during a resize interaction of a [`PaneGrid`].
#[derive(Debug, Clone, Copy)]
pub struct ResizeEvent {
    /// The [`Split`] that is being dragged for resizing.
    pub split: Split,

    /// The new ratio of the [`Split`].
    ///
    /// The ratio is a value in [0, 1], representing the exact position of a
    /// [`Split`] between two panes.
    pub ratio: f32,
}

/*
 * Helpers
 */
/// All seams whose grab zone contains `cursor_position`, at most one per axis.
/// Both axes being present means the cursor is on the crossing of a horizontal
/// and a vertical split, and both can be resized together.
fn hovered_splits<'a>(
    splits: impl Iterator<Item = (&'a Split, &'a (Axis, Rectangle, f32))>,
    spacing: f32,
    cursor_position: Point,
) -> state::ResizeTargets {
    let mut targets = state::ResizeTargets::default();

    for (split, (axis, region, ratio)) in splits {
        let bounds = axis.split_line_bounds(*region, *ratio, spacing);

        if bounds.contains(cursor_position) {
            match axis {
                Axis::Horizontal => {
                    targets.horizontal.get_or_insert(*split);
                }
                Axis::Vertical => {
                    targets.vertical.get_or_insert(*split);
                }
            }
        }
    }

    targets
}

/// The ratio a seam reaches when the cursor is at `cursor_position`.
fn resize_ratio(axis: Axis, region: Rectangle, cursor_position: Point, bounds: Rectangle) -> f32 {
    match axis {
        Axis::Horizontal => {
            let position = cursor_position.y - bounds.y - region.y;

            (position / region.height).clamp(0.0, 1.0)
        }
        Axis::Vertical => {
            let position = cursor_position.x - bounds.x - region.x;

            (position / region.width).clamp(0.0, 1.0)
        }
    }
}

/// The appearance of a [`PaneGrid`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// The color of the dividers drawn in the gaps between panes.
    pub divider: Color,
    /// The appearance of a hovered region highlight.
    pub hovered_region: Highlight,
    /// The appearance of a picked split.
    pub picked_split: Line,
    /// The appearance of a hovered split.
    pub hovered_split: Line,
    /// Drawn over a pane while its preview is dragged (see
    /// [`Content::preview`]): the grid's backdrop, partly transparent,
    /// fades the pane into it.
    pub picked_pane: Color,
}

/// The appearance of a highlight of the [`PaneGrid`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Highlight {
    /// The [`Background`] of the pane region.
    pub background: Background,
    /// The [`Border`] of the pane region.
    pub border: Border,
}

/// A line.
///
/// It is normally used to define the highlight of something, like a split.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    /// The [`Color`] of the [`Line`].
    pub color: Color,
    /// The width of the [`Line`].
    pub width: f32,
}

/// The theme catalog of a [`PaneGrid`].
pub trait Catalog: container::Catalog {
    /// The item class of this [`Catalog`].
    type Class<'a>;

    /// The default class produced by this [`Catalog`].
    fn default<'a>() -> <Self as Catalog>::Class<'a>;

    /// The [`Style`] of a class with the given status.
    fn style(&self, class: &<Self as Catalog>::Class<'_>) -> Style;
}

/// A styling function for a [`PaneGrid`].
///
/// This is just a boxed closure: `Fn(&Theme, Status) -> Style`.
pub type StyleFn<'a, Theme> = Box<dyn Fn(&Theme) -> Style + 'a>;

impl Catalog for Theme {
    type Class<'a> = StyleFn<'a, Self>;

    fn default<'a>() -> StyleFn<'a, Self> {
        Box::new(default)
    }

    fn style(&self, class: &StyleFn<'_, Self>) -> Style {
        class(self)
    }
}

/// The default style of a [`PaneGrid`].
pub fn default(theme: &Theme) -> Style {
    let palette = theme.palette();

    Style {
        divider: palette.background.strong.color,
        hovered_region: Highlight {
            background: Background::Color(Color {
                a: 0.5,
                ..palette.primary.base.color
            }),
            border: Border {
                width: 2.0,
                color: palette.primary.strong.color,
                radius: 0.0.into(),
            },
        },
        hovered_split: Line {
            color: palette.primary.base.color,
            width: 2.0,
        },
        picked_split: Line {
            color: palette.primary.strong.color,
            width: 2.0,
        },
        picked_pane: palette.background.base.color.scale_alpha(0.6),
    }
}
