//! `rvw`: a per-workspace queue of code review feedback.
fn main() {
    std::process::exit(rvw_cli::run(&std::env::args().skip(1).collect::<Vec<_>>()));
}
