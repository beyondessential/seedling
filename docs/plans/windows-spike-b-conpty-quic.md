# Spike B: ConPTY over QUIC

Budget: an afternoon. Environment: any Windows Server 2019+ box plus a
machine running the existing web UI; no field image needed.

## At stake

- `win[shell.conpty]` — carries the `[spike]` tag: stream mapping, merged
  output, resize, and the empty-stderr contract all need confirming against
  a real client.
- `i[stream.shell]` — whether the three-stream shape survives unmodified
  when stderr is structurally empty.
- Side task: whether the Linux daemon already effectively merges stderr for
  TTY-attached shells, which decides if the empty-stderr note in
  `win[shell.conpty]` is a divergence or a description of existing
  behaviour.

## Setup

A minimal ConPTY host: `CreatePseudoConsole`, spawn `cmd`, PowerShell, and
a Node REPL under it, wire the ConPTY input/output pipes. Bridge it over
QUIC using the existing interface crates if convenient, or a bare quinn
harness shaped like `i[stream.shell]`: session stream for stdin and the
exit frame, one unidirectional stream for stdout, one opened-but-silent
unidirectional stream for stderr.

## Experiments

1. **Interactive session.** Drive each shell through the bridge from the
   web terminal. Confirm VT sequences render, line editing works, and the
   merged stdout/stderr stream reads acceptably (run a command that writes
   to both and check nothing interleaves into garbage).
2. **Empty stderr.** Confirm the web terminal and CLI neither block on nor
   misbehave over a stderr stream that never carries bytes, and that
   closing it early versus holding it open makes no client-visible
   difference. Pick one behaviour and record it.
3. **Resize.** Propagate terminal resizes to `ResizePseudoConsole` mid
   session, including during a full-screen program; confirm reflow.
4. **Exit codes.** Normal exit, and session teardown via Job termination:
   confirm the exit frame carries the process's own code in the first case
   and the negative synthesised code (`win[signal.exit-codes]`) in the
   second.
5. **Linux stderr side task.** On the Linux runtime, open a TTY-attached
   shell, write to fd 2 inside the container, and observe which stream
   delivers it. Record the answer here and adjust the wording of the
   `win[shell.conpty]` note accordingly.
6. **Protocol notes for Q2.** Note anything the bridge forces onto the
   supervisor pipe protocol (framing, backpressure, resize commands) as
   input to pinning Q2 — do not design the protocol here.

## Exit criteria

- A shell session driven end-to-end from the real web terminal with
  resize, clean exit, and killed exit: remove the `[spike]` tag from
  `win[shell.conpty]`.
- The empty-stderr contract confirmed against both clients and stated
  without hedging in the spec.
- The Linux merge question answered and the divergence note resolved
  either way.

## If it fails

- Merged output renders poorly in the web terminal: fix the client, not
  the runtime — ConPTY does not offer split streams; the alternative is
  non-TTY piping, which changes shell semantics and is not on the table.
- Resize glitches: acceptable to note as platform behaviour if cosmetic;
  a broken reflow that loses buffer content needs client-side handling.
