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
//! - layout ([`panel`], [`card`], [`field`], [`list_row`], ...), and the
//!   parts of a pane: [`header`], [`actions`], [`banner`], [`activity`],
//!   [`badge`].
//!
//! Every pane has the same anatomy, top to bottom: a header saying what it
//! is about and how that stands, grouped actions, banners and activity
//! while they apply, then its body (see AGENTS.md, "The design language").
//!
//! A view that needs something new adds it here when a second view would
//! build it the same way.

mod button;
mod container;
pub mod icon;
mod input;
pub mod style;
mod text;
pub mod theme;
mod tool;

pub use button::{button, button_with, entry, icon_button};
pub use container::{
    actions, activity, badge, banner, card, card_list, divider, empty, field, floating, list_row,
    meter, overline, panel, panel_with, picture, progress, screen, section, stat, stats, tooltip,
};
pub use icon::Icon;
pub use input::{checkbox, pick_list, slider, text_input};
pub use style::{Density, Tone, set_density, size, space};
pub use text::{TextExt, caption, mono, severity, text, title};
pub use tool::{header, tool};
