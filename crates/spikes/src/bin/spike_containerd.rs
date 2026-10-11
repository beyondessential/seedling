//! Spike: containerd survival + control (the decider for the container runtime).
//!
//! Draft harness for `docs/plans/windows-spike-containerd.md`.
//!
//! Proves the make-or-break property: a process-isolated Windows container
//! keeps running across a full containerd service stop/start. Container
//! operations go through `ctr` and the containerd service through `sc`; the
//! crux check — that the workload is alive while containerd is stopped — is
//! done by opening the workload process directly, which does not depend on
//! containerd being up. Run elevated.
//!
//! Usage: `spike-containerd [<image-ref>]`
//! (default: a servercore image; pick a tag matching the host build).
//!
//! At stake: wcr[shim.ownership] (shim outlives containerd),
//! wcr[daemon.reconnect] (containerd re-attaches on restart),
//! wcr[engine.lifecycle] (on-demand start/stop).

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "spike-containerd is a Windows-only harness; run on Windows Server 2019+ \
         with the Containers feature and containerd + ctr installed"
    );
}

#[cfg(windows)]
fn main() -> seedling_spikes::Outcome {
    imp::run()
}

#[cfg(windows)]
mod imp {
    use std::process::Command;

    use seedling_spikes::{Outcome, observe, record, step};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    const SERVICE: &str = "containerd";
    const CONTAINER_ID: &str = "seedling-spike";
    /// `GetExitCodeProcess` reports this while the process is still running.
    const STILL_ACTIVE: u32 = 259;

    /// Run a command, returning (success, combined stdout+stderr trimmed).
    fn sh(program: &str, args: &[&str]) -> std::io::Result<(bool, String)> {
        let out = Command::new(program).args(args).output()?;
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        Ok((out.status.success(), text.trim().to_string()))
    }

    /// True if the PID names a live process, checked without going through
    /// containerd (this is the survival evidence in experiment 3).
    fn pid_alive(pid: u32) -> bool {
        unsafe {
            let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return false;
            };
            let mut code = 0u32;
            let alive = GetExitCodeProcess(handle, &mut code).is_ok() && code == STILL_ACTIVE;
            let _ = CloseHandle(handle);
            alive
        }
    }

    /// Parse the PID column for `id` out of `ctr task ls` output (TASK PID STATUS).
    fn task_pid(task_ls: &str, id: &str) -> Option<u32> {
        for line in task_ls.lines() {
            let mut cols = line.split_whitespace();
            if cols.next() == Some(id) {
                return cols.next().and_then(|c| c.parse().ok());
            }
        }
        None
    }

    fn cleanup() {
        let _ = sh("ctr", &["task", "kill", "-s", "SIGKILL", CONTAINER_ID]);
        let _ = sh("ctr", &["task", "rm", CONTAINER_ID]);
        let _ = sh("ctr", &["container", "rm", CONTAINER_ID]);
    }

    pub fn run() -> Outcome {
        let image = std::env::args()
            .nth(1)
            .unwrap_or_else(|| "mcr.microsoft.com/windows/servercore:ltsc2022".to_string());

        step(
            1,
            "Preflight: containerd service and ctr present, host build",
        );
        let (svc_ok, svc) = sh("sc", &["query", SERVICE])?;
        record(
            "containerd service",
            if svc_ok { "present" } else { "MISSING" },
        );
        observe(svc);
        let (_, ver) = sh("cmd", &["/c", "ver"])?;
        record("host build", ver);
        let (ctr_ok, _) = sh("ctr", &["version"])?;
        if !ctr_ok {
            observe("ctr not reachable; install containerd + ctr and re-run elevated");
            return Ok(());
        }

        step(2, "Start a long-lived process-isolated container");
        cleanup();
        let (pull_ok, pull) = sh("ctr", &["image", "pull", &image])?;
        if !pull_ok {
            observe(pull);
            observe(
                "pull failed; a base matching the host build must be available (wcr[base.image])",
            );
            return Ok(());
        }
        let (run_ok, run_out) = sh(
            "ctr",
            &["run", "-d", &image, CONTAINER_ID, "ping", "-t", "127.0.0.1"],
        )?;
        observe(run_out);
        if !run_ok {
            observe("container failed to start; check the base tag matches the host build");
            return Ok(());
        }

        step(3, "Record the workload PID and its shim");
        let (_, tasks) = sh("ctr", &["task", "ls"])?;
        observe(&tasks);
        let pid = task_pid(&tasks, CONTAINER_ID);
        match pid {
            Some(p) => record("workload task pid", p),
            None => observe("could not parse the task PID; capture it manually before continuing"),
        }
        let (_, shims) = sh(
            "tasklist",
            &["/fi", "IMAGENAME eq containerd-shim-runhcs-v1.exe", "/nh"],
        )?;
        observe(format!("shim: {shims}"));

        step(
            4,
            "Stop containerd; confirm the workload survives (make-or-break)",
        );
        let (_, stop) = sh("sc", &["stop", SERVICE])?;
        observe(stop);
        observe(
            "containerd is down; ctr can no longer reach it, but the container must keep running",
        );
        if let Some(p) = pid {
            if pid_alive(p) {
                observe("PASS: workload alive with containerd stopped (wcr[shim.ownership])");
            } else {
                observe(
                    "FAIL: workload died with containerd — the survival property does not hold; \
                     price the hand-rolled supervisor fallback (option A)",
                );
            }
        }

        step(
            5,
            "Restart containerd; confirm re-attach (wcr[daemon.reconnect])",
        );
        let (_, start) = sh("sc", &["start", SERVICE])?;
        observe(start);
        let (_, tasks2) = sh("ctr", &["task", "ls"])?;
        observe(&tasks2);
        match pid {
            Some(p) => observe(format!(
                "expect task {CONTAINER_ID} RUNNING with pid {p}: containerd re-attached to the surviving shim"
            )),
            None => observe("expect the task listed RUNNING with its original pid"),
        }

        step(6, "On-demand teardown (wcr[engine.lifecycle])");
        cleanup();
        let (_, stop2) = sh("sc", &["stop", SERVICE])?;
        observe(stop2);
        observe(
            "with the world empty, seedlingd stops containerd; the content store and pulled images stay intact for the next start",
        );
        observe(
            "follow-up: drive the same lifecycle over gRPC via the containerd-client crate (control-path criterion)",
        );
        Ok(())
    }
}
