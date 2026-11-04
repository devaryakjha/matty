//! The `matty` binary: parses the command line and starts the app.

use std::process::ExitCode;

#[expect(
    clippy::print_stdout,
    reason = "--version and --help answer on stdout before the app or its logger starts"
)]
fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        // A Matty already running takes this launch's link, or only comes
        // forward, and this one exits (#84).
        None if matty_app::instance::forward(None) => ExitCode::SUCCESS,
        None => {
            matty_app::run();
            ExitCode::SUCCESS
        }
        Some("--offline") => {
            matty_app::run_offline();
            ExitCode::SUCCESS
        }
        Some("--version" | "-V") => {
            println!("matty {}", matty_app::VERSION);
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") => {
            println!(
                "matty {}\n\nUSAGE:\n    matty [--offline | --version | --help | <link>]\n\n    --offline    open the last account's cache without connecting\n    <link>       open a matty:// or mattermost:// link: a channel, a post, or the SSO hand-off",
                matty_app::VERSION
            );
            ExitCode::SUCCESS
        }
        // A deep link, such as the SSO hand-off a browser passes to
        // `matty mattermost://…` on systems that launch the handler.
        Some(link) if link.contains("://") && matty_app::instance::forward(Some(link)) => {
            ExitCode::SUCCESS
        }
        Some(link) if link.contains("://") => {
            matty_app::run_with_links(vec![link.to_string()]);
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("matty: unknown argument `{other}`; try `matty --help`");
            ExitCode::from(2)
        }
    }
}
