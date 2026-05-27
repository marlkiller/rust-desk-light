#[cfg(debug_assertions)]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        {
            eprintln!($($arg)*);
        }
    };
}

#[cfg(not(debug_assertions))]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        {
            let _ = format_args!($($arg)*);
        }
    };
}

mod app;
mod app_event;
#[cfg(feature = "gui")]
mod app_ui;
mod client_network;
mod commands;
mod execute;
mod live_control;
mod outbound;
mod remote_management;
mod runtime;
mod session;
mod support;
mod system_info;
mod text_decode;
mod tools;
mod user_interaction;
#[cfg(feature = "gui")]
mod windowing;
#[cfg(target_os = "windows")]
mod windows_service;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run()
}
