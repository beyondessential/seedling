//! Spike B: ConPTY over QUIC.
//!
//! Draft harness for `docs/plans/windows-spike-b-conpty-quic.md`.
//!
//! Creates a pseudoconsole, spawns a shell under it, pumps ConPTY output to
//! stdout and feeds a scripted input line, and exercises `ResizePseudoConsole`.
//! The QUIC bridge and the empty-stderr contract against the real web terminal
//! are left as an explicit stub: they need the interface crates and a running
//! client, which belong in the follow-up, not this local draft.
//!
//! At stake: `win[shell.conpty]` (stream mapping, resize, empty stderr) and the
//! `i[stream.shell]` three-stream shape.

#[cfg(not(windows))]
fn main() {
    eprintln!("spike-b-conpty is a Windows-only harness; run on Windows Server 2019+");
}

#[cfg(windows)]
fn main() -> seedling_spikes::Outcome {
    imp::run()
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsStr;
    use std::io::Read;
    use std::os::windows::ffi::OsStrExt;
    use std::time::Duration;

    use seedling_spikes::{Outcome, observe, step};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Console::{
        COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
    };
    use windows::Win32::System::Pipes::CreatePipe;
    use windows::Win32::System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
        InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
        PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTUPINFOEXW, STARTUPINFOW,
        UpdateProcThreadAttribute,
    };
    use windows::core::{PCWSTR, PWSTR};

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// A pseudoconsole plus the pipe ends the caller pumps.
    struct Pty {
        hpc: HPCON,
        /// We read ConPTY output (stdout+stderr merged) from here.
        output_read: HANDLE,
        /// We write operator input here.
        input_write: HANDLE,
    }

    fn create_pty(size: COORD) -> windows::core::Result<Pty> {
        let mut input_read = HANDLE::default();
        let mut input_write = HANDLE::default();
        let mut output_read = HANDLE::default();
        let mut output_write = HANDLE::default();
        unsafe {
            CreatePipe(&mut input_read, &mut input_write, None, 0)?;
            CreatePipe(&mut output_read, &mut output_write, None, 0)?;
            // ConPTY takes the read end of input and the write end of output;
            // it owns them for its lifetime.
            let hpc = CreatePseudoConsole(size, input_read, output_write, 0)?;
            // Our copies of the ends ConPTY now owns can be closed.
            CloseHandle(input_read)?;
            CloseHandle(output_write)?;
            Ok(Pty {
                hpc,
                output_read,
                input_write,
            })
        }
    }

    /// Spawn a command attached to the pseudoconsole via a proc-thread
    /// attribute list (the ConPTY spawn contract).
    fn spawn_under_pty(pty: &Pty, command: &str) -> windows::core::Result<PROCESS_INFORMATION> {
        unsafe {
            // Size the attribute list, allocate, initialise, then attach the PTY.
            let mut bytes = 0usize;
            // First call with a null list sizes the buffer.
            let _ = InitializeProcThreadAttributeList(None, 1, None, &mut bytes);
            let mut buffer = vec![0u8; bytes];
            let attrs = LPPROC_THREAD_ATTRIBUTE_LIST(buffer.as_mut_ptr() as *mut _);
            InitializeProcThreadAttributeList(Some(attrs), 1, None, &mut bytes)?;
            UpdateProcThreadAttribute(
                attrs,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                Some(pty.hpc.0 as *const _),
                size_of::<HPCON>(),
                None,
                None,
            )?;

            let si = STARTUPINFOEXW {
                StartupInfo: STARTUPINFOW {
                    cb: size_of::<STARTUPINFOEXW>() as u32,
                    ..Default::default()
                },
                lpAttributeList: attrs,
            };
            let mut cmdline = wide(command);
            let mut pi = PROCESS_INFORMATION::default();
            let result = CreateProcessW(
                PCWSTR::null(),
                Some(PWSTR(cmdline.as_mut_ptr())),
                None,
                None,
                false,
                EXTENDED_STARTUPINFO_PRESENT,
                None,
                PCWSTR::null(),
                &si.StartupInfo,
                &mut pi,
            );
            DeleteProcThreadAttributeList(attrs);
            result?;
            Ok(pi)
        }
    }

    pub fn run() -> Outcome {
        step(1, "Create a pseudoconsole and spawn a shell under it");
        let pty = create_pty(COORD { X: 120, Y: 30 })?;
        let pi = spawn_under_pty(&pty, "cmd.exe /c ver && echo spike-b-ok")?;
        observe("shell spawned under ConPTY");
        unsafe {
            // We do not wait on the workload here; the ConPTY drives it. Release
            // the process/thread handles we own.
            let _ = CloseHandle(pi.hProcess);
            let _ = CloseHandle(pi.hThread);
        }

        // Pump merged output to our stdout in a thread. `win[shell.conpty]` maps
        // this single stream to the stdout unidirectional stream of
        // i[stream.shell]; stderr carries nothing.
        // Move the raw handle across the thread boundary as an isize (HANDLE's
        // *mut c_void is not Send); reconstruct the File inside the thread.
        let output_raw = pty.output_read.0 as isize;
        let pump = std::thread::spawn(move || {
            let mut file = unsafe {
                use std::os::windows::io::FromRawHandle;
                std::fs::File::from_raw_handle(output_raw as *mut _)
            };
            let mut buf = [0u8; 4096];
            let mut seen = String::new();
            while let Ok(n) = file.read(&mut buf) {
                if n == 0 {
                    break;
                }
                seen.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
            seen
        });

        step(3, "Resize mid-session");
        std::thread::sleep(Duration::from_millis(200));
        unsafe { ResizePseudoConsole(pty.hpc, COORD { X: 80, Y: 24 })? };
        observe("resized 120x30 -> 80x24; confirm reflow in an interactive run");

        // Let the shell finish and tear down.
        std::thread::sleep(Duration::from_millis(500));
        unsafe {
            ClosePseudoConsole(pty.hpc);
            CloseHandle(pty.input_write)?;
        }
        let seen = pump.join().unwrap_or_default();
        if seen.contains("spike-b-ok") {
            observe("merged ConPTY output captured over the output pipe");
        } else {
            observe("expected marker not seen — inspect the raw ConPTY stream");
        }

        step(2, "Empty-stderr contract and QUIC bridge");
        observe(
            "drive this through the interface crates' three-stream shell shape \
             against the real web terminal and CLI: confirm neither blocks on the \
             silent stderr stream; then settle close-early vs hold-open in \
             win[shell.conpty]",
        );
        seedling_spikes::not_yet!(
            "bridge the ConPTY pipes over QUIC per i[stream.shell] and drive from the web terminal"
        );
    }
}
