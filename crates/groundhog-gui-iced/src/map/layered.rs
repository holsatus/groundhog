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
    Event, Length, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Layout, Renderer as _, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Meta, Operation, Tree},
    },
};

const BASE: usize = 0;
const OVERLAY: usize = 1;

/// The map tiles with the overlay on top. Both fill the available space.
pub(super) struct Layered<B, O> {
    base: B,
    overlay: O,
}

impl<B, O> Layered<B, O> {
    pub fn new(base: B, overlay: O) -> Self {
        Self { base, overlay }
    }
}

impl<B, O> Meta for Layered<B, O> {}

impl<B, O, Message> Widget<Message, Theme, Renderer> for Layered<B, O>
where
    B: Widget<Message, Theme, Renderer>,
    O: Widget<Message, Theme, Renderer>,
{
    fn diff(&mut self, tree: &mut Tree) {
        if tree.children.len() != 2 {
            tree.children = vec![Tree::new(&self.base), Tree::new(&self.overlay)];
        }

        tree.children[BASE].diff(&mut self.base);
        tree.children[OVERLAY].diff(&mut self.overlay);
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fill,
            height: Length::Fill,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let limits = limits.width(Length::Fill).height(Length::Fill);

        self.base
            .layout(&mut tree.children[BASE], renderer, &limits);
        let size = limits.resolve(Length::Fill, Length::Fill, tree.children[BASE].size);

        self.overlay.layout(
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
            let mut children = layout.iter_mut(&mut tree.children);
            if let Some((layout, tree)) = children.next() {
                self.base
                    .operate(tree, layout, viewport, renderer, operation);
            }
            if let Some((layout, tree)) = children.next() {
                self.overlay
                    .operate(tree, layout, viewport, renderer, operation);
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
        let mut children = layout.iter_mut(&mut tree.children);
        let (Some((base_layout, base_tree)), Some((overlay_layout, overlay_tree))) =
            (children.next(), children.next())
        else {
            return;
        };

        // Overlay first; the tiles only lose events the overlay captured
        // (a drag), never just because the overlay shows a cursor.
        self.overlay.update(
            overlay_tree,
            event,
            overlay_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );

        if shell.is_event_captured() {
            return;
        }

        self.base.update(
            base_tree,
            event,
            base_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let mut children = layout.iter(&tree.children);
        let (Some((base_layout, base_tree)), Some((overlay_layout, overlay_tree))) =
            (children.next(), children.next())
        else {
            return mouse::Interaction::default();
        };

        match self.overlay.mouse_interaction(
            overlay_tree,
            overlay_layout,
            cursor,
            viewport,
            renderer,
        ) {
            mouse::Interaction::None => {
                self.base
                    .mouse_interaction(base_tree, base_layout, cursor, viewport, renderer)
            }
            interaction => interaction,
        }
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

        let mut children = layout.iter(&tree.children);

        if let Some((layout, tree)) = children.next() {
            self.base
                .draw(tree, renderer, theme, style, layout, cursor, &viewport);
        }

        if let Some((layout, tree)) = children.next() {
            renderer.with_layer(viewport, |renderer| {
                self.overlay
                    .draw(tree, renderer, theme, style, layout, cursor, &viewport);
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
        let mut children = layout.iter_mut(&mut tree.children);
        let mut overlays = Vec::new();

        if let Some((layout, tree)) = children.next() {
            overlays.extend(self.base.overlay(
                tree,
                layout,
                renderer,
                viewport,
                translation,
                window,
            ));
        }
        if let Some((layout, tree)) = children.next() {
            overlays.extend(self.overlay.overlay(
                tree,
                layout,
                renderer,
                viewport,
                translation,
                window,
            ));
        }

        overlays
    }
}
