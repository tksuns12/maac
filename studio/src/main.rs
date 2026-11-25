mod analysis;
mod app;
mod audio;
mod editor;
mod mixer;
mod project;
mod timeline;
mod watcher;

use std::path::PathBuf;

use iced::Size;

fn main() -> iced::Result {
    let initial_path = std::env::args_os().nth(1).map(PathBuf::from);
    iced::application(
        app::StudioApp::title,
        app::StudioApp::update,
        app::StudioApp::view,
    )
    .subscription(app::StudioApp::subscription)
    .theme(app::StudioApp::theme)
    .window_size(Size::new(1360.0, 840.0))
    .run_with(move || app::StudioApp::new(initial_path))
}
