# Ports that are written but not wired yet

The files here are integration test files for plugin ports that are written but
not registered in the plugin table, so their members do not exist yet and the
tests cannot pass. Cargo does not build a test target from a subdirectory, so
they stay here, out of the build, until the port is wired.

`layer_ex_btoa.rs` - the port is in
`crates/compat/src/natives/plugins/layer_ex_btoa/`; its writer stopped before
the tests were finished, and the port does not compile, so the module is not
declared either. The specification is in
`docs/code/compat/upstream/layerExBTOA.md`.
