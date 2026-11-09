use fr::store::Store;
use fr::ui::{Pal, Terminal};
use fr::{cli, Error};
use std::io::{self, ErrorKind, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match try_main(&args) {
        Ok(()) => ExitCode::SUCCESS,
        // `fr list | head` closing the pipe early is not an error.
        Err(Error::Os(e)) if e.kind() == ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(Error::Cancelled) => {
            eprintln!("\n  {}", Pal::stderr().dim("Cancelled."));
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("{} {e}", Pal::stderr().red("error:"));
            ExitCode::FAILURE
        }
    }
}

fn try_main(args: &[String]) -> fr::Result<()> {
    let command = cli::Command::parse(args)?;
    let store = Store::from_env()?;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    cli::execute(command, &store, &mut Terminal::new(), &mut out)?;
    out.flush()?;
    Ok(())
}
