//! `sv-kernel`: `init-kernel supervise --under-root` for process-level tests (fds 3 and 4).
fn main() {
    std::process::exit(kernel_supervise::main_under_root(std::env::args().skip(1).collect()));
}
