# Q6 test cases

## Automated

- [x] Each bundle under `demos/` loads as a bundle and evaluates with no params set (params-tour: with only its required param), with no error.
- [x] bundle-tour and bundle-share are assembled through the same include assembly the CLI uses, and both carry the files from `demos/shared/`.
- [x] ctl's `apps create` from a demo folder still assembles includes after the assembly moves.
- [x] actions-and-gates: a gate exclusive with a deployment lists that deployment.
- [x] actions-and-gates: a gate with a check job and an interval reports both.
- [x] actions-and-gates: `after` ordering between gates, and deployments ordered after gates, match the definition.
- [x] actions-and-gates: changing a param a gate converges on moves that gate's key; changing an unrelated param leaves it.
- [x] actions-and-gates: a gate body takes its first-run branch under a probe with no last success, and its later-run branch with one.
- [x] actions-and-gates: a refused transition (the `format` param stepping down from its last success) fails the gate body.
- [x] actions-and-gates: every action body runs under a probe.
- [x] Nothing in the repo still names `apps/`, `postgres.seed.rhai` or `tamanu-*.seed.rhai`.

## On a dev VM

- [ ] hello-world installs from its folder with no params set and answers on `hello-world.demo.internal`.
- [ ] http-tour: each route shows its feature in the echoed request or response (redirect, header operations, compression, rate limit, balancing across replicas, HTTP to HTTPS redirect).
- [ ] protocol-tour: plain TCP, TLS to TCP, DTLS to UDP, HTTPS to HTTP/1.1 and HTTPS to h2c each echo through their ingress.
- [ ] protocol-tour: a site ingress attaches to its attachable ingress and carries traffic.
- [ ] protocol-tour installed twice under different app names with the same hostname: the second gets an `ingress_conflict` fault.
- [ ] actions-and-gates: the `docs/dev-vm.md` walkthrough runs as written.
- [ ] actions-and-gates: the scheduled action runs on its schedule, and the shell opens.
- [ ] paired apps: the consumer reaches the provider's exported service and reads its exported volume; the external service routes once mapped.
- [ ] params-tour: each kind shows its operator input in the web console, and a validator refuses a bad value.
- [ ] workloads-tour: installs, a failing health check triggers its failure response, and the job runs to its deadline.
- [ ] bundle-tour and bundle-share: each serves the shared static page.
- [ ] language-tour installs and its output shows each helper's result.
