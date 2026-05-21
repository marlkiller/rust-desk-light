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
mod audio_stream;
mod client_network;
mod commands;
mod execute;
mod live_control;
mod live_video_stream;
mod outbound;
mod payload;
mod p2p;
mod remote_management;
mod reverse_proxy;
mod runtime;
mod session;
mod stream_state;
mod support;
mod system_info;
mod text_decode;
mod user_interaction;
#[cfg(feature = "gui")]
mod windowing;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run()
}
