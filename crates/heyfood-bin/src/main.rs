//! Native heyfood executable composition root.

#![forbid(unsafe_code)]

fn main() {
    let _ = (
        heyfood_core::VERSION,
        heyfood_application::VERSION,
        heyfood_agent_runtime::VERSION,
        heyfood_platform::VERSION,
        heyfood_voice::VERSION,
        heyfood_cli::VERSION,
        heyfood_tui::VERSION,
    );
}
