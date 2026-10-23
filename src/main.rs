#[cfg(feature = "clipboard")]
use our_clipboard::clipboard::TextClipboard;
use std::process::ExitCode;

#[cfg(feature = "clipboard")]
const HELP: &str = "Our Clipboard — early development

Usage: our-clipboard <COMMAND>

Commands:
  doctor      Report runtime platform information
  doctor --clipboard
              Initialize the clipboard backend without reading or writing text
  clipboard-probe --replace-clipboard
              Replace the clipboard with synthetic text and verify access
  watch-probe --observe-clipboard <seconds>
              Count native clipboard notifications without reading text (1-60 seconds)
  transport-probe
              Check synthetic mutual TLS over direct and relayed loopback connections
  help        Show this help
  --version   Show version

Clipboard synchronization is not implemented yet.";

#[cfg(not(feature = "clipboard"))]
const HELP: &str = "Our Clipboard — headless transport prototype

Usage: our-clipboard <COMMAND>

Commands:
  doctor           Report runtime platform information
  transport-probe  Check synthetic mutual TLS over direct and relayed loopback connections
  help             Show this help
  --version        Show version

Clipboard integration is disabled. Synchronization is not implemented yet.";

fn run(args: &[String]) -> Result<String, &'static str> {
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] | ["help"] | ["--help"] | ["-h"] => Ok(HELP.into()),
        ["--version"] | ["-V"] => Ok(format!("our-clipboard {}", env!("CARGO_PKG_VERSION"))),
        #[cfg(feature = "clipboard")]
        ["clipboard-probe", "--replace-clipboard"] => clipboard_probe(),
        #[cfg(feature = "clipboard")]
        ["watch-probe", "--observe-clipboard", seconds] => watch_probe(seconds),
        #[cfg(feature = "clipboard")]
        ["doctor", "--clipboard"] => clipboard_doctor(),
        ["transport-probe"] => our_clipboard::transport::probe(),
        ["doctor"] => Ok(format!(
            "OS: {}\nArchitecture: {}\nSynchronization: not implemented",
            std::env::consts::OS,
            std::env::consts::ARCH
        )),
        _ => Err("Unknown command or unexpected arguments. Run 'our-clipboard help'."),
    }
}

#[cfg(feature = "clipboard")]
fn watch_probe(seconds: &str) -> Result<String, &'static str> {
    use std::{
        io::Write,
        time::{Duration, Instant},
    };
    let seconds = seconds
        .parse::<u64>()
        .ok()
        .filter(|s| (1..=60).contains(s))
        .ok_or("Watch duration must be an integer from 1 to 60 seconds.")?;
    let mut watcher = our_clipboard::watch::Watcher::new()?;
    println!(
        "Watching: {}. Clipboard contents are never read.",
        watcher.backend_name()
    );
    std::io::stdout()
        .flush()
        .map_err(|_| "Could not write watch status.")?;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut count = 0_u64;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        if watcher.wait(remaining)? {
            count += 1;
        }
    }
    Ok(format!(
        "Clipboard notifications: {count}. Synchronization is not implemented."
    ))
}

#[cfg(feature = "clipboard")]
fn clipboard_doctor() -> Result<String, &'static str> {
    let clipboard = TextClipboard::new()?;
    Ok(format!(
        "OS: {}\nArchitecture: {}\nClipboard backend: {}\nBackend initialization: passed\nRead/write verification: not performed\nSynchronization: not implemented",
        std::env::consts::OS,
        std::env::consts::ARCH,
        clipboard.backend_name()
    ))
}

#[cfg(feature = "clipboard")]
fn clipboard_probe() -> Result<String, &'static str> {
    // Never read the user's old clipboard or include backend errors in logs.
    let mut clipboard = TextClipboard::new()?;
    let text = "Our Clipboard synthetic probe\nZażółć gęślą jaźń 🦀";
    clipboard.set_text(text)?;
    let observed = clipboard.get_text()?;
    if observed != text {
        return Err("Clipboard verification failed; contents are omitted.");
    }
    Ok(format!(
        "Synthetic clipboard round trip passed ({}). Synchronization is not implemented.",
        clipboard.backend_name()
    ))
}

fn main() -> ExitCode {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
    }
}
