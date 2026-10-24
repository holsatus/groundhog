//! Layouts that rearrange their children by the width they are given.
//!
//! A panel is shown in a 500-pixel pane on the desktop and in a 360-pixel
//! sheet on a phone, at whatever scale the operator chose. These decide by
//! the width they are offered, not by the device, so a narrow desktop pane
//! and a phone get the same arrangement:
//!
//! - [`form_row`]: a label beside its control while both fit, above it
//!   otherwise;
//! - [`grid`]: cells in as many columns as fit, up to a maximum;
//! - [`header`]: a subject, its status and its tools on one line, the tools
//!   folded into a menu when they would squeeze the subject.
//!
//! They decide during layout, from the limits they are given, so the view
//! that builds them does not need to know the width (unlike iced's
//! `responsive`, which rebuilds its content on every layout).

use iced::{
    Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Layout, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Meta, Operation, Tree, tree},
    },
};

/// `label` beside `control` in a column `label_width` wide while the
/// control keeps at least `min_control` beside it, and above it otherwise,
/// `spacing` apart either way. The row fills its width.
pub fn form_row<'a, M: 'a>(
    label: impl Widget<M, Theme, Renderer> + 'a,
    control: impl Widget<M, Theme, Renderer> + 'a,
    label_width: f32,
    min_control: f32,
    spacing: f32,
) -> Adaptive<'a, M> {
    use iced::Widget as _;

    Adaptive {
        children: vec![label.boxed(), control.boxed()],
        arrangement: Arrangement::FormRow {
            label_width,
            min_control,
        },
        spacing,
    }
}

/// `cells` in rows of as many columns as fit cells at least `min_cell`
/// wide, and at most `max_columns`; the cells of a row share its width
/// equally, `spacing` apart. The grid fills its width.
pub fn grid<'a, M: 'a>(
    cells: impl IntoIterator<Item = Element<'a, M>>,
    min_cell: f32,
    max_columns: usize,
    spacing: f32,
) -> Adaptive<'a, M> {
    Adaptive {
        children: cells.into_iter().collect(),
        arrangement: Arrangement::Grid {
            min_cell,
            max_columns: max_columns.max(1),
        },
        spacing,
    }
}

/// `subject` filling a line at least `height` tall, with `status` and then
/// `tools` at its end, `spacing` apart. Where `tools` would leave the
/// subject less than `min_subject`, `menu` (a button opening the same tools)
/// takes their place.
pub fn header<'a, M: 'a>(
    subject: impl Widget<M, Theme, Renderer> + 'a,
    status: impl Widget<M, Theme, Renderer> + 'a,
    tools: impl Widget<M, Theme, Renderer> + 'a,
    menu: impl Widget<M, Theme, Renderer> + 'a,
    min_subject: f32,
    height: f32,
    spacing: f32,
) -> Adaptive<'a, M> {
    use iced::Widget as _;

    Adaptive {
        children: vec![subject.boxed(), status.boxed(), tools.boxed(), menu.boxed()],
        arrangement: Arrangement::Header {
            min_subject,
            height,
        },
        spacing,
    }
}

pub struct Adaptive<'a, Message> {
    children: Vec<Element<'a, Message>>,
    arrangement: Arrangement,
    spacing: f32,
}

#[derive(Debug, Clone, Copy)]
enum Arrangement {
    FormRow { label_width: f32, min_control: f32 },
    Grid { min_cell: f32, max_columns: usize },
    Header { min_subject: f32, height: f32 },
}

/// The child the last layout left out, if any: it is not drawn and sees no
/// events.
#[derive(Debug, Default)]
struct Hidden(Option<usize>);

/// Whether a header of `width` keeps its tools beside a subject at least
/// `min_subject` wide, given the widths of its status and tools.
fn tools_fit(width: f32, status: f32, tools: f32, min_subject: f32, spacing: f32) -> bool {
    let gap = |width: f32| if width > 0.0 { spacing } else { 0.0 };
    width - status - gap(status) - tools - gap(tools) >= min_subject
}

/// Whether a form row of `width` puts its label beside the control.
fn beside(width: f32, label_width: f32, min_control: f32, spacing: f32) -> bool {
    width >= label_width + spacing + min_control
}

/// How many columns of cells at least `min_cell` wide, `spacing` apart, fit
/// in `width`: at least one, at most `max_columns`.
fn columns(width: f32, min_cell: f32, max_columns: usize, spacing: f32) -> usize {
    if !width.is_finite() {
        return max_columns;
    }
    let fit = ((width + spacing) / (min_cell + spacing)).floor() as usize;
    fit.clamp(1, max_columns)
}

