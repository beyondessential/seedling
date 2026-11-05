# Remove the backup-app framework

Removes backup apps, strategies, scheduling/execution, `backup_failed` and friends. Operation-scoped volume bindings and the reserved `_volume`/`_filename` param keys stay (still used by `kind: volume` action params and shells). The Canopy `backup-prompt` report text and `/backup-target` transport note are Canopy's, not ours, and stay.

## Steps

- [x] Specs: drop backup sections from `runtime.md`, `interface.md`, `web.md`; reword operation volume params and `namespace.reserved` (no more `backup-snap-`)
- [x] Migration v59 drops `backup_apps` and `backup_strategies`; bump db test version
- [x] Core: delete `backup_apps`, `backup_strategies`, `backup_execution`, `oi/handler/backups*`, `run_operation_for_backup`, apps.rs validation, handler routes, `BackupAppInUse`, `backup_actions`, `BackupStrategyName`
- [x] Daemon: remove backup ticker and startup snapshot sweep; remove `backup-snap-` reservation
- [x] ctl: delete `backups` command and the apps-create hint
- [x] Web: delete Backups route, navbar entry, types, tests, e2e entry
- [x] Docs: delete `backup-app.md` and `apps/kopia-s3.seed.rhai`; scrub references in guides/plans/threat model/failure modes
- [x] clippy, fmt, tracey, tests, frontend tests/typecheck
