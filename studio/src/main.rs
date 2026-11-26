mod analysis;
mod app;
mod audio;
mod automation;
mod code_view;
mod collab;
mod editor;
mod mixer;
mod plugins;
mod project;
mod sampler_import;
mod timeline;
mod watcher;

use std::borrow::Cow;
use std::path::PathBuf;

use iced::{Font, Size};

fn main() -> iced::Result {
    let initial_path = std::env::args_os().nth(1).map(PathBuf::from);
    let mut app = iced::application(
        app::StudioApp::title,
        app::StudioApp::update,
        app::StudioApp::view,
    )
    .subscription(app::StudioApp::subscription)
    .theme(app::StudioApp::theme)
    .window_size(Size::new(1360.0, 840.0));

    // Register macOS's built-in Korean/CJK sans-serif font so Hangul review pins,
    // AI chat prompts, and replies render crisply alongside Latin UI text.
    #[cfg(target_os = "macos")]
    {
        if let Ok(bytes) = std::fs::read("/System/Library/Fonts/AppleSDGothicNeo.ttc") {
            app = app
                .font(Cow::Owned(bytes))
                .default_font(Font::with_name("Apple SD Gothic Neo"));
        } else if let Ok(bytes) =
            std::fs::read("/System/Library/Fonts/Supplemental/AppleGothic.ttf")
        {
            app = app
                .font(Cow::Owned(bytes))
                .default_font(Font::with_name("AppleGothic"));
        }
    }

    app.run_with(move || app::StudioApp::new(initial_path))
}
