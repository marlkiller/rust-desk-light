mod app;
mod command_menu;
mod live_control;
mod remote_management;
mod runtime;
mod user_interaction;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run()
}
