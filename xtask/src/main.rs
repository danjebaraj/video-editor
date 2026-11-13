//! `cargo xtask ci` runs the same checks as CI locally.
use std::process::{exit, Command};

fn run(args: &[&str]) {
    eprintln!("xtask: cargo {}", args.join(" "));
    let st = Command::new(env!("CARGO")).args(args).status().expect("spawn cargo");
    if !st.success() {
        exit(st.code().unwrap_or(1));
    }
}

fn main() {
    let task = std::env::args().nth(1).unwrap_or_default();
    match task.as_str() {
        "ci" => {
            run(&["fmt", "--all", "--check"]);
            run(&["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]);
            run(&["test", "--workspace"]);
            run(&["run", "-p", "cutline-cli", "--", "selftest", "--quick"]);
        }
        _ => {
            eprintln!("usage: cargo xtask ci");
            exit(2);
        }
    }
}
