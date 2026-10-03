# Validation task context

Select the artifact in [validation navigation](../../docs/agent/repo-map.md#validation-provenance):
case schema, input-equivalence verdict, candidate identity, comparison or provenance.
Read that contract and its existing consumer/test first. The stable case facade
is [case/mod.rs](case/mod.rs); its private responsibility owners are listed in
the navigation row. Read the matching owner and colocated tests, not every case
module. Cross-field validation order stays in [case/manifest.rs](case/manifest.rs).
Use [focused checks](../../docs/agent/test-map.md#validation-provenance)
and preserve the distinction between execution, input equivalence and scientific verdict.
