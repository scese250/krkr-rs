# Code styles — fixture

## 10. What the gate enforces

The fixture's registry runs one cargo step and three checks; the table below
states them.

| check | the rule it protects |
|---|---|
| `cargo fmt --all -- --check` | formatting |
| `stateless` | no process-global state |
| `error-handling` | no silent fallbacks |
| `native-parity` | the native class surface vs the reference |

## 11. Definition of done

The fixture stops here.
