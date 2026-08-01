//! Spike S6: host filesystem behaviour under containers.
//!
//! Draft harness for `docs/plans/windows-spike-host-filesystem.md`. Two
//! questions that share a method — what one side of the container boundary can
//! do to a file the other side is using.
//!
//! Q1: can a host ACL name a container account? Microsoft's guidance is to
//! grant `Authenticated Users` on a bind mount because the container
//! identities do not exist on the host. Access checks compare SIDs rather than
//! names, so an ACE naming the SID literally may still be honoured. If it is,
//! `wcr[volume.host-exposure]` narrows from "any authenticated host principal"
//! to "any container process". ACLs go through `icacls` and containers through
//! `ctr`, because the question is about platform behaviour rather than about
//! any particular API binding.
//!
//! Q2: what can a second process do to a segment the log writer holds open?
//! `wlog[retain.rollover]` puts rollover in the writer on the grounds that an
//! external rename or delete is not reliable while the writer holds the file.
//! The sharing mode is set through `OpenOptionsExt::share_mode`, which is the
//! same `dwShareMode` a writer would pass to `CreateFileW`.
//!
//! Usage: `spike-host-fs [<image-ref>]`
//! (default: a nanoserver image; pick a tag matching the host build).
//! Run elevated — `icacls` and `ctr` both need it.

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "spike-host-fs is a Windows-only harness; run on Windows Server 2019+ \
         with the Containers feature and containerd + ctr installed"
    );
}

#[cfg(windows)]
fn main() -> seedling_spikes::Outcome {
    imp::run()
}

