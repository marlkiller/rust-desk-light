#[cfg(feature = "gui")]
compile_error!("rdl-client-cli must be built with --no-default-features --features cli");

include!("entry.rs");
