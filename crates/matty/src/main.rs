//! The `matty` binary: parses the command line and starts the app.

use std::process::ExitCode;

#[expect(
    clippy::print_stdout,
    reason = "--version and --help answer on stdout before the app or its logger starts"
)]
fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None => {
            matty_app::run();
            ExitCode::SUCCESS
        }
        Some("--offline") => {
            matty_app::run_offline();
            ExitCode::SUCCESS
        }
        Some("--version" | "-V") => {
            println!("matty {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") => {
            println!(
                "matty {}\n\nUSAGE:\n    matty [--offline | --version | --help]\n\n    --offline    open the last account's cache without connecting",
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
