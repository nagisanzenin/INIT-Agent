//! `init-fakeroot`: the kernel-facing contract of the Root, with the mock egress (testing.md §3.5b).
//! Logic: [`init_testkit::fakeroot`].
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(init_testkit::fakeroot::main_with_args(&args));
}
