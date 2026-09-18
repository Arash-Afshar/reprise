mod analyze;
mod app;
mod chess_util;
mod data;
mod engine;
mod sync;
mod theme;
mod ui;
mod weekly_review;

use gtk4::prelude::*;

fn main() {
    let app = gtk4::Application::builder()
        .application_id("org.omarchy.reprise")
        .build();

    app.connect_activate(app::build_ui);
    app.run();
}
