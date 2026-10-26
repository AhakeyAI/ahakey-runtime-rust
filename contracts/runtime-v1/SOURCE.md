Vendored from AhaKey Studio dev commit 5eb399c. These are the phase-one frontend wire fixtures and schema. See docs/rust-backend-integration.md in Studio.

The `ConfigurationPatch` definition and `fixtures/patch.json` are additive Runtime-owned extensions introduced for the Tauri Studio integration. Their validator and serializer are `src/configuration/patch.rs`; older vendored phase-one definitions retain their original provenance.

`ConfigurationRead`, the optional `ConfigurationDocument.read`, and `configuration-reading.json` describe the Runtime-owned asynchronous `configuration.read` extension. The task state is volatile; its implementation is `src/runtime/readback.rs`.

`ConfigurationProgress` and `configuration-paused.json` are Runtime-owned configuration transaction extensions. The durable checkpoint and field grouping are implemented in `src/configuration/transaction.rs` and `src/runtime/configuration_transaction.rs`.