#[cfg(windows)]
mod imp {
    use std::fs::{self, File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use seedling_spikes::{Outcome, observe, record, step};

    /// `dwShareMode` bits, as passed to `CreateFileW`.
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;

    /// Well-known container account SIDs. The spike confirms these resolve the
    /// way the plan assumes rather than trusting the values.
    const CONTAINER_ADMINISTRATOR_SID: &str = "S-1-5-93-2-1";
    const CONTAINER_USER_SID: &str = "S-1-5-93-2-2";

    const DEFAULT_IMAGE: &str = "mcr.microsoft.com/windows/nanoserver:ltsc2022";
    const CONTAINER_A: &str = "seedling-spike-fs-a";
    const CONTAINER_B: &str = "seedling-spike-fs-b";
    /// Where the host directory is mounted inside the container.
    const GUEST_MOUNT: &str = "C:\\mnt";

    /// Run a command, returning (success, combined stdout+stderr trimmed).
    fn sh(program: &str, args: &[&str]) -> std::io::Result<(bool, String)> {
        let out = Command::new(program).args(args).output()?;
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        Ok((out.status.success(), text.trim().to_string()))
    }

    /// First line of a command's output, for logging a verdict compactly.
    fn first_line(s: &str) -> &str {
        s.lines().next().unwrap_or("").trim()
    }

    fn scratch_dir(name: &str) -> std::io::Result<PathBuf> {
        let dir = std::env::temp_dir().join(name);
        if dir.exists() {
            let _ = fs::remove_dir_all(&dir);
        }
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    // -----------------------------------------------------------------------
    // Q1: can a host ACL name a container account?
    // -----------------------------------------------------------------------

    /// Experiment 1: the name must not resolve while the SID must be accepted.
    /// That gap is the premise of the whole question — if the name resolves,
    /// the container accounts are host principals after all and the rest of
    /// the design's reasoning about volume permissions needs revisiting.
    fn q1_resolve(dir: &Path) -> std::io::Result<()> {
        step(1, "Q1 resolve: name lookup vs literal SID");
        let path = dir.to_string_lossy().into_owned();

        let (by_name, out) = sh("icacls", &[&path, "/grant", "ContainerUser:(R)"])?;
        record("grant by name accepted", by_name);
        observe(first_line(&out));
        if by_name {
            observe(
                "UNEXPECTED: the container account name resolved on the host; \
                 re-check the premise of wcr[identity.workload]",
            );
        }

        let grant = format!("*{CONTAINER_USER_SID}:(R)");
        let (by_sid, out) = sh("icacls", &[&path, "/grant", &grant])?;
        record("grant by literal SID accepted", by_sid);
        observe(first_line(&out));
        Ok(())
    }

    /// Experiment 2: the ACE must survive a write/read round trip. An ACL API
    /// that accepts an unresolvable SID and then silently drops it would look
    /// like success at grant time and fail only at access time.
    fn q1_apply(dir: &Path) -> std::io::Result<bool> {
        step(
            2,
            "Q1 apply: set a container-SID-only DACL and read it back",
        );
        let path = dir.to_string_lossy().into_owned();

        // /inheritance:r drops inherited ACEs, so what remains is only what
        // this step granted — otherwise the container could be reaching the
        // directory through an inherited Users ACE and experiment 3 would
        // prove nothing.
        let (_, out) = sh(
            "icacls",
            &[
                &path,
                "/inheritance:r",
                "/grant",
                "*S-1-5-32-544:(OI)(CI)(F)",
                "/grant",
                &format!("*{CONTAINER_USER_SID}:(OI)(CI)(M)"),
                "/grant",
                &format!("*{CONTAINER_ADMINISTRATOR_SID}:(OI)(CI)(M)"),
            ],
        )?;
        observe(first_line(&out));

        let (_, listing) = sh("icacls", &[&path])?;
        for line in listing.lines() {
            observe(line.trim());
        }
        let persisted =
            listing.contains(CONTAINER_USER_SID) || listing.contains(CONTAINER_ADMINISTRATOR_SID);
        record("container-SID ACE persisted", persisted);
        Ok(persisted)
    }

    /// Run a command inside a throwaway container with `dir` mounted, and
    /// report whether it succeeded.
    fn run_in_container(
        id: &str,
        image: &str,
        dir: &Path,
        argv: &[&str],
    ) -> std::io::Result<(bool, String)> {
        let mount = format!(
            "type=bind,src={},dst={},options=rbind:rw",
            dir.display(),
            GUEST_MOUNT
        );
        let mut args = vec!["run", "--rm", "--mount", &mount, image, id];
        args.extend_from_slice(argv);
        let result = sh("ctr", &args);
        // Best-effort cleanup: --rm should handle it, but a failed start can
        // leave the container record behind and block the next run.
        let _ = sh("ctr", &["container", "rm", id]);
        result
    }

    /// Experiments 3 and 4: access under the container-SID-only DACL, and the
    /// negative control. Without the control, a success in (3) could just mean
    /// the DACL never took effect.
    fn q1_access(image: &str, granted: &Path, denied: &Path) -> std::io::Result<()> {
        step(3, "Q1 access: read and write the mounted directory");
        let (ok, out) = run_in_container(
            CONTAINER_A,
            image,
            granted,
            &[
                "cmd",
                "/c",
                "echo spike > C:\\mnt\\probe.txt && type C:\\mnt\\probe.txt",
            ],
        )?;
        record("container could write the granted directory", ok);
        observe(first_line(&out));
        if ok {
            observe(
                "the host honoured an ACE for a principal it cannot name: \
                 volume ACLs can be scoped to container processes",
            );
        } else {
            observe(
                "the ACE was not honoured: Authenticated Users is the floor, \
                 and wcr[volume.host-exposure] stands as written",
            );
        }

        step(
            4,
            "Q1 confine: negative control, Administrators-only directory",
        );
        let path = denied.to_string_lossy().into_owned();
        let (_, out) = sh(
            "icacls",
            &[
                &path,
                "/inheritance:r",
                "/grant",
                "*S-1-5-32-544:(OI)(CI)(F)",
            ],
        )?;
        observe(first_line(&out));
        let (ok, out) = run_in_container(
            CONTAINER_B,
            image,
            denied,
            &["cmd", "/c", "echo spike > C:\\mnt\\probe.txt"],
        )?;
        record(
            "container could write the Administrators-only directory",
            ok,
        );
        observe(first_line(&out));
        if ok {
            observe(
                "CONTROL FAILED: the container wrote a directory it was not granted, \
                 so experiment 3 proves nothing — investigate before reading either result",
            );
        }
        Ok(())
    }

    /// Experiment 5: two containers, same directory. Records the absence of
    /// per-instance granularity as an observation rather than an assumption —
    /// this is the evidence behind wcr[volume.boundary].
    fn q1_discriminate(image: &str, dir: &Path) -> std::io::Result<()> {
        step(5, "Q1 discriminate: two containers against one directory");
        let probe = &["cmd", "/c", "echo second > C:\\mnt\\probe2.txt"];
        let (a, _) = run_in_container(CONTAINER_A, image, dir, probe)?;
        let (b, _) = run_in_container(CONTAINER_B, image, dir, probe)?;
        record("first container access", a);
        record("second container access", b);
        if a == b {
            observe(
                "both containers see the same permissions, as expected: host ACLs \
                 cannot separate instances (wcr[volume.boundary])",
            );
        } else {
            observe("UNEXPECTED: the two containers differ; investigate before relying on either");
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Q2: what can a second process do to a file the writer holds open?
    // -----------------------------------------------------------------------

    /// Open a file for append with an explicit `dwShareMode`, the way a log
    /// writer holding its active segment would.
    fn open_writer(path: &Path, share: u32) -> std::io::Result<File> {
        OpenOptions::new()
            .create(true)
            .append(true)
            .share_mode(share)
            .open(path)
    }

    /// Try to rename and delete `path` the way a rotator in another process
    /// would. `std::fs` opens its own handle, so the sharing check applies as
    /// it would cross-process.
    fn try_external_ops(path: &Path, renamed: &Path) -> (bool, bool) {
        let rename_ok = fs::rename(path, renamed).is_ok();
        // Delete whichever name currently exists.
        let target = if rename_ok { renamed } else { path };
        let delete_ok = fs::remove_file(target).is_ok();
        (rename_ok, delete_ok)
    }

    fn q2_sharing(dir: &Path) -> std::io::Result<()> {
        step(
            6,
            "Q2 no sharing: rename and delete with FILE_SHARE_DELETE absent",
        );
        let seg = dir.join("0001.jsonl");
        let renamed = dir.join("0001.renamed.jsonl");
        let mut w = open_writer(&seg, FILE_SHARE_READ | FILE_SHARE_WRITE)?;
        writeln!(w, "{{\"seq\":1}}")?;
        w.flush()?;
        let (rename_ok, delete_ok) = try_external_ops(&seg, &renamed);
        record("rename succeeded", rename_ok);
        record("delete succeeded", delete_ok);
        if rename_ok || delete_ok {
            observe(
                "UNEXPECTED: an active segment was renamed or removed without delete sharing; \
                 wlog[retain.rollover]'s premise needs revisiting",
            );
        } else {
            observe("both refused, as wlog[retain.rollover] assumes");
        }
        drop(w);
        let _ = fs::remove_file(&seg);
        let _ = fs::remove_file(&renamed);

        step(
            7,
            "Q2 with sharing: the same operations with FILE_SHARE_DELETE set",
        );
        let seg = dir.join("0002.jsonl");
        let renamed = dir.join("0002.renamed.jsonl");
        let mut w = open_writer(&seg, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)?;
        writeln!(w, "{{\"seq\":1}}")?;
        w.flush()?;
        let (rename_ok, delete_ok) = try_external_ops(&seg, &renamed);
        record("rename succeeded", rename_ok);
        record("delete succeeded", delete_ok);

        step(
            8,
            "Q2 after the operation: where do the writer's appends go?",
        );
        let append_ok = writeln!(w, "{{\"seq\":2}}").is_ok() && w.flush().is_ok();
        record("append after rename/delete succeeded", append_ok);
        let name_reusable = open_writer(&seg, FILE_SHARE_READ | FILE_SHARE_WRITE).is_ok();
        record(
            "original name reusable while the handle is open",
            name_reusable,
        );
        if rename_ok {
            let followed = fs::read_to_string(&renamed)
                .map(|s| s.contains("\"seq\":2"))
                .unwrap_or(false);
            record("appends followed the file to its new name", followed);
            observe(if followed {
                "rename-then-reopen is expressible: seedlingd could own rollover too"
            } else {
                "appends did not follow the rename; rollover stays with the writer"
            });
        }
        drop(w);
        let _ = fs::remove_file(&seg);
        let _ = fs::remove_file(&renamed);

        step(9, "Q2 reader interference: follow while the writer appends");
        let seg = dir.join("0003.jsonl");
        let mut w = open_writer(&seg, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)?;
        writeln!(w, "{{\"seq\":1}}")?;
        w.flush()?;
        let mut reader = File::open(&seg)?;
        let mut buf = String::new();
        reader.read_to_string(&mut buf)?;
        record("reader opened and read the active segment", !buf.is_empty());
        let append_ok = writeln!(w, "{{\"seq\":2}}").is_ok() && w.flush().is_ok();
        record("append succeeded while a reader held the file", append_ok);
        if !append_ok {
            observe(
                "a follow reader blocked an append; wlog[read.follow] needs revisiting \
                 before anything is built on it",
            );
        }
        buf.clear();
        reader.read_to_string(&mut buf)?;
        record("reader saw the later append", buf.contains("\"seq\":2"));
        drop(w);
        let _ = fs::remove_file(&seg);
        Ok(())
    }

    pub(super) fn run() -> Outcome {
        let image = std::env::args()
            .nth(1)
            .unwrap_or_else(|| DEFAULT_IMAGE.into());
        record("image", &image);

        let granted = scratch_dir("seedling-spike-granted")?;
        let denied = scratch_dir("seedling-spike-denied")?;
        let segments = scratch_dir("seedling-spike-segments")?;
        record("granted dir", granted.display());
        record("denied dir", denied.display());
        record("segment dir", segments.display());

        q1_resolve(&granted)?;
        let persisted = q1_apply(&granted)?;
        if persisted {
            q1_access(&image, &granted, &denied)?;
            q1_discriminate(&image, &granted)?;
        } else {
            observe(
                "the container-SID ACE did not persist, so the access experiments \
                 would be measuring nothing — Authenticated Users is the floor",
            );
        }

        q2_sharing(&segments)?;

        let _ = fs::remove_dir_all(&granted);
        let _ = fs::remove_dir_all(&denied);
        let _ = fs::remove_dir_all(&segments);
        Ok(())
    }

    /// The experiments that need no container, so they can run on a hosted
    /// Windows CI runner rather than waiting for a real host. Containers are
    /// not available on hosted runners at all, so everything in Q1 from
    /// experiment 3 on stays in the binary.
    ///
    /// These assert only the properties the design actually rests on, and
    /// merely record the rest: a spike's job is to discover, but once a
    /// discovery is load-bearing it is worth a test that fails loudly when a
    /// future Windows release changes it.
    #[cfg(test)]
    mod tests {
        use super::*;

        fn test_dir(name: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!("seedling-spike-test-{name}"));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create test dir");
            dir
        }

        /// wlog[retain.rollover] puts segment rollover in the log writer
        /// because another process cannot take an actively written segment
        /// away from it. If this ever passes, seedlingd could own rollover
        /// too and the writer gets simpler.
        #[test]
        fn active_segment_resists_external_rename_and_delete() {
            let dir = test_dir("no-share-delete");
            let seg = dir.join("0001.jsonl");
            let renamed = dir.join("0001.renamed.jsonl");

            let mut w = open_writer(&seg, FILE_SHARE_READ | FILE_SHARE_WRITE).expect("open writer");
            writeln!(w, "{{\"seq\":1}}").expect("append");
            w.flush().expect("flush");

            let (rename_ok, delete_ok) = try_external_ops(&seg, &renamed);
            assert!(
                !rename_ok && !delete_ok,
                "an active segment was renamed ({rename_ok}) or deleted ({delete_ok}) by another \
                 opener without FILE_SHARE_DELETE — wlog[retain.rollover] assumes neither is \
                 possible, so the rotation split can be revisited"
            );

            drop(w);
            let _ = fs::remove_dir_all(&dir);
        }

        /// wlog[read.follow] requires that following a segment never blocks
        /// the producer appending to it.
        #[test]
        fn follow_reader_does_not_block_appends() {
            let dir = test_dir("follow");
            let seg = dir.join("0003.jsonl");

            let mut w = open_writer(&seg, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .expect("open writer");
            writeln!(w, "{{\"seq\":1}}").expect("first append");
            w.flush().expect("flush");

            let mut reader = File::open(&seg).expect("a reader must be able to open the segment");
            let mut buf = String::new();
            reader.read_to_string(&mut buf).expect("read");
            assert!(buf.contains("\"seq\":1"), "reader saw no records");

            writeln!(w, "{{\"seq\":2}}").expect(
                "appending while a reader holds the segment failed — wlog[read.follow] needs \
                 revisiting before the log store is built on it",
            );
            w.flush().expect("flush after read");

            buf.clear();
            reader.read_to_string(&mut buf).expect("read again");
            assert!(
                buf.contains("\"seq\":2"),
                "a follow reader did not see a record appended after it opened the segment"
            );

            drop(w);
            let _ = fs::remove_dir_all(&dir);
        }

        /// wcr[identity.workload] states that a container's account is not a
        /// host principal. If the name ever resolves, the reasoning behind
        /// wcr[volume.boundary] and wcr[volume.host-exposure] changes.
        #[test]
        fn container_account_name_does_not_resolve_on_the_host() {
            let dir = test_dir("sid-resolve");
            let path = dir.to_string_lossy().into_owned();

            let (by_name, out) =
                sh("icacls", &[&path, "/grant", "ContainerUser:(R)"]).expect("run icacls");
            assert!(
                !by_name,
                "the host resolved the container account by name ({}) — wcr[identity.workload] \
                 says it cannot, and the volume boundary rules depend on that",
                first_line(&out)
            );

            let _ = fs::remove_dir_all(&dir);
        }

        /// Whether a literal container SID is accepted in a host ACL is the
        /// open half of Q1. It is recorded rather than asserted: the answer
        /// only matters together with the container-side access test, which
        /// cannot run on a hosted runner.
        #[test]
        fn literal_container_sid_in_a_host_acl_is_recorded() {
            let dir = test_dir("sid-literal");
            let path = dir.to_string_lossy().into_owned();

            let grant = format!("*{CONTAINER_USER_SID}:(OI)(CI)(M)");
            let (accepted, out) = sh("icacls", &[&path, "/grant", &grant]).expect("run icacls");
            record("literal container SID accepted by icacls", accepted);
            observe(first_line(&out));

            if accepted {
                let (_, listing) = sh("icacls", &[&path]).expect("read back");
                record("ACE persisted", listing.contains(CONTAINER_USER_SID));
            }

            let _ = fs::remove_dir_all(&dir);
        }
    }
}
