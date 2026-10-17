//! Two views sharing a column's height by what they need.

use iced::{
    Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Layout, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Meta, Operation, Tree},
    },
};

use super::style::space;

const TOP: usize = 0;
const BOTTOM: usize = 1;

/// `top` above `bottom`, sharing the height by what each needs.
///
/// When both fit, each gets its natural height (the bottom directly under
/// the top, any space left beneath it). When they do not, each keeps what it
/// needs or half the height, whichever is less, and the one that needs more
/// gets the rest: only a view that does not fit is cut short.
///
/// Both should be shrink-height views that scroll themselves when cut short,
/// such as a list or an editor in a `scrollable`.
pub fn split_column<'a, M: 'a>(
    top: impl Widget<M, Theme, Renderer> + 'a,
    bottom: impl Widget<M, Theme, Renderer> + 'a,
) -> Element<'a, M> {
    use iced::Widget as _;

    SplitColumn {
        children: vec![top.boxed(), bottom.boxed()],
        spacing: space::M,
    }
    .boxed()
}

struct SplitColumn<'a, Message> {
    /// `[top, bottom]`.
    children: Vec<Element<'a, Message>>,
    spacing: f32,
}

/// The heights of the top and bottom, given what each needs and the height
/// both share.
fn share(top: f32, bottom: f32, available: f32) -> (f32, f32) {
    if top + bottom <= available {
        return (top, available - top);
    }
    let half = available / 2.0;
    let top = (available - bottom).clamp(top.min(half), top);
    (top, available - top)
}

impl<Message> Meta for SplitColumn<'_, Message> {}

impl<Message> Widget<Message, Theme, Renderer> for SplitColumn<'_, Message> {
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
        let size = limits.width(Length::Fill).height(Length::Fill).resolve(
            Length::Fill,
            Length::Fill,
            Size::ZERO,
        );

        // What each needs: its height with no limit on it.
        let mut needs = [0.0; 2];
        for (index, need) in needs.iter_mut().enumerate() {
            self.children[index].layout(
                &mut tree.children[index],
                renderer,
                &layout::Limits::new(Size::ZERO, Size::new(size.width, f32::INFINITY)),
            );
            *need = tree.children[index].size.height;
        }

        let available = (size.height - self.spacing).max(0.0);
        let (top, bottom) = share(needs[TOP], needs[BOTTOM], available);

        for (index, height) in [(TOP, top), (BOTTOM, bottom)] {
            self.children[index].layout(
                &mut tree.children[index],
                renderer,
                &layout::Limits::new(Size::ZERO, Size::new(size.width, height)),
            );
        }
        tree.children[TOP].translation = Vector::ZERO;
        tree.children[BOTTOM].translation = Vector::new(0.0, top + self.spacing);

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
        for (child, (layout, tree)) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
        {
            child.update(tree, event, layout, cursor, renderer, shell, viewport);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_that_fit_get_what_they_need() {
        assert_eq!(share(100.0, 200.0, 600.0), (100.0, 500.0));
    }

    #[test]
    fn a_short_view_keeps_its_height_beside_a_tall_one() {
        // A short list above a tall editor: the list is not cut.
        assert_eq!(share(100.0, 800.0, 600.0), (100.0, 500.0));
        // A tall list above a short editor: the editor is not cut.
        assert_eq!(share(1000.0, 200.0, 600.0), (400.0, 200.0));
    }

    #[test]
    fn two_tall_views_share_evenly() {
        assert_eq!(share(1000.0, 800.0, 600.0), (300.0, 300.0));
    }
}
