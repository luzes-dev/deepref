# Rust uncovered complexity

`just risk-rust` consumes `coverage/rust/lcov.info`, or accepts a different LCOV
path. It never runs coverage or tests. cargo-crap is pinned to 0.4.3; its canonical
maintained upstream is [minikin/cargo-crap](https://github.com/minikin/cargo-crap).

After the separate Rust coverage workflow lands, a successful development
coverage run triggers this report. The consumer downloads the existing LCOV,
checks its source revision, and checks out that exact source before analysis.
The workflow has read-only repository and Actions access. The release binary is
pinned to its published SHA-256. Reports are retained for 30 days.

The score combines uncovered code and cyclomatic complexity. This initial
report has no `--fail-above` or `--fail-regression` threshold: missing coverage,
broken tooling, and invalid inputs still fail normally, while legacy risk does
not become a new blocking PR check. Establish a baseline before choosing a
changed-function ratchet. Generated sources, test targets, benchmarks, and
repository tooling are excluded to match the coverage producer's scope.

The optional cargo-crap duplication analysis is deliberately disabled. Rust-only
jscpd owns duplication; this command owns only uncovered complexity. It also
does not replace mutation testing, which evaluates assertion strength.
