# Quantity parameter kind

## Parsing

- [x] `std::parse_quantity` gives `1Ki` as 1024, `1k` as 1000, and every other decimal and binary suffix its power (verifies spec: ACN)
- [x] `1e3`, `1E+3` and `15000e-3` give 1000, 1000 and 15; `2E` is 2×10^18 and `2E2` is 200 (verifies spec: ACN)
- [x] `.5Gi`, `5.Gi`, `-512Mi` and `+1k` parse (verifies spec: ACN)
- [x] `1.1Ki`, `1e-3` and `0.5` are refused as not whole; `0.5Ki` gives 512 (verifies spec: ACN)
- [x] `8Ei`, `10E` and `9223372036854775808` are refused as out of range; `-8Ei` is the smallest signed 64-bit integer (verifies spec: ACN)
- [x] A space anywhere, `1kk`, `1u` and `1e3Ki` are refused as not quantities (verifies spec: ACN)
- [x] `512m` is refused with "m is milli; did you mean 512Mi?", `512MiB` suggests `512Mi`, `1MB` suggests `1M` (verifies spec: ACN)

## The kind

- [x] `kind("quantity")` and `kind(#{ name: "quantity", range: 1..=1024 })` are accepted; an empty range or an unknown argument throws (verifies spec: ACN)
- [x] A value outside the range is refused with the bounds as quantities, such as "must be between 64Mi and 15.5Gi" (verifies spec: ACN)
- [x] `param.parsed()` and an action's param map give the amount as an integer (verifies spec: PARAM, ACN)
- [x] An integer `default_value` is stored as its decimal form; a default the kind refuses fails the evaluation (verifies spec: PARAM)
- [x] `/apps/params/set` and an action invocation take a JSON integer for a quantity param, checked against its range (verifies spec: PRM, ACI)
- [x] `/apps/show` reports a quantity param's kind, range and default (verifies spec: APPS)

## Container memory

- [x] `container.memory("512Mi")`, `memory("1024")`, `memory(536870912)` and `memory(AVAILABLE_MEMORY / 4)` set the limit in bytes (verifies spec: CTR)
- [x] `memory("512m")` throws with the suffix hint; `memory(0)`, `memory("-1Ki")` and `memory(1.5)` throw (verifies spec: CTR)
- [x] The container starts with `--memory=<bytes>`, and `/apps/show` reports the limit as an integer (verifies spec: APPS)

## Console

- [x] A quantity param shows `= 1,610,612,736` beside `1.5Gi` as it is typed, with the description on the helper line (verifies spec: ACN)
- [x] An out-of-range or malformed quantity shows the kind's error and holds back submitting or staging (verifies spec: ACN)
- [x] A container's memory limit shows as `mem: 512Mi`
- [ ] In a browser, edit a quantity param on the app detail page and check the resolved amount sits inside the field before the edit actions, as in the mockup
