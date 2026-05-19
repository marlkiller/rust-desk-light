pub const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/icons/rdl-icon-256.png");

pub fn app_window_icon() -> Option<egui::IconData> {
    let image = image::load_from_memory(APP_ICON_PNG).ok()?.into_rgba8();
    let width = image.width();
    let height = image.height();
    Some(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}
