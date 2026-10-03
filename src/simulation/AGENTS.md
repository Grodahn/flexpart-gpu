# Simulation task context

Start at [simulation navigation](../../docs/agent/repo-map.md#simulation) and the
forward/backward driver under change. Follow only its named stage encoders and
forcing/resource handoffs. Check whether a helper is standalone before composing
it; [GPU contract](../../docs/GPU_CONTRACT.md) owns submission/readback rules.
Use [driver checks](../../docs/agent/test-map.md#simulation); kernel tests alone
cannot establish release, operation-order or output behavior of the driver.
