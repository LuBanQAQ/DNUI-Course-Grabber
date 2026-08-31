#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod app;
mod model;
mod network;
mod ocr;
mod vpn_auth;

use app::XkApp;

fn main() -> eframe::Result<()> {
    if std::env::args().any(|arg| arg == "--vpn-auth-helper") {
        vpn_auth::run_helper();
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("DNUI 选课工具（Rust） By Msz")
            .with_inner_size([1_380.0, 820.0])
            .with_min_inner_size([980.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "DNUI 选课工具（Rust） By Msz",
        options,
        Box::new(|cc| Ok(Box::new(XkApp::new(cc)))),
    )
}
