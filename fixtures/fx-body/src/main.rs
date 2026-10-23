#![forbid(unsafe_code)]
//! WP15 fixture body: execs WP8b's model-driven `sv-agentbody` (its path is in `agent-path.txt`).

use std::os::unix::process::CommandExt;

const AGENT: &str = include_str!("../agent-path.txt");

fn main() {
    let agent = AGENT.trim();
    let err = std::process::Command::new(agent)
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("exec {agent}: {err}");
    std::process::exit(127);
}
