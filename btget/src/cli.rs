use bt::magnet::{self, Magnet};
use std::path::PathBuf;
use std::time::Duration;

pub const USAGE: &str = "usage: btget <torrent-file | magnet-link> [options]

options:
  -o, --output <dir>     output directory (default .)
  -p, --port <port>      listen port announced to peers (default 6881)
      --max-peers <n>    peer connection limit (default 40)
      --metadata-timeout <s>
                         seconds to keep looking for magnet metadata
                         (default: keep trying until interrupted)
  -v, --verbose          also log per-peer connections and failures
  -q, --quiet            only the result line and errors
  -h, --help             show this help";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Torrent(PathBuf),
    Magnet(Magnet),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub input: Input,
    pub output_dir: PathBuf,
    pub port: u16,
    pub max_peers: usize,
    pub metadata_timeout: Option<Duration>,
    pub verbose: bool,
    pub quiet: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cli {
    Run(Config),
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    MissingInput,
    ExtraInput(String),
    UnknownOption(String),
    MissingValue(&'static str),
    BadValue(&'static str, String),
    BadMagnet(magnet::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::MissingInput => write!(f, "no torrent file or magnet link given"),
            Error::ExtraInput(arg) => write!(f, "unexpected extra input: {}", arg),
            Error::UnknownOption(arg) => write!(f, "unknown option: {}", arg),
            Error::MissingValue(opt) => write!(f, "option {} needs a value", opt),
            Error::BadValue(opt, got) => write!(f, "bad value for {}: {}", opt, got),
            Error::BadMagnet(e) => write!(f, "bad magnet link: {:?}", e),
        }
    }
}

pub fn parse(args: &[String]) -> Result<Cli, Error> {
    let mut input = None;
    let mut output_dir = PathBuf::from(".");
    let mut port = 6881u16;
    let mut max_peers = 40usize;
    let mut metadata_timeout = None;
    let mut verbose = false;
    let mut quiet = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Cli::Help),
            "-v" | "--verbose" => verbose = true,
            "-q" | "--quiet" => quiet = true,
            "-o" | "--output" => {
                let value = iter.next().ok_or(Error::MissingValue("--output"))?;
                output_dir = PathBuf::from(value);
            }
            "-p" | "--port" => {
                let value = iter.next().ok_or(Error::MissingValue("--port"))?;
                port = value
                    .parse()
                    .map_err(|_| Error::BadValue("--port", value.clone()))?;
            }
            "--max-peers" => {
                let value = iter.next().ok_or(Error::MissingValue("--max-peers"))?;
                max_peers = value
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| Error::BadValue("--max-peers", value.clone()))?;
            }
            "--metadata-timeout" => {
                let value = iter.next().ok_or(Error::MissingValue("--metadata-timeout"))?;
                let seconds: u64 = value
                    .parse()
                    .map_err(|_| Error::BadValue("--metadata-timeout", value.clone()))?;
                metadata_timeout = match seconds {
                    0 => None,
                    n => Some(Duration::from_secs(n)),
                };
            }
            other if other.starts_with('-') && other.len() > 1 => {
                return Err(Error::UnknownOption(other.to_string()))
            }
            other => {
                if input.is_some() {
                    return Err(Error::ExtraInput(other.to_string()));
                }
                input = Some(parse_input(other)?);
            }
        }
    }
    let input = input.ok_or(Error::MissingInput)?;
    Ok(Cli::Run(Config {
        input,
        output_dir,
        port,
        max_peers,
        metadata_timeout,
        verbose,
        quiet,
    }))
}

fn parse_input(raw: &str) -> Result<Input, Error> {
    if raw.starts_with("magnet:") {
        magnet::parse(raw).map(Input::Magnet).map_err(Error::BadMagnet)
    } else {
        Ok(Input::Torrent(PathBuf::from(raw)))
    }
}
