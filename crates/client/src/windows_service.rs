use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

pub(crate) const SERVICE_NAME: &str = "RustDeskLightClientService";

define_windows_service!(ffi_service_main, my_service_main);

pub fn run() -> windows_service::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

fn my_service_main(arguments: Vec<OsString>) {
    if let Err(_e) = run_service(arguments) {
        // Log error or ignore
    }
}

fn run_service(arguments: Vec<OsString>) -> windows_service::Result<()> {
    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop => {
                std::process::exit(0);
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;

    let next_status = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    };

    status_handle.set_service_status(next_status)?;

    // Parse arguments passed to the service to find --config
    let mut config_path = None;
    let args: Vec<String> = arguments.into_iter().map(|a| a.to_string_lossy().to_string()).collect();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--config" {
            config_path = iter.next().map(PathBuf::from);
        } else if let Some(path) = arg.strip_prefix("--config=") {
            config_path = Some(PathBuf::from(path));
        }
    }

    let config = match crate::runtime::Config::load(rdl_config::EndpointOverrides {
        config_path,
        ..Default::default()
    }) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config: {}", e);
            return Ok(());
        }
    };
    let _ = crate::app::run_terminal(config, false);

    Ok(())
}
