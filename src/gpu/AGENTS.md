# GPU task context

Root rules and [GPU_CONTRACT](../../docs/GPU_CONTRACT.md) remain mandatory.
Select the process in [repo map](../../docs/agent/repo-map.md) before reading modules.
Start at its host/kernel pair and focused test; inspect `mod.rs` only for shared
runtime/export changes, `buffers.rs` for layout/transfer changes, and the simulation
call site for composition changes. An isolated dispatch test cannot prove a
production handoff. Use the [test map](../../docs/agent/test-map.md) for evidence.
