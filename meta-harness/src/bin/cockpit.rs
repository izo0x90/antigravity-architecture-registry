use meta_harness::cockpit::CockpitApp;

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!("Launching Meta-Harness Native Egui Cockpit...");

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Meta-Harness Control Cockpit")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([800.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Meta-Harness Cockpit",
        native_options,
        Box::new(|cc| Ok(Box::new(CockpitApp::new(cc)))),
    )
}
