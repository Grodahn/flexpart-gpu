# Navigation dry runs

These are context-selection dry runs, not scientific validation results.
Start with [root AGENTS](../../AGENTS.md) and one [domain row](repo-map.md).
The selected files and commands were checked against the issue #123 base checkout.
No task below needs a repository-wide inventory.

| Representative task | Read first | Expected working set | Allowed adjacent inspection | Focused verification |
| --- | --- | --- | --- | --- |
| Meteorology: reject an invalid instantaneous time bracket | [meteorology](repo-map.md#meteorology), temporal contract linked there | GPU temporal host + WGSL pair, shared time-bracket validator and temporal GPU tests from that row | canonical timestamp metadata and interpolation fixture provenance | [meteorology time row](test-map.md#meteorology); exact interior/invalid-bracket tests |
| Physics: correct wet mass-removal probability | [wet deposition](repo-map.md#wet-deposition), deposition/GPU contracts linked there | wet GPU host + WGSL pair and deposition mass-evolution test | supplied species/precipitation forcing identity, simulation encoder call site | [wet verification](test-map.md#wet-deposition); analytical evolution, device diagnostic, WET-008 paired check |
| Validation: fail on a missing consumed manifest input | [validation/provenance](repo-map.md#validation-provenance), provenance contract | corpus manifest writer, shared provenance owner and their regression tests from that row | case schema and existing input audit when they supply that input | [validation verification](test-map.md#validation-provenance); manifest regression, existing consumer run |
| Validation case: inspect a rejected output timing/grid declaration | [validation/provenance](repo-map.md#validation-provenance), case schema | [stable facade](../../src/validation/case/mod.rs) and [output owner with colocated tests](../../src/validation/case/output.rs) | [root manifest](../../src/validation/case/manifest.rs) for validation order; [oracle COMMAND](../../src/validation/case/oracle.rs) for synchronization handoff; [test fixtures](../../src/validation/case/test_support.rs) only when needed | `cargo test --lib validation::case::output::tests`; [context comparison](validation-case-decomposition.md) |

For the first task, unrelated provider decoding, convection and output files stay
outside the working set. For the second, changing precipitation transformation
would cross into its own meteorology contract. For the third, changing comparison
thresholds would cross into the scientific evaluation contract. Stop and record a
dependency at those boundaries; navigation metadata never expands an issue's scope.

The [checker](../../scripts/check_agent_navigation.py) audits linked paths,
references, rendered heading anchors and command targets, including named exact-test
selectors. These examples also provide a review checklist when navigation facts
change: resolve the row, select only the listed files, and verify the command
without reconstructing the repository's directory tree.
