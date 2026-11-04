use super::{Draggable, TitleBar};
use iced::advanced::layout;
use iced::advanced::mouse;
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::{self, Tree, Void};
use iced::advanced::{self as core, Layout, Shell, Widget};
use iced::border::Radius;
use iced::widget::container;
use iced::{Background, Border, Color, Element, Event, Point, Rectangle, Size, Vector};

/// The content of a [`Pane`].
///
/// [`Pane`]: super::Pane
pub struct Content<'a, Message, T, W, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Theme: container::Catalog,
    Renderer: core::Renderer,
{
    title_bar: Option<TitleBar<'a, Message, T, Theme, Renderer>>,
    body: W,
    /// Drawn at the cursor while the pane is dragged (see [`Content::preview`]).
    preview: Option<Element<'a, Message, Theme, Renderer>>,
    class: Theme::Class<'a>,
}

impl<'a, Message, W, Theme, Renderer> Content<'a, Message, Void, W, Theme, Renderer>
where
    Theme: container::Catalog,
    Renderer: core::Renderer,
{
    /// Creates a new [`Content`] with the provided body.
    pub fn new(body: W) -> Self {
        Self {
            title_bar: None,
            body,
            preview: None,
            class: Theme::default(),
        }
    }
}

impl<'a, Message, T, W, Theme, Renderer> Content<'a, Message, T, W, Theme, Renderer>
where
    Theme: container::Catalog,
    Renderer: core::Renderer,
{
    /// Sets the [`TitleBar`] of the [`Content`].
    pub fn title_bar<Title>(
        self,
        title_bar: TitleBar<'a, Message, Title, Theme, Renderer>,
    ) -> Content<'a, Message, Title, W, Theme, Renderer> {
        Content {
            title_bar: Some(title_bar),
            body: self.body,
            preview: self.preview,
            class: self.class,
        }
    }

    /// Sets what follows the cursor while the pane is dragged, in place of
    /// the pane itself: a label for it, say. The pane then stays drawn where
    /// it is until it is dropped.
    #[must_use]
    pub fn preview(mut self, preview: impl Widget<Message, Theme, Renderer> + 'a) -> Self {
        self.preview = Some(iced::Widget::boxed(preview));
        self
    }

    /// Sets the style of the [`Content`].
    #[must_use]
    pub fn style(mut self, style: impl Fn(&Theme) -> container::Style + 'a) -> Self
    where
        Theme::Class<'a>: From<container::StyleFn<'a, Theme>>,
    {
        self.class = (Box::new(style) as container::StyleFn<'a, Theme>).into();
        self
    }

    /// Sets the style class of the [`Content`].
    #[must_use]
    pub fn class(mut self, class: impl Into<Theme::Class<'a>>) -> Self {
        self.class = class.into();
        self
    }
}

impl<Message, T, W, Theme, Renderer> Content<'_, Message, T, W, Theme, Renderer>
where
    Theme: container::Catalog,
    Renderer: core::Renderer,
    T: Widget<Message, Theme, Renderer>,
    W: Widget<Message, Theme, Renderer>,
{
    /// `[body, title bar, preview]`, an empty tree standing in for what is
    /// not there.
    pub(super) fn state(&self) -> Tree {
        let title_bar = self
            .title_bar
            .as_ref()
            .map_or_else(Tree::empty, TitleBar::state);
        let preview = self.preview.as_ref().map_or_else(Tree::empty, Tree::new);
        let children = vec![Tree::new(&self.body), title_bar, preview];

        Tree {
            children,
            ..Tree::empty()
        }
    }

    pub(super) fn diff(&mut self, tree: &mut Tree) {
        if tree.children.len() == 3 {
            if let Some(title_bar) = &mut self.title_bar {
                title_bar.diff(&mut tree.children[1]);
            }

            tree.children[0].diff(&mut self.body);

            match &mut self.preview {
                Some(preview) => tree.children[2].diff(preview),
                None => tree.children[2] = Tree::empty(),
            }
        } else {
            *tree = self.state();
        }
    }

    /// Draw only the title bar, on the pane's surface and in its border:
    /// the compact preview of a pane being dragged. Returns `false`, drawing
    /// nothing, for a pane without a title bar.
    pub(super) fn draw_title_bar(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        viewport: &Rectangle,
    ) -> bool {
        let Some(title_bar) = &self.title_bar else {
            return false;
        };
        let Some((title_bar_layout, title_bar_tree)) = layout.iter(&tree.children).nth(1) else {
            return false;
        };

        let surface = theme.style(&self.class);
        container::draw_background(renderer, &surface, title_bar_layout.bounds());
        title_bar.draw(
            title_bar_tree,
            renderer,
            theme,
            style,
            title_bar_layout,
            mouse::Cursor::Unavailable,
            viewport,
            false,
        );
        true
    }

    /// Whether the pane has a [`Content::preview`].
    pub(super) fn has_preview(&self) -> bool {
        self.preview.is_some()
    }

    /// Draw the [`Content::preview`] `lift` above `pointer`, inside the
    /// `window`. Returns `false`, drawing nothing, for a pane without one.
    #[expect(
        clippy::too_many_arguments,
        reason = "a widget's draw arguments, plus where to put it"
    )]
    pub(super) fn draw_preview(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        pointer: Point,
        lift: f32,
        window: Size,
    ) -> bool {
        let Some(preview) = &self.preview else {
            return false;
        };

        crate::widget::drag_preview::draw(
            preview,
            &tree.children[2],
            renderer,
            theme,
            style,
            pointer,
            lift,
            window,
        );
        true
    }

    /// Draws the [`Content`] with the provided [`Renderer`] and [`Layout`],
    /// and, given `corners`, paints that colour over the pane's corners,
    /// outside its rounded outline (see [`corner_mask`]).
    ///
    /// [`Renderer`]: core::Renderer
    #[allow(
        clippy::too_many_arguments,
        reason = "a public signature kept close to upstream"
    )]
    pub fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        resizing: bool,
        corners: Option<Color>,
    ) {
        let bounds = layout.bounds();
        let surface = theme.style(&self.class);
        let mask = corners.and_then(|backdrop| corner_mask(bounds, surface.border, backdrop));

        // With a mask, the border is drawn only over the contents, unsnapped
        // as the mask is (see `corner_mask`): drawn here too, snapped, it
        // would show beside that one.
        if mask.is_some() {
            let border = Border {
                width: 0.0,
                ..surface.border
            };
            container::draw_background(renderer, &container::Style { border, ..surface }, bounds);
        } else {
            container::draw_background(renderer, &surface, bounds);
        }

        if let Some(title_bar) = &self.title_bar {
            let mut children = layout.iter(&tree.children);
            let (body_layout, body_tree) = children.next().unwrap();
            let (title_bar_layout, title_bar_tree) = children.next().unwrap();

            // Hide the controls while a split is being dragged, so they do not
            // flicker as the cursor passes over the panes beside the divider.
            let show_controls = !resizing && cursor.is_over(bounds);

            self.body.draw(
                body_tree,
                renderer,
                theme,
                style,
                body_layout,
                cursor,
                viewport,
            );

            title_bar.draw(
                title_bar_tree,
                renderer,
                theme,
                style,
                title_bar_layout,
                cursor,
                viewport,
                show_controls,
            );
        } else {
            self.body.draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                viewport,
            );
        }

        // Paint the corners and re-stroke the pane outline over the contents.
        // The surface is drawn first, so a shaded header (drawn last) would
        // otherwise hide the top of the border, and a body that paints its
        // whole rectangle (the map, a video) its corners; and a body that
        // clips itself (a scrollable, the map canvas) pushes its own renderer
        // layer, so these need a layer of their own, created here, after
        // those, to stay on top.
        if mask.is_some() || surface.border.width > 0.0 {
            renderer.with_layer(bounds, |renderer| {
                if let Some(mask) = mask {
                    renderer.fill_quad(mask, Background::Color(Color::TRANSPARENT));
                }
                if surface.border.width > 0.0 {
                    let outline = renderer::Quad {
                        bounds,
                        border: surface.border,
                        ..renderer::Quad::default()
                    };
                    let outline = renderer::Quad {
                        snap: outline.snap && mask.is_none(),
                        ..outline
                    };
                    renderer.fill_quad(outline, Background::Color(Color::TRANSPARENT));
                }
            });
        }
    }

    pub(crate) fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        // The preview's size, as it would be alone; it is placed when drawn.
        if let Some(preview) = &mut self.preview {
            preview.layout(
                &mut tree.children[2],
                renderer,
                &layout::Limits::new(Size::ZERO, Size::INFINITE),
            );
        }

        if let Some(title_bar) = &mut self.title_bar {
            let max_size = limits.max;

            title_bar.layout(
                &mut tree.children[1],
                renderer,
                &layout::Limits::new(Size::ZERO, max_size),
            );

            let title_bar_size = tree.children[1].size;

            self.body.layout(
                &mut tree.children[0],
                renderer,
                &layout::Limits::new(
                    Size::ZERO,
                    Size::new(max_size.width, max_size.height - title_bar_size.height),
                ),
            );

            tree.children[1].translation = Vector::ZERO;
            tree.children[0].translation = Vector::new(0.0, title_bar_size.height);

            tree.size = max_size;
        } else {
            self.body.layout(&mut tree.children[0], renderer, limits);

            tree.size = tree.children[0].size;
        }
    }

    pub(crate) fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        let (body_layout, body_tree) = if let Some(title_bar) = &mut self.title_bar {
            let mut children = layout.iter_mut(&mut tree.children);
            let (body_layout, body_tree) = children.next().unwrap();
            let (title_bar_layout, title_bar_tree) = children.next().unwrap();

            title_bar.operate(
                title_bar_tree,
                title_bar_layout,
                viewport,
                renderer,
                operation,
            );

            (body_layout, body_tree)
        } else {
            layout.iter_mut(&mut tree.children).next().unwrap()
        };

        self.body
            .operate(body_tree, body_layout, viewport, renderer, operation);
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "a public signature kept identical to upstream"
    )]
    pub(crate) fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
        is_picked: bool,
    ) {
        let (body_layout, body_tree) = if let Some(title_bar) = &mut self.title_bar {
            let mut children = layout.iter_mut(&mut tree.children);
            let (body_layout, body_tree) = children.next().unwrap();
            let (title_bar_layout, title_bar_tree) = children.next().unwrap();

            title_bar.update(
                title_bar_tree,
                event,
                title_bar_layout,
                cursor,
                renderer,
                shell,
                viewport,
            );

            (body_layout, body_tree)
        } else {
            layout.iter_mut(&mut tree.children).next().unwrap()
        };

        // A picked pane's body is not pointed at, but it keeps the rest
        // (redraws, in particular): it stays on screen, live, while a
        // preview is dragged in its stead.
        let pointer = matches!(event, Event::Mouse(_) | Event::Touch(_));
        if !(is_picked && pointer) {
            let cursor = if is_picked {
                mouse::Cursor::Unavailable
            } else {
                cursor
            };
            self.body.update(
                body_tree,
                event,
                body_layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
        }
    }

    pub(crate) fn grid_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        drag_enabled: bool,
    ) -> Option<mouse::Interaction> {
        let title_bar = self.title_bar.as_ref()?;

        let (title_bar_layout, _) = layout.iter(&tree.children).nth(1).unwrap();

        let is_over_pick_area = cursor
            .position()
            .map(|cursor_position| {
                title_bar.is_over_pick_area(&tree.children[1], title_bar_layout, cursor_position)
            })
            .unwrap_or_default();

        if is_over_pick_area && drag_enabled {
            return Some(mouse::Interaction::Grab);
        }

        None
    }

    pub(crate) fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
        drag_enabled: bool,
    ) -> mouse::Interaction {
        let (body_layout, title_bar_interaction) = if let Some(title_bar) = &self.title_bar {
            let mut children = layout.iter(&tree.children);
            let (body_layout, _) = children.next().unwrap();
            let (title_bar_layout, title_bar_tree) = children.next().unwrap();

            let is_over_pick_area = cursor
                .position()
                .map(|cursor_position| {
                    title_bar.is_over_pick_area(title_bar_tree, title_bar_layout, cursor_position)
                })
                .unwrap_or_default();

            if is_over_pick_area && drag_enabled {
                return mouse::Interaction::Grab;
            }

            let mouse_interaction = title_bar.mouse_interaction(
                title_bar_tree,
                title_bar_layout,
                cursor,
                viewport,
                renderer,
            );

            (body_layout, mouse_interaction)
        } else {
            (layout, mouse::Interaction::default())
        };

        self.body
            .mouse_interaction(&tree.children[0], body_layout, cursor, viewport, renderer)
            .max(title_bar_interaction)
    }

    pub(crate) fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        if let Some(title_bar) = self.title_bar.as_mut() {
            let mut children = layout.iter_mut(&mut tree.children);
            let (body_layout, body_tree) = children.next().unwrap();
            let (title_bar_layout, title_bar_tree) = children.next().unwrap();

            let title_bar_overlays = title_bar.overlay(
                title_bar_tree,
                title_bar_layout,
                renderer,
                viewport,
                translation,
                window,
            );

            let body_overlays = self.body.overlay(
                body_tree,
                body_layout,
                renderer,
                viewport,
                translation,
                window,
            );

            title_bar_overlays
                .into_iter()
                .chain(body_overlays)
                .collect()
        } else {
            let (layout, body_tree) = layout.iter_mut(&mut tree.children).next().unwrap();

            self.body
                .overlay(body_tree, layout, renderer, viewport, translation, window)
        }
    }
}

