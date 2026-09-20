mod app;
mod commands;
mod geometry;
mod model;
mod render;
mod selection;

fn main() -> eframe::Result {
    env_logger::init();

    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Inkline")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([720.0, 480.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Inkline",
        native_options,
        Box::new(|cc| Ok(Box::new(app::InklineApp::new(cc)))),
    )
}
