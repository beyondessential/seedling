import { useEffect, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { Box, Button, Typography } from "@mui/material";
import { useSessionContext } from "../components/SessionProvider";

// w[shells.ui]
// w[shells.resize]
export default function Shell() {
  const { name, shellName } = useParams<{ name: string; shellName: string }>();
  const navigate = useNavigate();
  const { session, uniRouter } = useSessionContext();

  const containerRef = useRef<HTMLDivElement>(null);
  const [exitCode, setExitCode] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!session || !uniRouter || !containerRef.current || !name || !shellName) return;

    const term = new Terminal({
      theme: { background: "#1e1e1e" },
      fontFamily: "monospace",
      fontSize: 14,
      scrollback: 5000,
      cursorBlink: true,
    });
    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(containerRef.current);
    fitAddon.fit();

    let sessionId: string | null = null;
    let closed = false;

    // Debounced resize: coalesce resize events to at most one in-flight.
    let resizeInFlight = false;
    let resizePending = false;
    const sendResize = (rows: number, cols: number) => {
      if (!sessionId) return;
      if (resizeInFlight) { resizePending = true; return; }
      resizeInFlight = true;
      session.client.request("/shells/resize", { session_id: sessionId, rows, cols })
        .catch(() => undefined)
        .finally(() => {
          resizeInFlight = false;
          if (resizePending) {
            resizePending = false;
            fitAddon.fit();
          }
        });
    };

    // Observe container size changes.
    const ro = new ResizeObserver(() => {
      fitAddon.fit();
      sendResize(term.rows, term.cols);
    });
    ro.observe(containerRef.current);

    term.onResize(({ rows, cols }) => {
      sendResize(rows, cols);
    });

    const enc = new TextEncoder();

    // Open the shell session.
    session.client
      .openShell({ app: name, name: shellName, rows: term.rows, cols: term.cols }, uniRouter)
      .then(({ sessionId: sid, writer, exitCode: exitPromise, stdout, stderr }) => {
        if (closed) { void writer.close().catch(() => undefined); return; }
        sessionId = sid;

        // Wire up stdin: xterm data → daemon bidi send.
        term.onData((str) => {
          void writer.write(enc.encode(str)).catch(() => undefined);
        });

        // Pump stdout into the terminal.
        const pumpStream = async (stream: ReadableStream<Uint8Array>) => {
          const reader = stream.getReader();
          try {
            for (;;) {
              const { done, value } = await reader.read();
              if (done) break;
              term.write(value);
            }
          } catch { /* stream closed */ } finally {
            reader.releaseLock();
          }
        };
        void pumpStream(stdout);
        void pumpStream(stderr);

        // Await exit.
        exitPromise.then((code) => {
          if (!closed) setExitCode(code);
        });
      })
      .catch((e: unknown) => {
        if (!closed) setError(String(e));
      });

    return () => {
      closed = true;
      ro.disconnect();
      term.dispose();
      // Best-effort stop if we have a session ID and haven't exited cleanly.
      if (sessionId) {
        void session.client.request("/shells/stop", { session_id: sessionId }).catch(() => undefined);
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session, uniRouter, name, shellName]);

  return (
    <Box sx={{ display: "flex", flexDirection: "column", height: "100vh", bgcolor: "#1e1e1e" }}>
      {/* Header */}
      <Box sx={{ display: "flex", alignItems: "center", gap: 1, px: 2, py: 1, bgcolor: "#252526", borderBottom: "1px solid #3c3c3c" }}>
        <Typography sx={{ color: "#cccccc", fontSize: 14, fontFamily: "monospace", flexGrow: 1 }}>
          {name} / {shellName}
        </Typography>
        <Button
          size="small"
          onClick={() => navigate(`/apps/${name ?? ""}`)}
          sx={{ color: "#cccccc", textTransform: "none", minWidth: 0 }}
        >
          Close
        </Button>
      </Box>

      {/* Terminal container */}
      <Box
        ref={containerRef}
        sx={{ flexGrow: 1, overflow: "hidden", "& .xterm": { height: "100%" }, "& .xterm-viewport": { overflowY: "hidden" } }}
      />

      {/* Exit / error overlay */}
      {(exitCode !== null || error !== null) && (
        <Box
          sx={{
            position: "absolute", inset: 0, display: "flex", alignItems: "center",
            justifyContent: "center", bgcolor: "rgba(0,0,0,0.7)", flexDirection: "column", gap: 2,
          }}
        >
          <Typography sx={{ color: "#cccccc", fontSize: 16 }}>
            {error !== null ? `Error: ${error}` : `Shell exited with code ${exitCode}`}
          </Typography>
          <Box sx={{ display: "flex", gap: 2 }}>
            <Button variant="outlined" onClick={() => navigate(`/apps/${name ?? ""}`)}>
              Back
            </Button>
            <Button variant="contained" onClick={() => { setExitCode(null); setError(null); }}>
              Reopen
            </Button>
          </Box>
        </Box>
      )}
    </Box>
  );
}
