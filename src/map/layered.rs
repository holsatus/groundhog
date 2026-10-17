//! The container stacking the interactive overlay on the map tiles.
//!
//! iced's `Stack` hides the cursor from lower layers ("levitates" it) whenever
//! an upper layer reports a mouse interaction. The overlay reports one while
//! the pointer hovers a handle (to show a grab or pointer cursor), so under a
//! `Stack` the tiles would stop seeing the pointer there, and wheel zoom and
//! panning would stop working over every marker.
//!
//! This container gives the overlay first pick of each event, exactly like a
//! stack, but stops only when the overlay *captures* the event. The tiles
//! always see the real cursor, and the visible cursor is the overlay's when it
//! has one, the tiles' otherwise.

use iced::{
    Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Layout, Renderer as _, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Meta, Operation, Tree},
    },
};

const BASE: usize = 0;
const OVERLAY: usize = 1;

/// The map tiles with the overlay on top. Both fill the available space.
pub(super) struct Layered<'a, Message> {
    /// `[base, overlay]`.
    children: Vec<Element<'a, Message>>,
}

impl<'a, Message> Layered<'a, Message> {
    pub fn new(base: Element<'a, Message>, overlay: Element<'a, Message>) -> Self {
        Self {
            children: vec![base, overlay],
        }
    }
}

impl<Message> Meta for Layered<'_, Message> {}

impl<Message> Widget<Message, Theme, Renderer> for Layered<'_, Message> {
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fill,
            height: Length::Fill,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let limits = limits.width(Length::Fill).height(Length::Fill);

        self.children[BASE].layout(&mut tree.children[BASE], renderer, &limits);
        let size = limits.resolve(Length::Fill, Length::Fill, tree.children[BASE].size);

        self.children[OVERLAY].layout(
            &mut tree.children[OVERLAY],
            renderer,
            &layout::Limits::new(Size::ZERO, size),
        );

        tree.size = size;
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
            self.children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
                .for_each(|(child, (layout, state))| {
                    child.operate(state, layout, viewport, renderer, operation);
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
        // Overlay first; the tiles only lose events the overlay captured
        // (a drag), never just because the overlay shows a cursor.
        for (child, (layout, tree)) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
            .rev()
        {
            child.update(tree, event, layout, cursor, renderer, shell, viewport);

            if shell.is_event_captured() {
                return;
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
        self.children
            .iter()
            .zip(layout.iter(&tree.children))
            .rev()
            .map(|(child, (layout, tree))| {
                child.mouse_interaction(tree, layout, cursor, viewport, renderer)
            })
            .find(|&interaction| interaction != mouse::Interaction::None)
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
        let Some(viewport) = layout.bounds().intersection(viewport) else {
            return;
        };

        let mut layers = self.children.iter().zip(layout.iter(&tree.children));

        if let Some((base, (layout, tree))) = layers.next() {
            base.draw(tree, renderer, theme, style, layout, cursor, &viewport);
        }

        for (overlay, (layout, tree)) in layers {
            renderer.with_layer(viewport, |renderer| {
                overlay.draw(tree, renderer, theme, style, layout, cursor, &viewport);
            });
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
