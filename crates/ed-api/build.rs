// The server says what it is: /v1/version carries the git sha and the
// build time so Settings can show which server the app is on and what
// it was built from (2026-10-07: the maintainer's dev app was on a local
// server built from a stale tree, and nothing on either side could say
// so). CI passes EDDA_GIT_SHA; a local build asks git; neither present
// reads "unknown".
fn main() {
    println!("cargo:rerun-if-env-changed=EDDA_GIT_SHA");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    let sha = std::env::var("EDDA_GIT_SHA").ok().filter(|s| !s.trim().is_empty()).or_else(|| {
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    });
    let sha = sha.unwrap_or_else(|| "unknown".into());
    let built_at = std::process::Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=EDDA_GIT_SHA={sha}");
    println!("cargo:rustc-env=EDDA_BUILT_AT={built_at}");
}
