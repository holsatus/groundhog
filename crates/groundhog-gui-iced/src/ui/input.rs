//! Inputs, styled alike, as tall as a button at the density views are
//! built at, and filling their width unless told otherwise.

use std::{borrow::Borrow, ops::RangeInclusive};

use iced::{
    Length, Padding,
    widget::{Checkbox, PickList, Slider, TextInput, slider::HandleShape, text::IntoFragment},
};

use super::style::{self, Density, size};

/// The height of a line of body text: iced's default size, 16, at its
/// default line height of 1.3.
const LINE: f32 = 16.0 * 1.3;

/// Padding that makes a one-line input as tall as a control, `sides` wide.
fn input_padding(sides: f32) -> Padding {
    let vertical = ((size::control() - LINE) / 2.0).max(0.0);
    Padding {
        top: vertical,
        bottom: vertical,
        left: sides,
        right: sides,
    }
}

/// A text field.
pub fn text_input<'a, M: Clone + 'a>(
    placeholder: impl IntoFragment<'a>,
    value: impl IntoFragment<'a>,
) -> TextInput<'a, M> {
    iced::widget::text_input(placeholder, value)
        .style(style::text_input)
        .padding(input_padding(5.0))
        .width(Length::Fill)
}

/// A choice of one of `options`, shown with `to_string`.
pub fn pick_list<'a, T, L, V, M>(
    selected: Option<V>,
    options: L,
    to_string: impl Fn(&T) -> String + 'a,
) -> PickList<'a, T, L, V, M>
where
    T: PartialEq + Clone + 'a,
    L: Borrow<[T]> + 'a,
    V: Borrow<T> + 'a,
    M: Clone + 'a,
{
    iced::widget::pick_list(selected, options, to_string)
        .style(style::pick_list)
        .padding(input_padding(10.0))
        .width(Length::Fill)
}

/// A labelled checkbox.
pub fn checkbox<'a, M: 'a>(label: impl IntoFragment<'a>, checked: bool) -> Checkbox<'a, M> {
    unlabelled_checkbox(checked).label(label)
}

/// A checkbox whose label is elsewhere, such as a form row's
/// ([`field`](super::field)).
pub fn unlabelled_checkbox<'a, M: 'a>(checked: bool) -> Checkbox<'a, M> {
    iced::widget::checkbox(checked).size(size::checkbox())
}

/// A slider over `range`, its handle large enough for a finger at touch
/// density.
pub fn slider<'a, T, M>(
    range: RangeInclusive<T>,
    value: T,
    on_change: impl Fn(T) -> M + 'a,
) -> Slider<'a, T, M>
where
    T: Copy + From<u8> + PartialOrd,
    M: Clone,
{
    let touch = style::density() == Density::Touch;
    iced::widget::slider(range, value, on_change)
        .height(size::slider())
        .style(move |theme, status| {
            let mut style = iced::widget::slider::default(theme, status);
            if touch {
                style.handle.shape = HandleShape::Circle { radius: 11.0 };
                style.rail.width = 6.0;
            }
            style
        })
}
