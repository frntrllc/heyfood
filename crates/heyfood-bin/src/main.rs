//! Native heyfood executable composition root.

#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    #[cfg(feature = "native-credentials")]
    if let Some(outcome) = heyfood_platform::run_credential_broker_if_requested() {
        return outcome;
    }

    // Phase 0 deliberately has no public fake-service or credential bootstrap
    // switch. The released Python artifact remains authoritative until the
    // native cutover gates provide explicit validated inputs to
    // `run_qualified_session`.
    eprintln!("{}", heyfood_bin::QUALIFICATION_MESSAGE);
    ExitCode::from(78)
}
