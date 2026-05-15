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
        builder.with_fullscreen(false)
    }

    #[cfg(not(target_os = "macos"))]
    {
        builder
    }
}

pub(crate) fn render_child_window_controls(ui: &mut egui::Ui) {
    #[cfg(target_os = "macos")]
    macos::install_zoom_button_handlers();

    let _ = ui;
}

#[cfg(target_os = "macos")]
mod macos {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{define_class, msg_send, sel, ClassType};
    use objc2_app_kit::{
        NSApplication, NSScreen, NSWindow, NSWindowButton, NSWindowCollectionBehavior,
    };
    use objc2_foundation::{MainThreadMarker, NSObject, NSRect};

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "RustDeskLightZoomButtonTarget"]
        struct ZoomButtonTarget;

        impl ZoomButtonTarget {
            #[unsafe(method(rdlZoomButtonClicked:))]
            fn zoom_button_clicked(&self, sender: &AnyObject) {
                let window: *mut NSWindow = unsafe { msg_send![sender, window] };
                let Some(window) = (unsafe { window.as_ref() }) else {
                    return;
                };

                toggle_window_zoom(window);
            }
        }
    );

    pub(super) fn install_zoom_button_handlers() {
        let Some(main_thread) = MainThreadMarker::new() else {
            return;
        };

        let app = NSApplication::sharedApplication(main_thread);
        let mut active_window_ids = Vec::new();

        for window in app.windows().iter() {
            active_window_ids.push(window_id(&window));
            prefer_zoom_over_fullscreen_space(&window);
            install_zoom_button_handler(&window);
        }

        if let Ok(mut restore_frames) = restore_frames().lock() {
            restore_frames.retain(|window_id, _| active_window_ids.contains(window_id));
        }
    }

    fn prefer_zoom_over_fullscreen_space(window: &NSWindow) {
        let mut behavior = window.collectionBehavior();
        behavior.remove(
            NSWindowCollectionBehavior::FullScreenPrimary
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::FullScreenAllowsTiling,
        );
        behavior.insert(
            NSWindowCollectionBehavior::FullScreenNone
                | NSWindowCollectionBehavior::FullScreenDisallowsTiling,
        );

        if window.collectionBehavior() != behavior {
            window.setCollectionBehavior(behavior);
        }
    }

    fn install_zoom_button_handler(window: &NSWindow) {
        let Some(button) = window.standardWindowButton(NSWindowButton::ZoomButton) else {
            return;
        };

        unsafe {
            button.setTarget(Some(zoom_button_target()));
            button.setAction(Some(sel!(rdlZoomButtonClicked:)));
        }
    }

    fn zoom_button_target() -> &'static AnyObject {
        static TARGET: OnceLock<usize> = OnceLock::new();

        let target = TARGET.get_or_init(|| {
            let target: Retained<ZoomButtonTarget> =
                unsafe { msg_send![ZoomButtonTarget::class(), new] };
            Retained::into_raw(target).cast::<AnyObject>() as usize
        });

        unsafe { &*(*target as *const AnyObject) }
    }

    fn toggle_window_zoom(window: &NSWindow) {
        let Some(visible_frame) = visible_frame(window) else {
            return;
        };

        let current_frame = window.frame();
        let window_id = window_id(window);

        if rects_match(current_frame, visible_frame) {
            let restore_frame = restore_frames()
                .lock()
                .ok()
                .and_then(|mut frames| frames.remove(&window_id));
            window.setFrame_display(
                restore_frame.unwrap_or_else(|| fallback_restore_frame(visible_frame)),
                true,
            );
        } else {
            if let Ok(mut frames) = restore_frames().lock() {
                frames.insert(window_id, current_frame);
            }
            window.setFrame_display(visible_frame, true);
        }
    }

    fn visible_frame(window: &NSWindow) -> Option<NSRect> {
        let Some(main_thread) = MainThreadMarker::new() else {
            return None;
        };

        let screen = window
            .screen()
            .or_else(|| NSScreen::mainScreen(main_thread))?;
        Some(screen.visibleFrame())
    }

    fn fallback_restore_frame(mut frame: NSRect) -> NSRect {
        frame.origin.x += frame.size.width * 0.1;
        frame.origin.y += frame.size.height * 0.1;
        frame.size.width *= 0.8;
        frame.size.height *= 0.8;
        frame
    }

    fn rects_match(a: NSRect, b: NSRect) -> bool {
        const TOLERANCE: f64 = 1.0;

        (a.origin.x - b.origin.x).abs() <= TOLERANCE
            && (a.origin.y - b.origin.y).abs() <= TOLERANCE
            && (a.size.width - b.size.width).abs() <= TOLERANCE
            && (a.size.height - b.size.height).abs() <= TOLERANCE
    }

    fn window_id(window: &NSWindow) -> usize {
        window as *const NSWindow as usize
    }

    fn restore_frames() -> &'static Mutex<HashMap<usize, NSRect>> {
        static RESTORE_FRAMES: OnceLock<Mutex<HashMap<usize, NSRect>>> = OnceLock::new();
        RESTORE_FRAMES.get_or_init(|| Mutex::new(HashMap::new()))
    }
}
