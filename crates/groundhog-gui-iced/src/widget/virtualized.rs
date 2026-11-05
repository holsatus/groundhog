//! A vertical list which only builds and lays out the items near the visible area.
//!
//! Place it inside a [`scrollable`](fn@iced::widget::scrollable). The list reads
//! the visible region from the viewport it is given during `update`, so it
//! needs no application state or messages.
//!
//! Item heights are either given exactly with [`VirtualList::item_height`],
//! or, by default, measured as items become visible, unseen items assumed to
//! be `DEFAULT_ESTIMATE` tall. With measured heights, the scrollbar and
//! content may shift slightly as items are measured for the first time.

use std::{collections::HashMap, hash::Hash};

use iced::{
    Event, Length, Rectangle, Size, Vector,
    advanced::{
        Layout, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Meta, Operation, Tree, tree},
    },
};

/// Visible height assumed until the list has seen its real viewport.
const INITIAL_HEIGHT: f32 = 2000.0;

/// Pixels to build beyond the visible edges, to avoid pop-in while scrolling.
const OVERSCAN: f32 = 200.0;

/// Default height assumed for items which have not been measured yet.
const DEFAULT_ESTIMATE: f32 = 32.0;

/// Creates a [`VirtualList`] which calls `view` only for the visible items.
pub fn virtual_list<'a, T, W>(
    items: impl IntoIterator<Item = T>,
    view: impl Fn(&T) -> W + 'a,
) -> VirtualList<'a, T, W> {
    VirtualList {
        items: items.into_iter().collect(),
        view: Box::new(view),
        key: Box::new(|index, _| index),
        height: Height::Measured {
            estimate: DEFAULT_ESTIMATE,
        },
        spacing: 0.0,
        keys: Vec::new(),
        children: Vec::new(),
    }
}

enum Height<'a, T> {
    Exact(Box<dyn Fn(&T) -> f32 + 'a>),
    Measured { estimate: f32 },
}

/// Builds the widget for a single item.
type View<'a, T, W> = Box<dyn Fn(&T) -> W + 'a>;

/// Builds the stable key identifying a single item.
type KeyFn<'a, T, K> = Box<dyn Fn(usize, &T) -> K + 'a>;

pub struct VirtualList<'a, T, W, K = usize> {
    items: Vec<T>,
    view: View<'a, T, W>,
    key: KeyFn<'a, T, K>,
    height: Height<'a, T>,
    spacing: f32,

    // Built during layout, since only then is the visible region known
    keys: Vec<K>,
    children: Vec<W>,
}

impl<'a, T, W, K> VirtualList<'a, T, W, K> {
    /// Use exact item heights.
    ///
    /// Each item is laid out within its height and **centered vertically**: the
    /// height is a fixed row slot and the built content is centered inside it,
    /// so the height must fit the item's content or it will be clipped. This
    /// centering is part of the contract of this method, not an implementation
    /// detail.
    pub fn item_height(mut self, height: impl Fn(&T) -> f32 + 'a) -> Self {
        self.height = Height::Exact(Box::new(height));
        self
    }

