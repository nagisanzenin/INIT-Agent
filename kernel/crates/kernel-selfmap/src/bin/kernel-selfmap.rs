#![forbid(unsafe_code)]
//! `kernel-selfmap`: the selfmap CLI on its own (the same arguments as `init-kernel selfmap …`).

fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let exe = std::env::current_exe()
        .map(|p| p.into_os_string())
        .unwrap_or_else(|_| "kernel-selfmap".into());
    std::process::exit(kernel_selfmap::cli::run(&args, &[exe]));
}
