# Round 2 code divergences (for D6)

## Web
- crates/web/src/http.rs content_security_policy: `connect-src 'self' https://*:<port>` for an IPv6-literal Host.
- crates/web/src/http.rs connect / auth.rs handle_connect: 400 for a malformed body or both token and password, before any credential source.
- crates/web/src/main.rs: --wt-port conflicts with --wt-listen, and with --listen only when neither --wt-interface nor --wt-listen is given; wt_bind_port always args.wt_port.
- crates/web/src/event_broker.rs: reset back-off to 1 s once a subscription is established.
- crates/web/src/shell.rs exit_relay: relay the exit frame only when complete.
- crates/web/src/wt.rs: answer /forwards/start with requirements_invalid (direct connection needed).
- crates/web/src/proxy.rs peek_request and wt.rs: oversized, non-JSON or non-object first line answered requirements_invalid.
- crates/web/src/auth/tailscale.rs extract_actor: visible ASCII is non-empty bytes 0x21–0x7E.
- crates/web/src/web_sessions.rs: web entries' actor_* always strings.
