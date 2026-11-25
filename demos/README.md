# Demos

Each folder here is a definition bundle that shows and exercises one area of Seedling.
They are demos, not production definitions: none of them deploys a real service, and none is meant as a starting point for one.

Every demo installs with no parameters set, except params-tour, which needs `contact`.
Hostnames default to `<demo>.demo.internal`, served with a certificate from the proxy's internal CA.
[Bringing up a demo app](../docs/dev-vm.md#bringing-up-a-demo-app) walks through one on a dev VM, and each demo's header comment lists what to try.

| demo | shows |
| --- | --- |
| [hello-world](hello-world/) | the smallest app: an echo server behind one HTTPS ingress |
| [http-tour](http-tour/) | every HTTP proxy setting, one route each: redirects, headers, compression, rate limits, balancing |
| [protocol-tour](protocol-tour/) | one ingress per termination, shared ports, the HTTPS redirect, and a target for a site ingress |
| [actions-and-gates](actions-and-gates/) | gates keeping a store's format current, and actions and shells using every method of the runtime handle |
| [paired-provider](paired-provider/) | an app exporting an HTTP service, a TCP service and a volume |
| [paired-consumer](paired-consumer/) | an app using paired-provider's exports through external services, an external volume and service mounts |
| [params-tour](params-tour/) | one parameter of every kind, with defaults, a required parameter, secrets and validators |
| [workloads-tour](workloads-tour/) | deployments, jobs, containers, pods and volumes, every feature of each |
| [language-tour](language-tour/) | the standard helpers the other demos do not use, and what they return |
| [bundle-tour](bundle-tour/) | a bundle's metadata, script files, sidecar files and includes |
| [bundle-share](bundle-share/) | a second bundle including the same files as bundle-tour |

[shared](shared/) is not a demo: it holds the helper script and pages that bundle-tour and bundle-share both include.
