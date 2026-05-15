mod app;
mod commands;
mod live_control;
mod remote_management;
mod runtime;
mod support;
mod system_info;
mod user_interaction;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run()
}
