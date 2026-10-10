//! Records the compiler version for benchmark reports.

fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
    let version = std::process::Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map_or_else(|| "unknown".to_owned(), |v| v.trim().to_owned());
    println!("cargo:rustc-env=BI_RUSTC_VERSION={version}");
    println!("cargo:rerun-if-env-changed=RUSTC");
}
