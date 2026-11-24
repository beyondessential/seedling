# Redacting sensitive values: test cases

## Describing an app

- [x] Set a secret param, declare a volume write and a container env, command, arg and healthcheck command that hold its value, then describe the app: each of those strings reads `null`, and its neighbours read as declared (verifies spec: APPS)
- [x] Declare a volume write holding a value from `std::random_string`, then describe the app: the write reads `null` (verifies spec: APPS)
- [x] Set a plain param holding the value of a secret param that is set later. Before the secret is set, describe shows the plain value. Afterwards it reads `null` with `is_set` true (verifies spec: APPS)

## History and events

- [x] After the change above, list the generation history: every entry for the plain param is `redacted`, including the one recorded before the secret was set (verifies spec: SEC)
- [x] The `ParamsChanged` event for that change carries the plain param as `redacted`, and no event carries the secret (verifies spec: EVT)

## Action log and breadcrumbs

- [x] Invoke a sub-action with a plain param holding an operation-derived value: the log entry does not hold it in the clear, and a replay recovers it (verifies spec: HIST)
- [x] An `rt.exec` breadcrumb whose arguments hold a secret param and a derived password shows each as `<redacted>` and the rest as written (verifies spec: ACTU)
- [x] A sub-action breadcrumb masks a param value whose characters JSON would escape (verifies spec: ACTU)
- [ ] Run an action that calls `rt.exec` with a secret param on a real host, then read `apps logs`: the breadcrumb shows `<redacted>` and the container's own output is unchanged (verifies spec: SEC)

## Free text

- [x] A validator that throws a message quoting the proposed secret value: the rejection reads `<redacted>` in its place (verifies spec: SEC)
- [x] An action that throws a message quoting a value it derived: the failure reads `<redacted>` in its place (verifies spec: SEC)
- [x] A shell session whose closure throws a message quoting a secret shell param: the error the client receives reads `<redacted>` in its place (verifies spec: SEC)
- [x] Set a weak-password param to a single character: every described string holding that character reads `null`, and the rest read as declared (verifies spec: SEC)

## Whole responses and events

- [x] Change a secret after a plain param quoted it: the plain param and its history stay withheld (verifies spec: SEC)
- [x] A fault quoting a value that later became secret is masked in `/apps/show` and `/faults/list` (verifies spec: SEC)
- [x] An event quoting an app's secret is masked as the event stream sends it, while its `app` and other identifiers are left as they are (verifies spec: SEC)
- [x] An error message quoting the requested app's secret is masked (verifies spec: SEC)
- [x] Retrieving an app's script returns a secret default written in it as stored (verifies spec: SEC)
