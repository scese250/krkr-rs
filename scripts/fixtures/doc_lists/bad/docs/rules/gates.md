# Gates — fixture

## 1. The ordered static gate

The fixture's registry runs four steps; the table below drifts from it in
every way the check can see: `stateless` is missing, `native-parity` and
`error-handling` are in the wrong order, the cargo step is called a check,
and `doc-lists` names a step the registry does not run.

| # | Step | Kind | What it protects |
|---|---|---|---|
| 1 | `cargo fmt --all -- --check` | check | formatting |
| 2 | `native-parity --no-doc` | check | the native class surface vs the reference |
| 3 | `error-handling` | check | no silent fallbacks |
| 4 | `doc-lists` | check | a step the registry does not run |

## 2. The test suites

The fixture stops here.
