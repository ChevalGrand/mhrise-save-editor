use eframe::egui;
use mhrise_save_editor::gui::GuiApp;

fn main() -> eframe::Result<()> {
  let options = eframe::NativeOptions {
    viewport: egui::ViewportBuilder::default().with_inner_size([780.0, 640.0]),
    ..Default::default()
  };
  eframe::run_native(
    "MHRise Save Editor",
    options,
    Box::new(|creation_context| Ok(Box::new(GuiApp::new(creation_context)))),
  )
}
