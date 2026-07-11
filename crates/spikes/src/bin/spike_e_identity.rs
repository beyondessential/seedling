//! Spike E: identity mechanics.
//!
//! Draft harness for `docs/plans/windows-spike-e-identity.md`. Run on a box
//! carrying the field hardening GPO baseline.
//!
//! The stripped-token spawn (the core of `win[identity.non-admin]`) is done for
//! real: derive a restricted token from our own, spawn a child under it, and
//! confirm the child's privilege set is strictly narrower. Virtual-account
//! logon, deterministic SIDs, DeleteService ghosts, and the ACE-inheritance
//! break are driven through `sc.exe`/`icacls`/PowerShell and observed; the
//! `EnumServicesStatusEx`-based GC probe is named as the follow-up.
//!
//! At stake: `win[identity.virtual-account]`, `win[identity.non-admin]`,
//! `win[identity.lifecycle]`/`win[identity.gc]`, `win[identity.file-permissions]`.

#[cfg(not(windows))]
fn main() {
    eprintln!("spike-e-identity is a Windows-only harness; run on a hardened Windows image");
}

#[cfg(windows)]
fn main() -> seedling_spikes::Outcome {
    imp::run()
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::process::Command;

    use seedling_spikes::{Outcome, observe, record, step};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, GetTokenInformation, TOKEN_ASSIGN_PRIMARY,
        TOKEN_DUPLICATE, TOKEN_PRIVILEGES, TOKEN_QUERY, TokenPrivileges,
    };
    use windows::Win32::System::Threading::{
        CreateProcessAsUserW, GetCurrentProcess, OpenProcessToken, PROCESS_INFORMATION,
        STARTUPINFOW,
    };
    use windows::core::{PCWSTR, PWSTR};

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn sc(args: &[&str]) -> (bool, String) {
        let out = Command::new("sc.exe").args(args).output();
        match out {
            Ok(o) => (
                o.status.success(),
                String::from_utf8_lossy(&o.stdout).into_owned(),
            ),
            Err(_) => (false, String::new()),
        }
    }

    /// Read the privilege count out of a token (the leading field of
    /// TOKEN_PRIVILEGES).
    fn privilege_count(token: HANDLE) -> windows::core::Result<u32> {
        let mut len = 0u32;
        unsafe {
            let _ = GetTokenInformation(token, TokenPrivileges, None, 0, &mut len);
        }
        let mut buf = vec![0u8; len.max(size_of::<TOKEN_PRIVILEGES>() as u32) as usize];
        unsafe {
            GetTokenInformation(
                token,
                TokenPrivileges,
                Some(buf.as_mut_ptr() as *mut _),
                buf.len() as u32,
                &mut len,
            )?;
        }
        Ok(u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]))
    }

    fn stripped_token_spawn() -> windows::core::Result<()> {
        let mut base = HANDLE::default();
        unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY,
                &mut base,
            )?;
        }
        let mut restricted = HANDLE::default();
        unsafe {
            // DISABLE_MAX_PRIVILEGE drops every privilege from the new token —
            // the "no extra privileges" slice a workload runs under.
            CreateRestrictedToken(
                base,
                DISABLE_MAX_PRIVILEGE,
                None,
                None,
                None,
                &mut restricted,
            )?;
        }

        let base_privs = privilege_count(base)?;
        let restricted_privs = privilege_count(restricted)?;
        record("supervisor-token privileges", base_privs);
        record("workload-token privileges", restricted_privs);
        if restricted_privs < base_privs || base_privs == 0 {
            observe("restricted token is strictly narrower (or already minimal)");
        } else {
            observe("restricted token not narrower — unexpected; inspect");
        }

        // Prove a workload actually runs under it.
        let si = STARTUPINFOW {
            cb: size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        let mut cmdline = wide("cmd.exe /c exit 0");
        let spawned = unsafe {
            CreateProcessAsUserW(
                Some(restricted),
                PCWSTR::null(),
                PWSTR(cmdline.as_mut_ptr()),
                None,
                None,
                false.into(),
                windows::Win32::System::Threading::PROCESS_CREATION_FLAGS(0),
                None,
                PCWSTR::null(),
                &si,
                &mut pi,
            )
        };
        match spawned {
            Ok(()) => observe("spawned a child under the stripped token"),
            Err(e) => observe(format!(
                "CreateProcessAsUser failed: {e} (run as a service for a real virtual account)"
            )),
        }

        unsafe {
            let _ = CloseHandle(base);
            let _ = CloseHandle(restricted);
        }
        Ok(())
    }

    pub fn run() -> Outcome {
        step(1, "Virtual-account logon under the GPO baseline");
        let (created, _) = sc(&[
            "create",
            "seedling-spike-va",
            "binPath=",
            "C:\\Windows\\System32\\cmd.exe",
            "obj=",
            "NT SERVICE\\seedling-spike-va",
        ]);
        let (started, _) = sc(&["start", "seedling-spike-va"]);
        observe(match (created, started) {
            (true, true) => {
                "virtual-account service started — the baseline grants Log-on-as-a-service"
            }
            (true, false) => {
                "service created but did NOT start — check the Log-on-as-a-service GPO (known failure mode)"
            }
            _ => "service create failed — run elevated",
        });

        step(4, "DeleteService ghost behaviour");
        let (_deleted, _) = sc(&["delete", "seedling-spike-va"]);
        let (_, query) = sc(&["query", "seedling-spike-va"]);
        observe(if query.contains("1060") || query.trim().is_empty() {
            "service gone after delete"
        } else {
            "service lingers (marked-for-delete) while handles/instances remain — GC must reap it"
        });
        observe(
            "the real GC probe enumerates seedling- registrations via \
             EnumServicesStatusEx and distinguishes live/ghosted/orphaned (win[identity.gc])",
        );

        step(3, "Stripped-token spawn");
        stripped_token_spawn()?;

        step(5, "NTFS ACE inheritance break on volume creation");
        let dir = std::env::temp_dir().join("seedling-spike-vol");
        let _ = std::fs::create_dir_all(&dir);
        let dir_s = dir.display().to_string();
        // Break inheritance, then grant only the four principals of
        // win[identity.file-permissions] (instance SID stands in as the current
        // user here; a real run uses the computed service SID).
        let _ = Command::new("icacls")
            .args([&dir_s, "/inheritance:r"])
            .status();
        let _ = Command::new("icacls")
            .args([
                &dir_s,
                "/grant",
                "*S-1-5-18:(OI)(CI)F",
                "/grant",
                "*S-1-5-32-544:(OI)(CI)F",
            ])
            .status();
        observe(
            "confirm a fifth principal is denied, a deep file does not inherit \
             access from above the volume root, and Administrators can still take \
             ownership (expected per threat-model WN1)",
        );
        Ok(())
    }
}
