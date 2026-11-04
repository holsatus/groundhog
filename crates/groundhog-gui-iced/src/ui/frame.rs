//! A pane's rounded corners and border, drawn over content that paints its
//! whole rectangle (the map's tiles, a video's frame), which a rounded
//! container cannot clip.
//!
//! The corners are painted in the window's background, outside the pane,
//! and the border stroked inside the bounds, as [`style::pane`] draws them
//! around any other pane.

use iced::{
    Color, Point, Rectangle, Renderer, Size, Theme,
    border::Radius,
    mouse,
    widget::canvas::{self, Canvas, Fill, Frame, Geometry, LineJoin, Path, Stroke, Style, fill},
};

use super::style::{PANE_BORDER_WIDTH, PANE_RADIUS};

/// Which edges of the pane the content reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edges {
    /// All four: a pane of its own (the map).
    All,
    /// All but the top, where the pane's title bar is: the top corners are
    /// the title bar's, and its edge is not the pane's border.
    BelowTitle,
}

/// Draw the frame of a pane whose content fills `size`, the corners in
/// `background` and the border in `border`.
pub fn draw(frame: &mut Frame, size: Size, edges: Edges, background: Color, border: Color) {
    const EXPAND: f32 = 1.0;

    let radius = PANE_RADIUS;
    let width = PANE_BORDER_WIDTH;
    let half_width = width / 2.0;
    let rounded = |radius: f32| match edges {
        Edges::All => Radius::new(radius),
        Edges::BelowTitle => Radius::default().bottom(radius),
    };
    // The top edge, when it is the title bar's, is neither painted over nor
    // stroked.
    let top = match edges {
        Edges::All => EXPAND,
        Edges::BelowTitle => 0.0,
    };

    // Rectangle minus rounded rectangle, even-odd: exactly the corners.
    let corners = Path::new(|builder| {
        builder.rectangle(
            Point::new(-EXPAND, -top),
            Size::new(size.width + EXPAND * 2.0, size.height + EXPAND + top),
        );
        builder.rounded_rectangle(
            Point::new(half_width, half_width),
            Size::new(size.width - width, size.height - width),
            rounded(radius - half_width),
        );
    });
    frame.fill(
        &corners,
        Fill {
            style: Style::Solid(background),
            rule: fill::Rule::EvenOdd,
        },
    );

    // Inset by half the stroke so the border stays inside the bounds; a
    // canvas is not clipped to them.
    let inset = half_width;
    let radius = (radius - inset).max(0.0);
    let (right, bottom) = (size.width - inset, size.height - inset);
    let outline = match edges {
        Edges::All => Path::rounded_rectangle(
            Point::new(inset, inset),
            Size::new(size.width - width, size.height - width),
            rounded(radius),
        ),
        Edges::BelowTitle => Path::new(|builder| {
            builder.move_to(Point::new(inset, 0.0));
            builder.line_to(Point::new(inset, bottom - radius));
            builder.arc_to(
                Point::new(inset, bottom),
                Point::new(inset + radius, bottom),
                radius,
            );
            builder.line_to(Point::new(right - radius, bottom));
            builder.arc_to(
                Point::new(right, bottom),
                Point::new(right, bottom - radius),
                radius,
            );
            builder.line_to(Point::new(right, 0.0));
        }),
    };
    frame.stroke(
        &outline,
        Stroke {
            style: Style::Solid(border),
            width,
            line_join: LineJoin::Round,
            ..Stroke::default()
        },
    );
}

/// The colours [`draw`] takes in `theme`: the window's background around the
/// pane, and the pane's border.
pub fn colours(theme: &Theme) -> (Color, Color) {
    let palette = theme.palette();
    (
        palette.background.base.color,
        palette.background.strongest.color,
    )
}

/// A layer that draws the frame of a pane body below its title bar, over
/// whatever lies under it. It takes no input: what is under it keeps its
/// presses.
pub fn below_title<M>() -> Canvas<BelowTitle, M> {
    Canvas::new(BelowTitle)
        .width(iced::Length::Fill)
        .height(iced::Length::Fill)
}

/// See [`below_title`].
pub struct BelowTitle;

impl<M> canvas::Program<M> for BelowTitle {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let (background, border) = colours(theme);
        draw(
            &mut frame,
            bounds.size(),
            Edges::BelowTitle,
            background,
            border,
        );
        vec![frame.into_geometry()]
    }
}
