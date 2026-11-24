mod comments;
mod discover;
mod github;
mod inventory;
mod output;
mod prefilter;
mod references;
mod search;
mod tree;

fn main() -> std::process::ExitCode {
    output::entry()
}
