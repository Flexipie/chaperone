use clap::Parser;

#[derive(Parser)]
#[command(name = "seamlane", version, about)]
struct Cli {}

fn main() {
    Cli::parse();
}
