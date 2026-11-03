//! The `matty` binary: parses the command line and starts the app.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None => {
            matty_app::run();
            ExitCode::SUCCESS
        }
        Some("--version" | "-V") => {
            println!("matty {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") => {
            println!(
                "matty {}\n\nUSAGE:\n    matty [--version | --help]",
                env!("CARGO_PKG_VERSION")
            );
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("matty: unknown argument `{other}`; try `matty --help`");
            ExitCode::from(2)
        }
    }
}
