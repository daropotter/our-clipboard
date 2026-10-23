use our_clipboard::clipboard::TextClipboard;
use std::process::ExitCode;

const HELP: &str = "Our Clipboard — early development

Usage: our-clipboard <COMMAND>

Commands:
  doctor      Report runtime platform information
  clipboard-probe --replace-clipboard
              Replace the clipboard with synthetic text and verify access
  help        Show this help
  --version   Show version

Clipboard synchronization is not implemented yet.";

fn run(args: &[String]) -> Result<String, &'static str> {
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] | ["help"] | ["--help"] | ["-h"] => Ok(HELP.into()),
        ["--version"] | ["-V"] => Ok(format!("our-clipboard {}", env!("CARGO_PKG_VERSION"))),
        ["clipboard-probe", "--replace-clipboard"] => clipboard_probe(),
        ["doctor"] => Ok(format!(
            "OS: {}\nArchitecture: {}\nSynchronization: not implemented",
            std::env::consts::OS,
            std::env::consts::ARCH
        )),
        _ => Err("Unknown command or unexpected arguments. Run 'our-clipboard help'."),
    }
}

fn clipboard_probe() -> Result<String, &'static str> {
    // Never read the user's old clipboard or include backend errors in logs.
    let mut clipboard = TextClipboard::new()?;
    let text = "Our Clipboard synthetic probe\nZażółć gęślą jaźń 🦀";
    clipboard
        .set_text(text)
        .map_err(|_| "Clipboard write failed.")?;
    let observed = clipboard.get_text().map_err(|_| "Clipboard read failed.")?;
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
