//! `init-fakekernel`: the Root-facing contract of a kernel (testing.md §3.5a). Logic: [`init_testkit::fakekernel`].
fn main() {
    std::process::exit(init_testkit::fakekernel::main());
}
