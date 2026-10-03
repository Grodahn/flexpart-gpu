# Validation task context

Select the artifact in [validation navigation](../../docs/agent/repo-map.md#validation-provenance):
case schema, input-equivalence verdict, candidate identity, comparison or provenance.
Read that contract and its existing consumer/test first; avoid loading all of
`case.rs` when changing a separate artifact. Cross-field semantics stay with the
case contract. Use [focused checks](../../docs/agent/test-map.md#validation-provenance)
and preserve the distinction between execution, input equivalence and scientific verdict.
