//! NovaDB command-line client and interactive `NovaQL` shell.

#![forbid(unsafe_code)]

use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use nova_client::Client;
use nova_core::error::{NovaError, Result};

const DEFAULT_ADDRESS: &str = "127.0.0.1:7400";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Version,
    Query(String),
    Shell,
    Help,
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    address: String,
    token: Option<String>,
    command: Command,
}

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("nova: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Vec<String>) -> Result<()> {
    let options = parse_options(arguments)?;
    match options.command {
        Command::Version => {
            println!("NovaDB {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Help => {
            print_help();
            Ok(())
        }
        Command::Query(query) => {
            let mut client = configured_client(&options.address, options.token)?;
            println!("{}", client.execute(query)?.payload);
            Ok(())
        }
        Command::Shell => shell(configured_client(&options.address, options.token)?),
    }
}

fn configured_client(address: &str, token: Option<String>) -> Result<Client> {
    let mut client = Client::connect(address)?;
    if let Some(token) = token {
        client.set_token(token);
    }
    Ok(client)
}

fn shell(mut client: Client) -> Result<()> {
    println!("NovaDB {} — type 'exit' to quit", env!("CARGO_PKG_VERSION"));
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    loop {
        print!("nova> ");
        io::stdout().flush()?;
        let Some(line) = lines.next() else { break };
        let line = line?;
        let query = line.trim();
        if matches!(query, "exit" | "quit") {
            break;
        }
        if query.is_empty() {
            continue;
        }
        match client.execute(query) {
            Ok(response) => println!("{}", response.payload),
            Err(error) => eprintln!("error: {error}"),
        }
    }
    Ok(())
}

fn parse_options(arguments: Vec<String>) -> Result<Options> {
    let mut address =
        std::env::var("NOVADB_ADDRESS").unwrap_or_else(|_| DEFAULT_ADDRESS.to_owned());
    let mut token = std::env::var("NOVADB_TOKEN").ok();
    let mut positional = Vec::new();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--address" | "-a" => {
                address = arguments.next().ok_or_else(|| {
                    NovaError::InvalidArgument("--address requires a value".to_owned())
                })?;
            }
            "--token" | "-t" => {
                token = Some(arguments.next().ok_or_else(|| {
                    NovaError::InvalidArgument("--token requires a value".to_owned())
                })?);
            }
            "--help" | "-h" => positional.push("help".to_owned()),
            "--version" | "-V" => positional.push("version".to_owned()),
            value if value.starts_with('-') => {
                return Err(NovaError::InvalidArgument(format!(
                    "unknown option {value:?}"
                )));
            }
            value => positional.push(value.to_owned()),
        }
    }
    let command = match positional.first().map(String::as_str) {
        None | Some("shell") => Command::Shell,
        Some("version") if positional.len() == 1 => Command::Version,
        Some("help") if positional.len() == 1 => Command::Help,
        Some("query") if positional.len() > 1 => Command::Query(positional[1..].join(" ")),
        Some("query") => {
            return Err(NovaError::InvalidArgument(
                "query command requires NovaQL text".to_owned(),
            ));
        }
        Some(other) => {
            return Err(NovaError::InvalidArgument(format!(
                "unknown command {other:?}"
            )));
        }
    };
    Ok(Options {
        address,
        token,
        command,
    })
}

fn print_help() {
    println!(
        "NovaDB command-line client\n\n\
         Usage:\n  nova [--address HOST:PORT] [--token TOKEN] shell\n  \
         nova [options] query <NovaQL>\n  nova version\n\n\
         Environment: NOVADB_ADDRESS, NOVADB_TOKEN"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands_and_options() {
        let options = parse_options(vec![
            "--address".to_owned(),
            "127.0.0.1:9999".to_owned(),
            "--token".to_owned(),
            "secret".to_owned(),
            "query".to_owned(),
            "students.get".to_owned(),
            "{}".to_owned(),
        ])
        .unwrap();
        assert_eq!(options.address, "127.0.0.1:9999");
        assert_eq!(options.token.as_deref(), Some("secret"));
        assert_eq!(
            options.command,
            Command::Query("students.get {}".to_owned())
        );
    }

    #[test]
    fn rejects_unknown_and_incomplete_arguments() {
        assert!(parse_options(vec!["--bad".to_owned()]).is_err());
        assert!(parse_options(vec!["query".to_owned()]).is_err());
        assert!(parse_options(vec!["--address".to_owned()]).is_err());
    }
}
