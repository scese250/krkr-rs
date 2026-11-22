# Gates — fixture

## 1. The ordered static gate

The fixture's registry runs four steps; the table below states them.

| # | Step | Kind | What it protects |
|---|---|---|---|
| 1 | `cargo fmt --all -- --check` | cargo | formatting |
| 2 | `stateless` | check | no process-global state |
| 3 | `error-handling` | check | no silent fallbacks |
| 4 | `native-parity --no-doc` | check | the native class surface vs the reference |

## 2. The test suites

The fixture stops here.
