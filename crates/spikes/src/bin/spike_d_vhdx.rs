//! Spike D: run Tamanu from a read-only VHDX.
//!
//! Draft harness for `docs/plans/windows-spike-d-vhdx-run.md`.
//!
//! Hashes a produced artifact, attaches it read-only with no drive letter,
//! resolves the physical path, then detaches and re-hashes to prove the attach
//! left the image bit-identical. Launching Tamanu from the config blob and the
//! Process Monitor write-hunt are manual follow-ups noted in the output.
//!
//! Usage: `spike-d-vhdx <path-to.vhdx>`
//!
//! At stake: `win[artifact.attach]` (read-only attach), `win[artifact.verify]`
//! (per-attach digest cost), `win[artifact.rebase]`.

#[cfg(not(windows))]
fn main() {
    eprintln!("spike-d-vhdx is a Windows-only harness; run on Windows Server 2019+");
}

#[cfg(windows)]
fn main() -> seedling_spikes::Outcome {
    imp::run()
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::time::Instant;

    use seedling_spikes::{Outcome, observe, record, step};
    use sha2::{Digest, Sha256};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Storage::Vhd::{
        ATTACH_VIRTUAL_DISK_FLAG_NO_DRIVE_LETTER, ATTACH_VIRTUAL_DISK_FLAG_READ_ONLY,
        AttachVirtualDisk, DETACH_VIRTUAL_DISK_FLAG_NONE, DetachVirtualDisk,
        GetVirtualDiskPhysicalPath, OPEN_VIRTUAL_DISK_FLAG_NONE, OpenVirtualDisk,
        VIRTUAL_DISK_ACCESS_ALL, VIRTUAL_STORAGE_TYPE, VIRTUAL_STORAGE_TYPE_DEVICE_VHDX,
        VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT,
    };
    use windows::core::PCWSTR;

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn sha256_file(path: &str) -> std::io::Result<String> {
        let bytes = std::fs::read(path)?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        Ok(hex(&hasher.finalize()))
    }

    fn hex(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    fn open_vhdx(path: &str) -> windows::core::Result<HANDLE> {
        let mut storage = VIRTUAL_STORAGE_TYPE {
            DeviceId: VIRTUAL_STORAGE_TYPE_DEVICE_VHDX,
            VendorId: VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT,
        };
        let mut handle = HANDLE::default();
        let path_w = wide(path);
        let err = unsafe {
            OpenVirtualDisk(
                &mut storage,
                PCWSTR(path_w.as_ptr()),
                VIRTUAL_DISK_ACCESS_ALL,
                OPEN_VIRTUAL_DISK_FLAG_NONE,
                None,
                &mut handle,
            )
        };
        err.ok()?;
        Ok(handle)
    }

    fn physical_path(handle: HANDLE) -> windows::core::Result<String> {
        let mut len = 512u32;
        let mut buf = vec![0u16; len as usize];
        unsafe { GetVirtualDiskPhysicalPath(handle, &mut len, PCWSTR(buf.as_mut_ptr())).ok()? };
        let s = String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)]);
        Ok(s)
    }

    pub fn run() -> Outcome {
        let path = match std::env::args().nth(1) {
            Some(p) => p,
            None => {
                eprintln!("usage: spike-d-vhdx <path-to.vhdx>");
                return Ok(());
            }
        };

        step(1, "Verify the uncompressed image (per-attach cost)");
        let started = Instant::now();
        let before = sha256_file(&path)?;
        record("pre-attach sha256", &before);
        record("verify wall-clock", format!("{:?}", started.elapsed()));
        observe("if this is not negligible, propose per-boot cadence in win[artifact.verify]");

        step(2, "Attach read-only, no drive letter");
        let handle = open_vhdx(&path)?;
        let err = unsafe {
            AttachVirtualDisk(
                handle,
                None,
                ATTACH_VIRTUAL_DISK_FLAG_READ_ONLY | ATTACH_VIRTUAL_DISK_FLAG_NO_DRIVE_LETTER,
                0,
                None,
                None,
            )
        };
        err.ok()?;
        match physical_path(handle) {
            Ok(p) => record("attached device path", p),
            Err(e) => observe(format!("physical path query failed: {e}")),
        }
        observe(
            "read-only attach neutralises the dirty bit and $LogFile replay; \
             resolve entrypoint from the config blob and rebase WorkingDir/PATH \
             per win[artifact.rebase], then launch Tamanu (manual follow-up)",
        );

        step(5, "Detach and confirm the image is bit-identical");
        unsafe {
            DetachVirtualDisk(handle, DETACH_VIRTUAL_DISK_FLAG_NONE, 0).ok()?;
            CloseHandle(handle)?;
        }
        let after = sha256_file(&path)?;
        if before == after {
            observe("attach/detach left the image unchanged");
        } else {
            record("post-attach sha256", &after);
            observe("MISMATCH: read-only attach wrote to the image — investigate");
        }

        observe(
            "remaining manual steps: Process Monitor write-hunt under the mount \
             point, and the corrupted-store-entry quarantine check for win[artifact.verify]",
        );
        Ok(())
    }
}