/// Limits no wider than `width`, as tall as `limits` allow. Inside a
/// scrollable the height is unbounded, its maximum only a hint (the
/// viewport); that is kept, or content near the end of a scrolled area would
/// be squeezed into what is left of the viewport.
fn column_limits(limits: &layout::Limits, width: f32) -> layout::Limits {
    layout::Limits::with_flags(
        Size::ZERO,
        Size::new(width, limits.max.height),
        Size::new(false, limits.compression.height),
        Size::new(false, limits.infinite.height),
    )
}

impl<Message> Meta for Adaptive<'_, Message> {}

impl<'a, Message> Adaptive<'a, Message> {
    /// The children shown, with their layouts and trees.
    fn shown<'b>(
        &'b self,
        tree: &'b Tree,
        layout: Layout,
    ) -> impl Iterator<Item = (&'b Element<'a, Message>, (Layout, &'b Tree))> + 'b
    where
        'a: 'b,
    {
        let hidden = tree.state.downcast_ref::<Hidden>().0;
        self.children
            .iter()
            .zip(layout.iter(&tree.children))
            .enumerate()
            .filter(move |(index, _)| Some(*index) != hidden)
            .map(|(_, child)| child)
    }
}

impl<Message> Widget<Message, Theme, Renderer> for Adaptive<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Hidden>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(Hidden::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let limits = limits.width(Length::Fill);
        let width = limits.bounds().width;
        let spacing = self.spacing;
        tree.state.downcast_mut::<Hidden>().0 = None;

        match self.arrangement {
            Arrangement::FormRow {
                label_width,
                min_control,
            } => {
                // `form_row` builds exactly these two.
                let mut children = self.children.iter_mut();
                let (Some(label), Some(control)) = (children.next(), children.next()) else {
                    tree.size = Size::ZERO;
                    return;
                };

                if beside(width, label_width, min_control, spacing) {
                    label.layout(
                        &mut tree.children[0],
                        renderer,
                        &column_limits(&limits, label_width),
                    );
                    let control_width = width - label_width - spacing;
                    control.layout(
                        &mut tree.children[1],
                        renderer,
                        &column_limits(&limits, control_width),
                    );

                    let label = tree.children[0].size;
                    let control = tree.children[1].size;
                    let height = label.height.max(control.height);

                    tree.children[0].translation = Vector::new(0.0, (height - label.height) / 2.0);
                    tree.children[1].translation =
                        Vector::new(label_width + spacing, (height - control.height) / 2.0);
                    tree.size = Size::new(
                        if width.is_finite() {
                            width
                        } else {
                            label_width + spacing + control.width
                        },
                        height,
                    );
                } else {
                    label.layout(
                        &mut tree.children[0],
                        renderer,
                        &column_limits(&limits, width),
                    );
                    control.layout(
                        &mut tree.children[1],
                        renderer,
                        &column_limits(&limits, width),
                    );

                    let label = tree.children[0].size;
                    let control = tree.children[1].size;

                    tree.children[0].translation = Vector::ZERO;
                    tree.children[1].translation = Vector::new(0.0, label.height + spacing);
                    tree.size = Size::new(width, label.height + spacing + control.height);
                }
            }
            Arrangement::Grid {
                min_cell,
                max_columns,
            } => {
                let columns = columns(width, min_cell, max_columns, spacing);
                let cell = if width.is_finite() {
                    (width - spacing * (columns - 1) as f32) / columns as f32
                } else {
                    min_cell
                };

                let mut y = 0.0;
                let mut index = 0;
                for row in self.children.chunks_mut(columns) {
                    let mut height: f32 = 0.0;
                    for (column, child) in row.iter_mut().enumerate() {
                        let tree = &mut tree.children[index + column];
                        child.layout(tree, renderer, &column_limits(&limits, cell));
                        tree.translation = Vector::new(column as f32 * (cell + spacing), y);
                        height = height.max(tree.size.height);
                    }
                    index += row.len();
                    y += height + spacing;
                }

                let height = (y - spacing).max(0.0);
                let width = if width.is_finite() {
                    width
                } else {
                    columns as f32 * (cell + spacing) - spacing
                };
                tree.size = Size::new(width, height);
            }
            Arrangement::Header {
                min_subject,
                height,
            } => {
                let loose = column_limits(&limits, width);
                let gap = |width: f32| if width > 0.0 { spacing } else { 0.0 };

                self.children[1].layout(&mut tree.children[1], renderer, &loose);
                self.children[2].layout(&mut tree.children[2], renderer, &loose);
                let status = tree.children[1].size.width;
                let tools = tree.children[2].size.width;

                let (shown, hidden) = if !width.is_finite()
                    || tools_fit(width, status, tools, min_subject, spacing)
                {
                    (2, 3)
                } else {
                    self.children[3].layout(&mut tree.children[3], renderer, &loose);
                    (3, 2)
                };
                tree.state.downcast_mut::<Hidden>().0 = Some(hidden);
                tree.children[hidden].size = Size::ZERO;

                let actions = tree.children[shown].size.width;
                let subject = if width.is_finite() {
                    (width - status - gap(status) - actions - gap(actions)).max(0.0)
                } else {
                    min_subject
                };
                self.children[0].layout(
                    &mut tree.children[0],
                    renderer,
                    &column_limits(&limits, subject),
                );

                let height = [0, 1, shown]
                    .into_iter()
                    .map(|index| tree.children[index].size.height)
                    .fold(height, f32::max);
                let x = [
                    0.0,
                    subject + gap(status),
                    subject + gap(status) + status + gap(actions),
                ];
                for (index, x) in [0, 1, shown].into_iter().zip(x) {
                    let child = &mut tree.children[index];
                    child.translation = Vector::new(x, (height - child.size.height) / 2.0);
                }

                let width = if width.is_finite() {
                    width
                } else {
                    x[2] + actions
                };
                tree.size = Size::new(width, height);
            }
        }
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let hidden = tree.state.downcast_ref::<Hidden>().0;
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            for (index, (child, (layout, tree))) in self
                .children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
                .enumerate()
            {
                if Some(index) != hidden {
                    child.operate(tree, layout, viewport, renderer, operation);
                }
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
        let hidden = tree.state.downcast_ref::<Hidden>().0;
        for (index, (child, (layout, tree))) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
            .enumerate()
        {
            if Some(index) != hidden {
                child.update(tree, event, layout, cursor, renderer, shell, viewport);
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
        self.shown(tree, layout)
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
        for (child, (layout, tree)) in self.shown(tree, layout) {
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
        let hidden = tree.state.downcast_ref::<Hidden>().0;
        self.children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
            .enumerate()
            .filter(|(index, _)| Some(*index) != hidden)
            .flat_map(|(_, (child, (layout, tree)))| {
                child.overlay(tree, layout, renderer, viewport, translation, window)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::{
        Point, Widget as _,
        widget::{button, space, text},
    };

    #[derive(Debug, Clone, PartialEq)]
    enum Message {
        Pressed,
    }

    #[test]
    fn a_form_row_stacks_when_the_control_would_be_squeezed() {
        assert!(beside(500.0, 140.0, 160.0, 6.0));
        assert!(beside(306.0, 140.0, 160.0, 6.0));
        assert!(!beside(305.0, 140.0, 160.0, 6.0));
    }

    #[test]
    fn a_header_folds_its_tools_when_they_squeeze_the_subject() {
        // 120 of status, 100 of tools, a subject of at least 140.
        assert!(tools_fit(400.0, 120.0, 100.0, 140.0, 6.0));
        assert!(!tools_fit(300.0, 120.0, 100.0, 140.0, 6.0));
        // No status takes no gap.
        assert!(tools_fit(246.0, 0.0, 100.0, 140.0, 6.0));
    }

    #[test]
    fn cells_keep_the_unbounded_height_of_a_scrollable() {
        let scrolled = layout::Limits::with_flags(
            Size::ZERO,
            Size::new(300.0, 10.0),
            Size::new(false, true),
            Size::new(false, true),
        );

        let cell = column_limits(&scrolled, 100.0);

        assert_eq!(cell.bounds(), Size::new(100.0, f32::INFINITY));
    }

    #[test]
    fn a_grid_takes_as_many_columns_as_fit() {
        assert_eq!(columns(500.0, 120.0, 3, 8.0), 3);
        assert_eq!(columns(300.0, 120.0, 3, 8.0), 2);
        assert_eq!(columns(100.0, 120.0, 3, 8.0), 1, "never none");
        assert_eq!(columns(f32::INFINITY, 120.0, 3, 8.0), 3);
    }

    /// A form row in a window `width` wide, its control a button 100 tall,
    /// put through a press at `at`.
    fn press_control(width: f32, at: Point) -> Vec<Message> {
        let row = form_row(
            text("Label"),
            button(space().height(100.0))
                .width(Length::Fill)
                .on_press(Message::Pressed),
            140.0,
            160.0,
            6.0,
        );
        let mut simulator = iced_test::Simulator::with_size(
            Default::default(),
            Size::new(width, 400.0),
            row.boxed(),
        );
        simulator.point_at(at);
        simulator.simulate([Event::Mouse(mouse::Event::ButtonPressed(
            mouse::Button::Left,
        ))]);
        simulator.simulate([Event::Mouse(mouse::Event::ButtonReleased(
            mouse::Button::Left,
        ))]);
        simulator.into_messages().collect()
    }

    #[test]
    fn the_control_moves_with_the_arrangement() {
        // Wide: the control starts after the label column.
        let beside_label = Point::new(200.0, 50.0);
        assert_eq!(press_control(600.0, beside_label), [Message::Pressed]);
        assert!(press_control(600.0, Point::new(50.0, 50.0)).is_empty());

        // Narrow: the control spans the width, below the label.
        let below_label = Point::new(50.0, 80.0);
        assert_eq!(press_control(300.0, below_label), [Message::Pressed]);
    }
}
