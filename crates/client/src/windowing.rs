use eframe::egui;

pub(crate) fn child_viewport_builder(
    title: impl Into<String>,
    inner_size: [f32; 2],
    min_inner_size: [f32; 2],
) -> egui::ViewportBuilder {
    let builder = egui::ViewportBuilder::default()
        .with_title(title)
        .with_inner_size(inner_size)
        .with_min_inner_size(min_inner_size)
        .with_resizable(true);

    #[cfg(target_os = "macos")]
    {
        builder.with_fullscreen(false).with_maximize_button(false)
    }

    #[cfg(not(target_os = "macos"))]
    {
        builder
    }
}

pub(crate) fn render_child_window_controls(ui: &mut egui::Ui) {
    let _ = ui;
}