    /// Identify items by key rather than by index, so widget state (such as
    /// text input focus) and measured heights follow the item when the list
    /// changes.
    pub fn key<K2>(self, key: impl Fn(&T) -> K2 + 'a) -> VirtualList<'a, T, W, K2> {
        VirtualList {
            items: self.items,
            view: self.view,
            key: Box::new(move |_, item| key(item)),
            height: self.height,
            spacing: self.spacing,
            keys: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Vertical spacing between items.
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }
}

struct State<K> {
    /// Visible region `(top, height)` relative to the list, from the last event
    visible: Option<(f32, f32)>,
    /// Region `(top, bottom)` covered by the current children
    built: (f32, f32),
    total: f32,
    keys: Vec<K>,
    measured: HashMap<K, f32>,
    /// Width the measured heights are valid for
    width: f32,
}

impl<T, W, K> Meta for VirtualList<'_, T, W, K> {}

impl<T, W, K, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for VirtualList<'_, T, W, K>
where
    W: Widget<Message, Theme, Renderer>,
    K: Hash + Eq + Clone + 'static,
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State<K>>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::<K> {
            visible: None,
            built: (0.0, 0.0),
            total: 0.0,
            keys: Vec::new(),
            measured: HashMap::new(),
            width: 0.0,
        })
    }

    fn diff(&mut self, _tree: &mut Tree) {
        // Deferred to layout, where the children are built
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let Tree {
            state,
            children,
            size: tree_size,
            ..
        } = tree;
        let state = state.downcast_mut::<State<K>>();

        let width = limits.max.width;
        if width != state.width {
            state.measured.clear();
            state.width = width;
        }

        let heights: Vec<f32> = self
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| match &self.height {
                Height::Exact(height) => height(item),
                Height::Measured { estimate } => state
                    .measured
                    .get(&(self.key)(index, item))
                    .copied()
                    .unwrap_or(*estimate),
            })
            .collect();

        let estimated_total =
            heights.iter().sum::<f32>() + self.spacing * heights.len().saturating_sub(1) as f32;

        // The list may have shrunk since the last event, in which case the
        // scrollable will clamp its offset to the new bottom.
        let (offset, height) = state.visible.unwrap_or((0.0, INITIAL_HEIGHT));
        let offset = offset.min((estimated_total - height).max(0.0));

        let top = offset - OVERSCAN;
        let bottom = offset + height + OVERSCAN;

        let mut old: HashMap<K, Tree> = std::mem::take(&mut state.keys)
            .into_iter()
            .zip(std::mem::take(children))
            .collect();

        self.keys.clear();
        self.children.clear();
        let mut y = 0.0;

        for (index, (item, mut item_height)) in self.items.iter().zip(heights).enumerate() {
            if y + item_height >= top && y <= bottom {
                let key = (self.key)(index, item);
                let mut child = (self.view)(item);

                let mut child_tree = old.remove(&key).unwrap_or_else(|| Tree::new(&child));
                child_tree.diff(&mut child);

                match &self.height {
                    Height::Exact(_) => {
                        let limits = layout::Limits::new(Size::ZERO, Size::new(width, item_height));
                        child.layout(&mut child_tree, renderer, &limits);
                        let centered = ((item_height - child_tree.size.height) / 2.0).max(0.0);
                        child_tree.translation = Vector::new(0.0, y + centered);
                    }
                    Height::Measured { .. } => {
                        let limits =
                            layout::Limits::new(Size::ZERO, Size::new(width, f32::INFINITY));
                        child.layout(&mut child_tree, renderer, &limits);
                        item_height = child_tree.size.height;
                        state.measured.insert(key.clone(), item_height);
                        child_tree.translation = Vector::new(0.0, y);
                    }
                }

                children.push(child_tree);
                self.keys.push(key);
                self.children.push(child);
            }

            y += item_height + self.spacing;
        }

        let total = (y - self.spacing).max(0.0);

        state.keys.clone_from(&self.keys);
        state.built = (top, bottom);
        state.total = total;

        *tree_size = Size::new(width, total);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            for (child, (layout, tree)) in self
                .children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
            {
                child.operate(tree, layout, viewport, renderer, operation);
            }
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
        for (child, (layout, tree)) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
        {
            child.update(tree, event, layout, cursor, renderer, shell, viewport);
        }

        let state = tree.state.downcast_mut::<State<K>>();
        let top = viewport.y - layout.bounds().y;
        let bottom = top + viewport.height;
        state.visible = Some((top, viewport.height));

        // Only rebuild once the overscan is used up, and there is more to show
        let (built_top, built_bottom) = state.built;
        let missing_above = top < built_top && built_top > 0.0;
        let missing_below = bottom > built_bottom && built_bottom < state.total;

        if missing_above || missing_below {
            shell.invalidate_layout();
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
        self.children
            .iter()
            .zip(layout.iter(&tree.children))
            .map(|(child, (layout, tree))| {
                child.mouse_interaction(tree, layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        for (child, (layout, tree)) in self.children.iter().zip(layout.iter(&tree.children)) {
            child.draw(tree, renderer, theme, style, layout, cursor, viewport);
        }
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
        overlay::from_children(
            &mut self.children,
            tree,
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}
