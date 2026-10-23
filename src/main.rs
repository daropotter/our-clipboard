use std::process::ExitCode;

const HELP: &str = "Our Clipboard — early development

Usage: our-clipboard <COMMAND>

Commands:
  doctor      Report runtime platform information
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
        ["doctor"] => Ok(format!(
            "OS: {}\nArchitecture: {}\nSynchronization: not implemented",
            std::env::consts::OS,
            std::env::consts::ARCH
        )),
        _ => Err("Unknown command or unexpected arguments. Run 'our-clipboard help'."),
    }
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
