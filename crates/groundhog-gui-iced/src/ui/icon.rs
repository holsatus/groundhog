//! The icon set.
//!
//! Every icon is an SVG under `assets/icons/`, embedded at compile time and
//! named here once. A view asks for an [`Icon`] by name, and never touches
//! files or handles:
//!
//! ```ignore
//! ui::icon_button(Icon::Upload).hint("Upload").on_press(Message::Upload)
//! ```
//!
//! To add an icon, put its file in `assets/icons/` and add a line to the
//! `icons!` list below.

use std::sync::LazyLock;

use iced::widget::svg::{Handle, Svg};

use super::style::{self, ButtonKind};

macro_rules! icons {
    ($($(#[$doc:meta])* $name:ident => $file:literal,)*) => {
        /// An icon of the set.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Icon {
            $($(#[$doc])* $name,)*
        }

        impl Icon {
            /// The icon's image. Each is parsed once and shared after that,
            /// so asking on every frame costs a reference count.
            pub fn handle(self) -> Handle {
                match self {
                    $(Icon::$name => {
                        static HANDLE: LazyLock<Handle> = LazyLock::new(|| {
                            Handle::from_memory(
                                include_bytes!(concat!("../../assets/icons/", $file)).as_slice(),
                            )
                        });
                        HANDLE.clone()
                    })*
                }
            }

            /// Every icon, for tests.
            #[cfg(test)]
            const ALL: &[Icon] = &[$(Icon::$name,)*];
        }
    };
}

icons! {
    /// Close a pane, dismiss a notice.
    Close => "x.svg",
    /// Delete something stored.
    Delete => "trash-2.svg",
    /// Edit something stored.
    Edit => "pencil.svg",
    /// Send to the vehicle.
    Upload => "upload.svg",
    /// Undo an edit.
    Revert => "undo-2.svg",
    /// Fetch from the vehicle.
    Download => "download.svg",
    /// Add an item.
    Add => "plus.svg",
    /// Move an item earlier.
    MoveUp => "arrow-up.svg",
    /// Move an item later.
    MoveDown => "arrow-down.svg",
    /// Write to a file.
    Save => "save.svg",
    /// Read from a file.
    Open => "folder-open.svg",
    /// Place on the map.
    Place => "map-pin.svg",
    /// Start or resume.
    Play => "play.svg",
    /// Pause.
    Pause => "pause.svg",
    /// Continue from here.
    Skip => "skip-forward.svg",
    /// Erase something stored on the vehicle.
    Erase => "eraser.svg",
    /// Maximize a pane.
    Maximize => "maximize-2.svg",
    /// Restore a maximized pane.
    Minimize => "minimize-2.svg",
    /// The map.
    Map => "map.svg",
    /// The vehicles panel.
    Vehicle => "navigation.svg",
    /// The mission panel.
    Mission => "route.svg",
    /// The parameters panel.
    Parameters => "variable.svg",
    /// The calibration panel.
    Calibration => "crosshair.svg",
    /// The links panel.
    Links => "link.svg",
    /// The log panel.
    Log => "list.svg",
    /// The settings panel.
    Settings => "wrench.svg",
    /// The video panel.
    Video => "video.svg",
    /// Open a menu of what does not fit.
    More => "ellipsis-vertical.svg",
    /// The destination for setting a vehicle up.
    Setup => "sliders-horizontal.svg",
    /// The destination holding the rest.
    Others => "ellipsis.svg",
    /// Go back a step.
    Back => "arrow-left.svg",
    /// Open what a row names.
    Forward => "chevron-right.svg",
}

/// `icon` at `size` (usually [`style::size::ICON`]), coloured for a button
/// of `kind`, or faded while not `enabled`.
pub fn icon<'a>(icon: Icon, size: f32, kind: ButtonKind, enabled: bool) -> Svg<'a> {
    Svg::new(icon.handle())
        .width(size)
        .height(size)
        .style(style::icon(kind, enabled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_an_svg() {
        for icon in Icon::ALL {
            let iced::advanced::svg::Data::Bytes(bytes) = icon.handle().data().clone() else {
                panic!("{icon:?} is embedded");
            };
            let text = std::str::from_utf8(&bytes).expect("an SVG is text");
            assert!(text.starts_with("<svg"), "{icon:?} is an SVG");
        }
    }
}
