use super::*;

impl AdminApp {
    pub(super) fn render_child_windows(&mut self, ctx: &egui::Context) {
        self.render_command_windows(ctx);
        self.render_file_manager_windows(ctx);
        self.render_desktop_windows(ctx);
        self.render_camera_windows(ctx);
        self.render_audio_windows(ctx);
        self.render_terminal_windows(ctx);
        self.render_proxy_windows(ctx);
        self.render_p2p_test_window(ctx);
        self.render_chat_windows(ctx);
        self.render_voice_chat_windows(ctx);
        self.render_interaction_command_windows(ctx);
        self.render_session_command_windows(ctx);
        self.render_execute_windows(ctx);
        if let Some(action) = client_aliases::render_alias_window(ctx, &mut self.alias_window) {
            self.apply_alias_action(action);
        }
        if let Some(action) = client_groups::render_move_group_window(
            ctx,
            &mut self.move_group_window,
            &self.client_groups,
        ) {
            self.apply_move_group_action(action);
        }
        self.render_settings_window(ctx);
        self.render_about_window(ctx);
    }
}
