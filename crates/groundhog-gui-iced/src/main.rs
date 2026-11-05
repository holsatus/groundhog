//! The desktop entry point. The application itself lives in the library, so
//! Android can start it through `android_main`.

fn main() -> iced::Result {
    groundhog_gui_iced::run()
}
