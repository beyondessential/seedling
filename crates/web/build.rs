use std::path::Path;
use std::process::Command;
use std::{env, fs};

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let frontend = Path::new(&manifest_dir).join("frontend");
    let dist = frontend.join("dist");

    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/package.json");
    println!("cargo:rerun-if-changed=frontend/vite.config.ts");
    println!("cargo:rerun-if-changed=frontend/index.html");

    if env::var("SKIP_FRONTEND_BUILD").is_ok() {
        assert!(
            dist.exists(),
            "SKIP_FRONTEND_BUILD set but frontend/dist does not exist; run `npm run build` first"
        );
        return;
    }

    let npm = which_npm();
    if npm.is_none() {
        assert!(
            dist.exists(),
            "npm not found and frontend/dist does not exist; install npm or run the build manually"
        );
        return;
    }
    let npm = npm.unwrap();

    let status = Command::new(&npm)
        .args(["install", "--frozen-lockfile"])
        .current_dir(&frontend)
        .status()
        .expect("failed to run npm install");
    assert!(status.success(), "npm install failed");

    let status = Command::new(&npm)
        .args(["run", "build"])
        .current_dir(&frontend)
        .status()
        .expect("failed to run npm run build");
    assert!(status.success(), "npm run build failed");

    // Touch a sentinel so cargo knows the build script output changed.
    fs::write(dist.join(".build-stamp"), "").ok();
}

fn which_npm() -> Option<String> {
    let candidates = ["npm", "npm.cmd"];
    for candidate in candidates {
        if Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return Some(candidate.to_owned());
        }
    }
    None
}
