use btget::cli;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse(&args) {
        Ok(cli::Cli::Help) => println!("{}", cli::USAGE),
        Ok(cli::Cli::Run(_config)) => {
            eprintln!("download engine not implemented yet");
            std::process::exit(3);
        }
        Err(e) => {
            eprintln!("{}", e);
            eprintln!("{}", cli::USAGE);
            std::process::exit(2);
        }
    }
}
