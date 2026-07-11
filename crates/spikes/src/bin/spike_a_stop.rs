//! Spike A: stop delivery + Job Objects.
//!
//! Draft harness for `docs/plans/windows-spike-a-stop-delivery.md`.
//!
//! Supervisor mode (no args) spawns this same binary in `child` mode inside a
//! Job Object and a new process group, delivers CTRL_BREAK to the group,
//! observes cooperative shutdown and exit-code capture, then exercises
//! `TerminateJobObject` exit-code synthesis. `latency` mode times
//! CreateService+StartService+DeleteService via `sc.exe` for the dynamic-Job
//! dispatch decision.
//!
//! At stake: `win[stop.methods]` (ctrl_break/ctrl_c), `win[signal.exit-codes]`,
//! and the dispatch-latency input to the `win[identity.dynamic-jobs]` retreat.

#[cfg(not(windows))]
fn main() {
    eprintln!("spike-a-stop is a Windows-only harness; run on Windows Server 2019+");
}

#[cfg(windows)]
fn main() -> seedling_spikes::Outcome {
    match std::env::args().nth(1).as_deref() {
        Some("child") => imp::child(),
        Some("latency") => imp::latency(),
        _ => imp::supervisor(),
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use seedling_spikes::{Outcome, observe, record, step};
    use windows::Win32::Foundation::{HANDLE, TRUE, WAIT_OBJECT_0};
    use windows::Win32::System::Console::{
        CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler,
    };
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject,
    };
    use windows::Win32::System::Threading::{
        CREATE_NEW_PROCESS_GROUP, CREATE_SUSPENDED, CreateProcessW, GetExitCodeProcess,
        PROCESS_INFORMATION, ResumeThread, STARTUPINFOW, WaitForSingleObject,
    };
    use windows::core::{PCWSTR, PWSTR};

    /// Exit code the child uses when it shuts down cooperatively, so the
    /// supervisor can tell a clean stop from a synthesised termination.
    const CHILD_CLEAN_EXIT: u32 = 42;
    /// Completion code handed to `TerminateJobObject`.
    const JOB_KILL_CODE: u32 = 0xDEAD;

    static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    // --- Child ----------------------------------------------------------

    unsafe extern "system" fn child_ctrl_handler(ctrl_type: u32) -> windows::core::BOOL {
        // CTRL_BREAK_EVENT arrives here for a process in a new group. Record the
        // request and let the main loop flush and exit; returning TRUE claims
        // the event as handled.
        if ctrl_type == CTRL_BREAK_EVENT {
            STOP_REQUESTED.store(true, Ordering::SeqCst);
        }
        TRUE
    }

    pub fn child() -> Outcome {
        unsafe { SetConsoleCtrlHandler(Some(child_ctrl_handler), true)? };
        // Simulate a workload that runs until asked to stop, then does shutdown
        // work (the flush a script depends on) before exiting with a
        // distinctive code.
        for _ in 0..600 {
            if STOP_REQUESTED.load(Ordering::SeqCst) {
                // Stand-in for real shutdown work (flush, checkpoint).
                std::thread::sleep(Duration::from_millis(50));
                std::process::exit(CHILD_CLEAN_EXIT as i32);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        std::process::exit(1)
    }

    // --- Supervisor -----------------------------------------------------

    unsafe extern "system" fn supervisor_ctrl_handler(_ctrl_type: u32) -> windows::core::BOOL {
        // Shield the supervisor: if a group-targeted event ever strikes us, we
        // must not die. Claiming it lets the harness observe that the child's
        // group event did not reach the supervisor.
        TRUE
    }

    fn spawn_child_in_job(job: HANDLE) -> windows::core::Result<PROCESS_INFORMATION> {
        let exe = std::env::current_exe().expect("current_exe");
        let mut cmdline = wide(&format!("\"{}\" child", exe.display()));
        let si = STARTUPINFOW {
            cb: size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        unsafe {
            CreateProcessW(
                PCWSTR::null(),
                Some(PWSTR(cmdline.as_mut_ptr())),
                None,
                None,
                // No inherited handles: the child must not hold anything of ours.
                false,
                CREATE_NEW_PROCESS_GROUP | CREATE_SUSPENDED,
                None,
                PCWSTR::null(),
                &si,
                &mut pi,
            )?;
            // Enclose before it runs a single instruction, per win[supervisor.ownership].
            AssignProcessToJobObject(job, pi.hProcess)?;
            ResumeThread(pi.hThread);
        }
        Ok(pi)
    }

    fn exit_code(process: HANDLE) -> windows::core::Result<u32> {
        let mut code = 0u32;
        unsafe { GetExitCodeProcess(process, &mut code)? };
        Ok(code)
    }

    pub fn supervisor() -> Outcome {
        unsafe { SetConsoleCtrlHandler(Some(supervisor_ctrl_handler), true)? };

        step(1, "CTRL_BREAK to a child's process group under a Job");
        let job = unsafe { CreateJobObjectW(None, PCWSTR::null())? };
        let pi = spawn_child_in_job(job)?;
        // The child's process-group id equals its PID (it leads a new group).
        std::thread::sleep(Duration::from_millis(300));
        unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pi.dwProcessId)? };

        let waited = unsafe { WaitForSingleObject(pi.hProcess, 5_000) };
        if waited == WAIT_OBJECT_0 {
            let code = exit_code(pi.hProcess)?;
            observe(format!("child exited with code {code}"));
            if code == CHILD_CLEAN_EXIT {
                observe("cooperative shutdown ran and the exit code was captured");
            } else {
                observe("child exited but not via the cooperative path — investigate");
            }
        } else {
            observe("child did not exit within timeout — CTRL_BREAK not delivered?");
        }
        observe("supervisor still alive (its own handler shielded any stray event)");

        step(2, "TerminateJobObject exit-code synthesis");
        let job2 = unsafe { CreateJobObjectW(None, PCWSTR::null())? };
        let pi2 = spawn_child_in_job(job2)?;
        std::thread::sleep(Duration::from_millis(300));
        unsafe { TerminateJobObject(job2, JOB_KILL_CODE)? };
        let _ = unsafe { WaitForSingleObject(pi2.hProcess, 5_000) };
        let code2 = exit_code(pi2.hProcess)?;
        record("raw job kill code", format!("0x{code2:X}"));
        observe(
            "runtime maps this to a negative recorded exit code per win[signal.exit-codes]; \
             confirm no cooperative-exit race during the stop_timeout wait",
        );

        step(3, "CTRL_C stance");
        observe(
            "CTRL_C is not group-targetable the way CTRL_BREAK is, and \
             CREATE_NEW_PROCESS_GROUP disables CTRL_C for the child by default; \
             decide whether ctrl_c survives as a distinct win[stop.methods] value",
        );

        Ok(())
    }

    // --- Dispatch latency ----------------------------------------------

    pub fn latency() -> Outcome {
        step(
            5,
            "CreateService + StartService + DeleteService dispatch latency",
        );
        let iterations = 20u32;
        let mut samples = Vec::with_capacity(iterations as usize);
        for i in 0..iterations {
            let name = format!("seedling-spike-{i}");
            let started = Instant::now();
            // A harness legitimately drives the SCM through sc.exe rather than
            // the raw Win32 CreateServiceW/StartServiceW surface; the number we
            // want is the register-to-running wall-clock edge.
            let create = sc(&[
                "create",
                &name,
                "binPath=",
                "C:\\Windows\\System32\\cmd.exe",
            ]);
            let start = sc(&["start", &name]);
            let edge = started.elapsed();
            let _ = sc(&["stop", &name]);
            let _ = sc(&["delete", &name]);
            if create && start {
                samples.push(edge);
            }
        }
        if samples.is_empty() {
            observe("no successful iterations — run elevated; sc.exe needs admin");
            return Ok(());
        }
        samples.sort();
        let p = |q: f64| samples[((samples.len() as f64 - 1.0) * q).round() as usize];
        record("dispatch p50", format!("{:?}", p(0.50)));
        record("dispatch p95", format!("{:?}", p(0.95)));
        observe(
            "verdict: per-invocation registration is fine, or trigger a retreat \
             from windows-runtime-rationale.md",
        );
        Ok(())
    }

    fn sc(args: &[&str]) -> bool {
        std::process::Command::new("sc.exe")
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}