/// A quad that paints `backdrop` over the corners of a pane at `bounds`
/// outside its `border`'s rounded outline, once clipped to `bounds`; `None`
/// for a pane without rounded corners.
///
/// A rounded container clips its background, not its content, so a body
/// that paints its whole rectangle would show past the corners. The quad is
/// all border: both renderers draw a border as the band between the quad's
/// rounded outline and that outline drawn `width` further in, with each
/// radius `width` smaller. Grown by `reach` on every side, with each radius
/// `reach` larger and a border `reach` wide, the band's inner edge would be
/// exactly the pane's outline. Its outer edge clears the pane's corner while
/// `reach` exceeds `(√2 − 1) · radius` and a pixel of anti-aliasing: the
/// corner lies `√2 · radius` from the arc's centre. `reach` is the largest
/// radius, which leaves room to spare.
///
/// The band reaches half the pane's border further in, under the border,
/// which is drawn over it. Were the band to end where the border begins,
/// each would cover about half of every pixel the outline crosses, and a
/// quarter of the body would show through between them: a faint line along
/// the outside of every rounded corner, and of every edge that falls
/// between pixels (at a fractional scale factor).
///
/// The quad is not snapped to the pixel grid, nor is the border drawn with
/// it. A renderer that snaps quads (wgpu, with iced's `crisp` feature)
/// rounds each of their edges on its own; the mask's lie `reach` from the
/// pane's, a fraction of a pixel at most scale factors, so the two would
/// round apart, and the arcs, measured from the rounded edges, would no
/// longer share their centres. Up to a pixel of the band's overlap with the
/// border would be lost, and the body would show through again.
fn corner_mask(bounds: Rectangle, border: Border, backdrop: Color) -> Option<renderer::Quad> {
    let radius = border.radius;
    let reach = radius
        .top_left
        .max(radius.top_right)
        .max(radius.bottom_right)
        .max(radius.bottom_left);
    if reach <= 0.0 {
        return None;
    }

    Some(renderer::Quad {
        bounds: bounds.expand(reach),
        border: Border {
            color: backdrop,
            width: reach + border.width / 2.0,
            radius: Radius {
                top_left: radius.top_left + reach,
                top_right: radius.top_right + reach,
                bottom_right: radius.bottom_right + reach,
                bottom_left: radius.bottom_left + reach,
            },
        },
        snap: false,
        ..renderer::Quad::default()
    })
}

