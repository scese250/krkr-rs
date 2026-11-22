# Code styles — fixture

## 10. What the gate enforces

The fixture's registry runs one cargo step and three checks; the table below
drifts from it in both ways the check can see: `stateless` is not named, and
`host-facts` names a check the registry does not run.

| check | the rule it protects |
|---|---|
| `cargo fmt --all -- --check` | formatting |
| `error-handling` | no silent fallbacks |
| `native-parity` | the native class surface vs the reference |
| `host-facts` | a check the registry does not run |

## 11. Definition of done

The fixture stops here.
