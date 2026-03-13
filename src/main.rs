#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod messaging;
mod p2p;
mod tor;
mod video;

fn main() -> anyhow::Result<()> {
    let native_options = eframe::NativeOptions::default();

    eframe::run_native(
        "Antifa Messenger",
        native_options,
        Box::new(
            |cc| -> Result<_, Box<dyn std::error::Error + Send + Sync>> {
                Ok::<Box<dyn eframe::App>, _>(Box::new(app::MessengerApp::new(cc)))
            },
        ),
    )?;

    Ok(())
}
