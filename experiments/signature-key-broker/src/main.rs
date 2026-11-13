//! Explicitly launched experimental stdio broker, never an app/UI entry point.
//!
//! PATH: experiments/signature-key-broker/src/main.rs
//! CONSUMED BY: developer protocol clients after the coordinator's CPU gate.
//! No CLI data/provider paths; one unnamed ephemeral TEST key per Windows process.

use std::process::ExitCode;

fn main() -> ExitCode {
    if std::env::args_os().len() != 1 {
        eprintln!("Experimental test-key broker accepts JSON lines on stdin only; no arguments.");
        return ExitCode::from(2);
    }
    #[cfg(not(windows))]
    {
        eprintln!("Experimental native key adapter is implemented only for Windows.");
        ExitCode::from(2)
    }
    #[cfg(windows)]
    {
        use signature_key_broker_experiment::{protocol, windows::EphemeralTestKey};
        let key = match EphemeralTestKey::new() {
            Ok(key) => key,
            Err(_) => {
                eprintln!("Ephemeral test-key initialization failed; no persistent-key fallback.");
                return ExitCode::from(1);
            }
        };
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        match protocol::serve(&key, stdin.lock(), stdout.lock()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => {
                eprintln!("Experimental broker transport failed; input/output data not logged.");
                ExitCode::from(1)
            }
        }
    }
}
