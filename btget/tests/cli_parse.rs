use btget::cli::{parse, Cli, Config, Error, Input};
use std::path::PathBuf;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn run(list: &[&str]) -> Result<Cli, Error> {
    parse(&args(list))
}

const HEX: &str = "0123456789abcdef0123456789abcdef01234567";

#[test]
fn torrent_path_with_defaults() {
    let cli = run(&["demo.torrent"]).unwrap();
    assert_eq!(
        cli,
        Cli::Run(Config {
            input: Input::Torrent(PathBuf::from("demo.torrent")),
            output_dir: PathBuf::from("."),
            port: 6881,
            max_peers: 40,
            verbose: false,
            quiet: false,
        })
    );
}

#[test]
fn magnet_input_parses() {
    let uri = format!("magnet:?xt=urn:btih:{}", HEX);
    match run(&[&uri]).unwrap() {
        Cli::Run(config) => match config.input {
            Input::Magnet(m) => assert_eq!(m.info_hash.to_hex(), HEX),
            other => panic!("{:?}", other),
        },
        other => panic!("{:?}", other),
    }
}

#[test]
fn options_parse() {
    let cli = run(&["demo.torrent", "-o", "out", "--port", "7000", "--max-peers", "5"]).unwrap();
    match cli {
        Cli::Run(config) => {
            assert_eq!(config.output_dir, PathBuf::from("out"));
            assert_eq!(config.port, 7000);
            assert_eq!(config.max_peers, 5);
        }
        other => panic!("{:?}", other),
    }
}

#[test]
fn verbosity_flags_parse() {
    match run(&["demo.torrent", "-v"]).unwrap() {
        Cli::Run(config) => {
            assert!(config.verbose);
            assert!(!config.quiet);
        }
        other => panic!("{:?}", other),
    }
    match run(&["demo.torrent", "--quiet", "--verbose"]).unwrap() {
        Cli::Run(config) => {
            assert!(config.verbose);
            assert!(config.quiet);
        }
        other => panic!("{:?}", other),
    }
}

#[test]
fn help_wins() {
    assert_eq!(run(&["--help"]).unwrap(), Cli::Help);
    assert_eq!(run(&["demo.torrent", "-h"]).unwrap(), Cli::Help);
}

#[test]
fn missing_input_rejected() {
    assert_eq!(run(&[]), Err(Error::MissingInput));
}

#[test]
fn extra_input_rejected() {
    assert_eq!(
        run(&["a.torrent", "b.torrent"]),
        Err(Error::ExtraInput("b.torrent".into()))
    );
}

#[test]
fn unknown_option_rejected() {
    assert_eq!(run(&["-x", "a.torrent"]), Err(Error::UnknownOption("-x".into())));
}

#[test]
fn missing_value_rejected() {
    assert_eq!(run(&["a.torrent", "-o"]), Err(Error::MissingValue("--output")));
}

#[test]
fn bad_port_rejected() {
    assert_eq!(
        run(&["a.torrent", "--port", "99999"]),
        Err(Error::BadValue("--port", "99999".into()))
    );
}

#[test]
fn zero_max_peers_rejected() {
    assert_eq!(
        run(&["a.torrent", "--max-peers", "0"]),
        Err(Error::BadValue("--max-peers", "0".into()))
    );
}

#[test]
fn bad_magnet_rejected() {
    assert!(matches!(run(&["magnet:?dn=x"]), Err(Error::BadMagnet(_))));
}
