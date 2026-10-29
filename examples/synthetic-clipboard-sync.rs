//! Test fixture only: disposable native clipboards; controller/receiver require X11.
fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [consent, mode] = args.as_slice() else {
        eprintln!(
            "Usage: synthetic-clipboard-sync --replace-clipboard <controller|receiver|inspect-request|inspect-response|check-display>"
        );
        return std::process::ExitCode::from(2);
    };
    if consent != "--replace-clipboard" {
        eprintln!("This fixture requires explicit clipboard replacement consent.");
        return std::process::ExitCode::from(2);
    }
    match our_clipboard::transport::run_synthetic_clipboard_fixture(mode) {
        Ok(message) => {
            if !message.is_empty() {
                println!("{message}");
            }
            std::process::ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{message}");
            std::process::ExitCode::from(2)
        }
    }
}