impl<Message, T, W, Theme, Renderer> Draggable for &Content<'_, Message, T, W, Theme, Renderer>
where
    Theme: container::Catalog,
    Renderer: core::Renderer,
    T: Widget<Message, Theme, Renderer>,
{
    fn can_be_dragged_at(&self, tree: &Tree, layout: Layout, cursor_position: Point) -> bool {
        if let Some(title_bar) = &self.title_bar {
            let (title_bar_layout, _) = layout.iter(&tree.children).nth(1).unwrap();

            title_bar.is_over_pick_area(&tree.children[1], title_bar_layout, cursor_position)
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::text;

    type Pane<'a> = Content<'a, (), Void, iced::widget::Text<'a>>;

    const BODY: Color = Color::from_rgb(1.0, 0.0, 0.0);
    const BACKDROP: Color = Color::from_rgb(0.0, 0.0, 1.0);
    const BORDER: Color = Color::from_rgb(0.0, 1.0, 0.0);
    const RADIUS: f32 = 12.0;
    const SIZE: Size<u32> = Size::new(100, 80);

    /// A rendered grid, in physical pixels at the simulator's scale factor.
    struct Rendered {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        scale: u32,
    }

    impl Rendered {
        /// The colour of the physical pixel `(x, y)`.
        fn physical(&self, x: u32, y: u32) -> Color {
            let at = ((y * self.width + x) * 4) as usize;
            Color::from_rgba8(
                self.rgba[at],
                self.rgba[at + 1],
                self.rgba[at + 2],
                f32::from(self.rgba[at + 3]) / 255.0,
            )
        }

        /// The colour at the logical pixel `(x, y)`.
        fn at(&self, x: u32, y: u32) -> Color {
            self.physical(x * self.scale, y * self.scale)
        }
    }

    /// A grid of one pane with corners of `radius`, `corners` painted,
    /// `inset` from the window's edges, rendered. The pane's body paints its
    /// whole rectangle, clipped to it, as the map's tiles do.
    fn render(corners: Option<Color>, radius: f32, inset: f32) -> Rendered {
        let (state, _) = super::super::State::new(());
        let grid = super::super::PaneGrid::new(&state, move |_, (), _| {
            let fill = container(iced::widget::Space::new())
                .width(iced::Length::Fill)
                .height(iced::Length::Fill)
                .style(|_| container::Style::default().background(BODY));
            let body = container(fill)
                .width(iced::Length::Fill)
                .height(iced::Length::Fill)
                .clip(true);
            Content::new(body).style(move |_| container::Style {
                border: Border {
                    color: BORDER,
                    width: 1.0,
                    radius: radius.into(),
                },
                ..container::Style::default()
            })
        })
        .style(move |theme| super::super::Style {
            corners,
            ..super::super::default(theme)
        });

        let mut simulator: iced_test::Simulator<'_, ()> = iced_test::Simulator::with_size(
            iced::Settings::default(),
            Size::new(SIZE.width as f32, SIZE.height as f32),
            container(grid).padding(inset),
        );
        let snapshot = simulator.snapshot(&iced::Theme::Dark).unwrap();

        // The snapshot's pixels are read back from the PNG it writes.
        let directory = std::env::temp_dir().join(format!(
            "groundhog-pane-corners-{}-{}-{radius}-{inset}",
            std::process::id(),
            corners.is_some()
        ));
        drop(std::fs::remove_dir_all(&directory));
        assert!(snapshot.matches_image(directory.join("pane.png")).unwrap());
        let written = std::fs::read_dir(&directory)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let mut reader = png::Decoder::new(std::io::BufReader::new(
            std::fs::File::open(&written).unwrap(),
        ))
        .read_info()
        .unwrap();
        let mut rgba = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut rgba).unwrap();
        std::fs::remove_dir_all(&directory).unwrap();

        let scale = info.width / SIZE.width;
        assert_eq!(info.height, SIZE.height * scale);
        Rendered {
            rgba,
            width: info.width,
            height: info.height,
            scale,
        }
    }

    /// How far `point` lies outside the outline of `bounds` with corners
    /// of `radius`; negative inside.
    fn outside_by(bounds: Rectangle, radius: f32, point: Point) -> f32 {
        let centre = bounds.center();
        let x = (point.x - centre.x).abs() - (bounds.width / 2.0 - radius);
        let y = (point.y - centre.y).abs() - (bounds.height / 2.0 - radius);
        x.max(0.0).hypot(y.max(0.0)) + x.max(y).min(0.0) - radius
    }

    #[track_caller]
    fn assert_near(actual: Color, expected: Color) {
        let close = |a: f32, b: f32| (a - b).abs() < 0.02;
        assert!(
            close(actual.r, expected.r)
                && close(actual.g, expected.g)
                && close(actual.b, expected.b),
            "{actual:?} is not {expected:?}"
        );
    }

    #[test]
    fn the_backdrop_covers_a_body_past_the_rounded_corners() {
        let rendered = render(Some(BACKDROP), RADIUS, 0.0);
        let pixel = |x, y| rendered.at(x, y);
        let (right, bottom) = (SIZE.width - 1, SIZE.height - 1);

        // Outside the arc, at every corner.
        for (x, y) in [(0, 0), (1, 1), (right, 0), (0, bottom), (right, bottom)] {
            assert_near(pixel(x, y), BACKDROP);
        }
        // Inside it, the body.
        assert_near(pixel(5, 5), BODY);
        assert_near(pixel(right - 5, bottom - 5), BODY);
        assert_near(pixel(SIZE.width / 2, SIZE.height / 2), BODY);
        // The straight edges keep their border.
        assert_near(pixel(0, SIZE.height / 2), BORDER);
        assert_near(pixel(SIZE.width / 2, bottom), BORDER);
    }

    #[test]
    fn without_corners_the_body_shows_past_them() {
        assert_near(render(None, RADIUS, 0.0).at(1, 1), BODY);
    }

    /// Where the pane's edges fall between pixels, as at a fractional scale
    /// factor, the pixels the outline crosses are covered in part by the
    /// border and in part by the backdrop, and the body must not show
    /// through between them. A renderer that snaps quads to the pixel grid
    /// rounds each of their edges on its own, so the corners must stay clear
    /// whatever the rounding does: the insets put the pane's edges at
    /// various fractions of a pixel, and the radius makes the distance from
    /// the pane's edge to the corner mask's (`corner_mask`) a fraction too.
    #[test]
    fn nothing_of_the_body_shows_past_the_outline() {
        const RADIUS: f32 = 12.05;

        for inset in [10.0, 10.1, 10.275, 10.3, 10.45, 10.6, 10.8] {
            let rendered = render(Some(BACKDROP), RADIUS, inset);
            let pane = Rectangle::new(
                Point::new(inset, inset),
                Size::new(
                    SIZE.width as f32 - 2.0 * inset,
                    SIZE.height as f32 - 2.0 * inset,
                ),
            );
            let scale = rendered.scale as f32;

            // The pixels of the pane's rectangle that lie past its corners:
            // the rest is the window's, which the pane does not draw.
            for y in 0..rendered.height {
                for x in 0..rendered.width {
                    let centre = Point::new((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale);
                    if pane.contains(centre) && outside_by(pane, RADIUS, centre) > 0.0 {
                        // The body is the only red; the window's grey
                        // (around the pane, where its edge falls between
                        // pixels) adds as much to every channel.
                        let colour = rendered.physical(x, y);
                        assert!(
                            colour.r - colour.g.min(colour.b) < 0.02,
                            "inset {inset}: the body shows at ({x}, {y}): {colour:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_preview_keeps_its_own_tree_through_a_diff() {
        let with = || Pane::new(text("Body")).preview(iced::Widget::boxed(text("Label")));
        let empty = Tree::empty().tag;

        let mut tree = with().state();
        assert_eq!(tree.children.len(), 3);
        assert_ne!(tree.children[2].tag, empty, "the preview has its tree");

        Pane::new(text("Body")).diff(&mut tree);
        assert_eq!(tree.children[2].tag, empty, "a preview taken away");

        with().diff(&mut tree);
        assert_ne!(tree.children[2].tag, empty, "a preview given back");
    }
}
