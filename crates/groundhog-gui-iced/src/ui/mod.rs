//! Groundhog's toolkit on top of iced.
//!
//! Views build their widgets here rather than from raw iced widgets, so the
//! look is decided in one place and a view says only what it shows:
//!
//! - [`style`]: design tokens ([`space`], [`size`]), [`Tone`]s, and the
//!   style function of every widget;
//! - [`theme`]: the palettes;
//! - [`icon`]: the icon set;
//! - text in the typography ([`caption`], [`title`], [`TextExt::tone`]);
//! - buttons ([`button()`], [`icon_button`]) built as an
//!   [`Action`](button::Action);
//! - inputs ([`text_input`], [`pick_list`], [`checkbox`]);
//! - layout ([`panel`], [`card`], [`field`], [`list_row`], [`toolbar`],
//!   [`split_column`], ...).
//!
//! A view that needs something new adds it here when a second view would
//! build it the same way.

mod button;
mod container;
pub mod icon;
mod input;
mod split;
pub mod style;
mod text;
pub mod theme;

pub use button::{button, button_with, icon_button};
pub use container::{
    card, card_list, chip, divider, empty, field, list_row, meter, panel, progress, section, stat,
    toolbar, tooltip,
};
pub use icon::Icon;
pub use input::{checkbox, pick_list, text_input};
pub use split::split_column;
pub use style::{Chip, Tone, size, space};
pub use text::{TextExt, caption, heading, mono, severity, text, title};
