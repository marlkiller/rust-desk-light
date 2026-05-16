mod app;
mod command_menu;
mod execute;
mod live_control;
mod remote_management;
mod runtime;
mod session;
mod user_interaction;
mod windowing;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run()
}
