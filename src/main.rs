mod comments;
mod discover;
mod github;
mod inventory;
mod output;
mod prefilter;
mod references;
mod search;

fn main() -> std::process::ExitCode {
    output::entry()
}
