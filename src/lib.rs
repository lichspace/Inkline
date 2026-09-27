mod app;
mod commands;
mod geometry;
mod model;
mod render;
mod selection;

/// Starts Inkline using eframe's native winit/wgpu integration.
pub fn run() -> eframe::Result {
    env_logger::init();

    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/inkline.png"))
        .expect("embedded app icon should be a valid PNG");

    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Inkline")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([360.0, 480.0])
            .with_icon(icon),
        ..Default::default()
    };

    #[cfg(target_os = "ios")]
    let mut native_options = native_options;

    // Apple's Metal implementation exposes the WebGPU downlevel limit of 15
    // inter-stage variables. egui's desktop default requests 16, which prevents
    // wgpu from creating a device on iOS before the first frame is drawn.
    #[cfg(target_os = "ios")]
    if let egui_wgpu::WgpuSetup::CreateNew(setup) = &mut native_options.wgpu_options.wgpu_setup {
        setup.device_descriptor = std::sync::Arc::new(|adapter| wgpu::DeviceDescriptor {
            label: Some("Inkline iOS wgpu device"),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        });
    }

    eframe::run_native(
        "Inkline",
        native_options,
        Box::new(|cc| Ok(Box::new(app::InklineApp::new(cc)))),
    )
}

/// Entry point called by the small Xcode launcher in `ios/Inkline/main.m`.
#[cfg(target_os = "ios")]
#[no_mangle]
pub extern "C" fn inkline_start() {
    run().expect("Inkline failed to start");
}
