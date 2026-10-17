//! Inputs, styled alike and filling their width unless told otherwise.

use std::borrow::Borrow;

use iced::{
    Length,
    widget::{Checkbox, PickList, TextInput, text::IntoFragment},
};

use super::style;

/// A text field.
pub fn text_input<'a, M: Clone + 'a>(
    placeholder: impl IntoFragment<'a>,
    value: impl IntoFragment<'a>,
) -> TextInput<'a, M> {
    iced::widget::text_input(placeholder, value)
        .style(style::text_input)
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
        .width(Length::Fill)
}

/// A labelled checkbox.
pub fn checkbox<'a, M: 'a>(label: impl IntoFragment<'a>, checked: bool) -> Checkbox<'a, M> {
    iced::widget::checkbox(checked).label(label)
}
