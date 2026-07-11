//! Spike C: networking on a worst-case image.
//!
//! Draft harness for `docs/plans/windows-spike-c-networking.md`. Run on a field
//! disk image with the real GPO baseline and EDR, not a lab box.
//!
//! The control-plane steps (loopback aliases, `skipassource`, NRPT) are driven
//! through `netsh`/PowerShell, which is how a harness legitimately pokes them.
//! The bind-verify probe uses the extended TCP table via IP Helper. The WFP
//! provider/sublayer/filter install is left as an explicit stub naming the FWPM
//! surface, because a full, correct FWPM sequence is too large to draft without
//! a Windows target to check against.
//!
//! At stake: `win[net.prefix]` (aliases, Q1 v4 fallback), `win[net.resolver]`
//! (NRPT), `win[wfp.*]`, and `win[net.bind-verify]`.

#[cfg(not(windows))]
fn main() {
    eprintln!("spike-c-net is a Windows-only harness; run on a field Windows image");
}

#[cfg(windows)]
fn main() -> seedling_spikes::Outcome {
    imp::run()
}

#[cfg(windows)]
mod imp {
    use std::process::Command;

    use seedling_spikes::{Outcome, observe, record, step};
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, TCP_TABLE_OWNER_PID_ALL,
    };
    use windows::Win32::Networking::WinSock::AF_INET6;

    /// A representative address inside a derived Seedling prefix. A real run
    /// derives the prefix from `MachineGuid`; the harness takes it as arg 1 so
    /// the same binary works across hosts.
    fn seedling_addr() -> String {
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "fd5e:ed00:0000::a".to_string())
    }

    fn netsh(args: &[&str]) -> bool {
        Command::new("netsh")
            .args(args)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn powershell(script: &str) -> bool {
        Command::new("powershell")
            .args(["-NoProfile", "-Command", script])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    pub fn run() -> Outcome {
        let addr = seedling_addr();

        step(1, "Loopback alias with skipassource");
        // Add the address to the loopback interface and mark it skipassource so
        // it is never chosen as an outbound source (win[net.prefix]).
        let added = netsh(&[
            "interface",
            "ipv6",
            "add",
            "address",
            "Loopback Pseudo-Interface 1",
            &addr,
            "skipassource=true",
        ]);
        observe(if added {
            "alias added; confirm no host software selects it as a source address"
        } else {
            "alias add failed — run elevated and check the loopback interface name"
        });

        step(2, "v4 fallback stance (Q1)");
        observe(
            "establish whether a v6-incapable dialler can reach a per-service \
             127.x alias relayed by the supervisor, or whether no v4 fallback is \
             offered; write the answer into win[net.prefix]",
        );

        step(3, "NRPT scoping for the Seedling zone");
        let nrpt = powershell(
            "Add-DnsClientNrptRule -Namespace '.seedling.internal' \
             -NameServers 'fd5e:ed00::53'",
        );
        observe(if nrpt {
            "NRPT rule added; confirm global resolution is untouched and removal restores prior state"
        } else {
            "NRPT add failed — check for conflicting corporate NRPT policy"
        });
        let _ = powershell(
            "Get-DnsClientNrptRule | Where-Object Namespace -eq '.seedling.internal' \
             | Remove-DnsClientNrptRule -Force",
        );

        step(
            4,
            "WFP provider/sublayer/filters under the deployment's EDR",
        );
        observe(
            "install persistent objects under the fixed Seedling provider GUID; \
             confirm ALE connect- and bind-layer default-deny holds with seedlingd \
             stopped, and that the EDR's own callouts keep working alongside ours",
        );
        // FwpmEngineOpen0 -> FwpmProviderAdd0 -> FwpmSubLayerAdd0 -> FwpmFilterAdd0
        // (ALE_AUTH_CONNECT / ALE_RESOURCE_ASSIGNMENT), then a provider-scoped
        // FwpmFilterDeleteById0 sweep. Drafted separately against a Windows target.
        // Deliberately not stubbed with todo!() so the rest of the harness runs.

        step(5, "BIND_ADDRESS end-to-end + bind-verify");
        match owner_pids_listening_v6() {
            Ok(count) => record("v6 listeners visible via extended TCP table", count),
            Err(e) => observe(format!("extended TCP table query failed: {e}")),
        }
        observe(
            "run a real Tamanu build with BIND_ADDRESS set, confirm each entry is \
             held by a process inside the instance's Job, and confirm win[net.bind-verify] \
             faults when the workload binds bare loopback instead",
        );

        Ok(())
    }

    /// Query the IPv6 owner-PID TCP table and return how many rows came back.
    /// The per-row match against a Job's process set is the part a real
    /// bind-verify performs; here we only prove the table is reachable and sized.
    fn owner_pids_listening_v6() -> windows::core::Result<u32> {
        let mut size = 0u32;
        // First call sizes the buffer.
        unsafe {
            GetExtendedTcpTable(
                None,
                &mut size,
                false.into(),
                AF_INET6.0 as u32,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            );
        }
        let mut buf = vec![0u8; size as usize];
        let err = unsafe {
            GetExtendedTcpTable(
                Some(buf.as_mut_ptr() as *mut _),
                &mut size,
                false.into(),
                AF_INET6.0 as u32,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            )
        };
        if err != 0 {
            return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                err as i32,
            )));
        }
        // The buffer is a MIB_TCP6TABLE_OWNER_PID: a u32 dwNumEntries followed by
        // that many MIB_TCP6ROW_OWNER_PID. Read just the count for the draft; the
        // real bind-verify walks the rows and matches LocalAddr:LocalPort against
        // each BIND_ADDRESS entry and the owning Job's PIDs.
        if buf.len() >= 4 {
            let n = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
            Ok(n)
        } else {
            Ok(0)
        }
    }
}
