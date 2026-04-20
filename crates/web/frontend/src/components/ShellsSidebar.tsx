import CloseIcon from "@mui/icons-material/Close";
import { Box, Divider, IconButton, Paper, Tab, Tabs, Typography } from "@mui/material";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useCallback, useEffect, useRef, useState } from "react";
import type { ShellTab } from "./SessionProvider";
import { useSessionContext } from "./SessionProvider";

const MIN_WIDTH = 300;
const MAX_WIDTH = 1400;

// w[shells.ui]
function ShellInstance({ tab, active }: { tab: ShellTab; active: boolean }) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const sessionIdRef = useRef<string | null>(null);
  const { session, uniRouter, closeShell } = useSessionContext();
  const [exitCode, setExitCode] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Refit when becoming active so xterm knows the correct size.
  useEffect(() => {
    if (active && fitAddonRef.current) {
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          fitAddonRef.current?.fit();
        });
      });
    }
  }, [active]);

  useEffect(() => {
    if (!session || !uniRouter || !containerRef.current) return;

    const term = new Terminal({
      theme: { background: "#1e1e1e" },
      fontFamily: "monospace",
      fontSize: 13,
      scrollback: 5000,
      cursorBlink: true,
    });
    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(containerRef.current);
    termRef.current = term;
    fitAddonRef.current = fitAddon;

    if (active) fitAddon.fit();

    let closed = false;

    let resizeInFlight = false;
    let resizePending = false;
    const sendResize = (rows: number, cols: number) => {
      if (!sessionIdRef.current) return;
      if (resizeInFlight) { resizePending = true; return; }
      resizeInFlight = true;
      session.client.request("/shells/resize", { session_id: sessionIdRef.current, rows, cols })
        .catch(() => undefined)
        .finally(() => {
          resizeInFlight = false;
          if (resizePending) {
            resizePending = false;
            fitAddon.fit();
          }
        });
    };

    const ro = new ResizeObserver(() => {
      if (active) {
        fitAddon.fit();
        sendResize(term.rows, term.cols);
      }
    });
    ro.observe(containerRef.current);

    term.onResize(({ rows, cols }) => sendResize(rows, cols));

    const enc = new TextEncoder();

    const openPromise = tab.kind === "volume"
      ? session.client.openVolumeShell(
          { volumes: tab.volumes, rows: term.rows, cols: term.cols },
          uniRouter,
        )
      : session.client.openShell(
          { app: tab.app, name: tab.shellName, rows: term.rows, cols: term.cols, params: tab.params },
          uniRouter,
        );

    openPromise
      .then(({ sessionId: sid, writer, exitCode: exitPromise, stdout, stderr }) => {
        if (closed) { void writer.close().catch(() => undefined); return; }
        sessionIdRef.current = sid;

        term.onData((str) => {
          void writer.write(enc.encode(str)).catch(() => undefined);
        });

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
      termRef.current = null;
      fitAddonRef.current = null;
      if (sessionIdRef.current) {
        void session.client.request("/shells/stop", { session_id: sessionIdRef.current }).catch(() => undefined);
        sessionIdRef.current = null;
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session, uniRouter]);

  const handleReopen = () => {
    setExitCode(null);
    setError(null);
  };

  return (
    <Box sx={{ position: "absolute", inset: 0, display: "flex", flexDirection: "column" }}>
      <Box
        ref={containerRef}
        sx={{
          flexGrow: 1,
          overflow: "hidden",
          "& .xterm": { height: "100%" },
          "& .xterm-viewport": { overflowY: "hidden" },
        }}
      />
      {(exitCode !== null || error !== null) && (
        <Box
          sx={{
            position: "absolute", inset: 0, display: "flex", alignItems: "center",
            justifyContent: "center", bgcolor: "rgba(0,0,0,0.75)", flexDirection: "column", gap: 1.5,
          }}
        >
          <Typography sx={{ color: "#cccccc", fontSize: 14, fontFamily: "monospace" }}>
            {error !== null ? `Error: ${error}` : `Exited: ${exitCode}`}
          </Typography>
          <Box sx={{ display: "flex", gap: 1 }}>
            <Box
              component="button"
              onClick={handleReopen}
              sx={{
                px: 2, py: 0.5, cursor: "pointer", bgcolor: "transparent",
                border: "1px solid #666", color: "#ccc", borderRadius: 1, fontSize: 13,
                "&:hover": { borderColor: "#aaa" },
              }}
            >
              Reopen
            </Box>
            <Box
              component="button"
              onClick={() => closeShell(tab.id)}
              sx={{
                px: 2, py: 0.5, cursor: "pointer", bgcolor: "transparent",
                border: "1px solid #666", color: "#ccc", borderRadius: 1, fontSize: 13,
                "&:hover": { borderColor: "#aaa" },
              }}
            >
              Close tab
            </Box>
          </Box>
        </Box>
      )}
    </Box>
  );
}

// w[shells.ui]
export function ShellsSidebar() {
  const {
    shellTabs, activeShellId, setActiveShellId, closeShell,
    shellsSidebarWidth, setShellsSidebarWidth,
  } = useSessionContext();

  const dragging = useRef(false);
  const startX = useRef(0);
  const startWidth = useRef(0);

  const onMouseDown = useCallback((e: React.MouseEvent) => {
    dragging.current = true;
    startX.current = e.clientX;
    startWidth.current = shellsSidebarWidth;
    e.preventDefault();
  }, [shellsSidebarWidth]);

  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      if (!dragging.current) return;
      const delta = e.clientX - startX.current;
      const next = Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, startWidth.current + delta));
      setShellsSidebarWidth(next);
    };
    const onUp = () => { dragging.current = false; };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [setShellsSidebarWidth]);

  const activeIndex = shellTabs.findIndex((t) => t.id === activeShellId);

  return (
    <Paper
      variant="outlined"
      square
      sx={{
        width: shellsSidebarWidth,
        flexShrink: 0,
        display: "flex",
        flexDirection: "column",
        position: "relative",
        borderTop: "none",
        borderBottom: "none",
        borderLeft: "none",
        overflow: "hidden",
        bgcolor: "#1e1e1e",
      }}
    >
      {/* Drag handle on right edge */}
      <Box
        onMouseDown={onMouseDown}
        sx={{
          position: "absolute",
          right: 0,
          top: 0,
          bottom: 0,
          width: 4,
          cursor: "col-resize",
          zIndex: 10,
          "&:hover": { bgcolor: "primary.main", opacity: 0.4 },
        }}
      />

      {/* Tab bar */}
      <Box sx={{ display: "flex", alignItems: "center", bgcolor: "#252526", borderBottom: "1px solid #3c3c3c", minHeight: 36 }}>
        <Tabs
          value={activeIndex >= 0 ? activeIndex : false}
          onChange={(_, i: number) => setActiveShellId(shellTabs[i].id)}
          variant="scrollable"
          scrollButtons="auto"
          sx={{
            flexGrow: 1,
            minHeight: 36,
            "& .MuiTab-root": {
              minHeight: 36, py: 0, px: 1.5, fontSize: 12,
              fontFamily: "monospace", color: "#999",
              textTransform: "none", minWidth: 0,
            },
            "& .Mui-selected": { color: "#ccc" },
            "& .MuiTabs-indicator": { backgroundColor: "#007acc" },
          }}
        >
          {shellTabs.map((tab) => (
            <Tab
              key={tab.id}
              label={
                <Box sx={{ display: "flex", alignItems: "center", gap: 0.5 }}>
                  <span>{tab.kind === "volume" ? tab.label : `${tab.app}/${tab.shellName}`}</span>
                  <IconButton
                    size="small"
                    onClick={(e) => { e.stopPropagation(); closeShell(tab.id); }}
                    sx={{ p: 0, color: "inherit", "&:hover": { color: "#fff" } }}
                  >
                    <CloseIcon sx={{ fontSize: 12 }} />
                  </IconButton>
                </Box>
              }
              disableRipple
            />
          ))}
        </Tabs>
      </Box>

      <Divider sx={{ borderColor: "#3c3c3c" }} />

      {/* Terminal area */}
      <Box sx={{ flexGrow: 1, position: "relative", overflow: "hidden" }}>
        {shellTabs.map((tab) => {
          const isActive = tab.id === activeShellId;
          return (
            <Box
              key={tab.id}
              sx={{
                position: "absolute",
                inset: 0,
                display: isActive ? "block" : "none",
              }}
            >
              <ShellInstance tab={tab} active={isActive} />
            </Box>
          );
        })}
      </Box>
    </Paper>
  );
}
