# Correctness Argument: Java Extraction Module

This document provides a correctness argument for the Java extraction module at
`src/extraction/java/`. It is intended for code review and audit contexts.

---

## 1. Architecture Overview

The module implements a two-phase pipeline:

```
Vec<SourceFile>
     │
     ▼
JavaLanguageExtractorSet          (single AST scan → ExtractionResult)
  ├── JavaImportExtractor          → imports + utility_imports
  ├── JavaPaginatorExtractor       → paginators
  ├── JavaWaiterCallExtractor      → waiters
  └── JavaMethodCallExtractor      → calls
     │
     ▼
JavaMatcher                       (ExtractionResult → Vec<SdkMethodCall>)
  ├── match_service_calls          × method_lookup
  ├── match_waiters                × waiter_lookup
  ├── match_paginators             × method_lookup
  └── match_utilities              × java-sdk-v2-utilities.json
```

The test suite mirrors this structure: extractor-level tests (unit), matcher-level tests
(integration), and entry-point tests (end-to-end).

---

## 2. Test Harness

Two macros drive the file-driven tests:

**[`java_extractor_test!`](test_macros.rs:38)** reads a `.java` + `.json` pair, runs
[`JavaLanguageExtractorSet::extract_from_file`](extractor.rs:192), and asserts
`result.<field> == expected.expected_<field>` using `assert_eq!` on the full `Vec`. Assertions
are structurally deep: every field of every extracted item is checked, including source location
(file, line, column), parameter values with `Resolved`/`Unresolved` discriminants, and
`ReceiverDeclaration`.

**[`java_matcher_test!`](test_macros.rs:157)** reads a JSON descriptor (source files, a
service-index file, expected SDK calls), runs the full extraction + merge + matching pipeline,
and asserts `actual == expected_sdk_calls` using full [`SdkMethodCall`](../../extraction/mod.rs:219)
equality — including `Name`, `PossibleServices`, and the complete `Metadata` field
(`Expr`, `Location`, `Parameters`, `Receiver`, `Usage`). Every fixture in
`tests/java/matchers/` carries the expected `Metadata` block, so matcher tests also validate
that the extraction pipeline propagates source location and parameter values correctly end-to-end.

Both macros use `rstest`'s `#[files(...)]` attribute, so every fixture file in the test
directories is automatically picked up as a separate test case.

**Ordering**: extractor tests rely on tree-sitter's deterministic source-order traversal and use
plain `Vec` equality. Matcher tests compare in output order; the ordering contract is documented
in the [`java_matcher_test!`](test_macros.rs:157) macro doc-comment: sub-matchers are called in
a fixed sequence (service calls → waiters → paginators → utilities), and within each sub-matcher
calls are emitted in source order. Entry-point tests sort both sides by `name` before comparing.

---

## 3. Test Coverage

### 3.1 Extractor Layer

**Import extraction** covers specific and wildcard service imports, static imports, nested model
class imports, Smithy-renamed services (e.g. `cloudwatchlogs` → `logs`), and mixed patterns.
Parameterised unit tests for [`extract_service_from_import`](extractors/import_extractor.rs:173)
cover all equivalence classes: direct-match, Smithy-renamed, auto-generated dash-restoration, and
non-AWS imports.

**Utility import extraction** covers S3 TransferManager, S3 Presigner, DynamoDB Enhanced Client,
SQS Async Batch Manager, and a mixed file containing both utility and regular service imports. The
`mixed_utility_and_service` fixture verifies that each import is routed to either `imports` or
`utility_imports` — not both.

**Method call extraction** covers zero-argument calls, literal argument resolution (string,
numeric, boolean, null, identifier), builder chains, field-access receivers, multi-variable
declarations, and all three lambda shadowing patterns (inferred parameter, typed parameter, and
local variable shadowing a field inside a lambda). These shadowing fixtures directly validate the
innermost-scope-wins semantics of [`find_receiver_declaration`](extractors/utils.rs:116).
Additional fixtures cover Java 16+ record types (record component as receiver, compact
constructor, and local variable shadowing a record component) and `instanceof` pattern matching
(single and two-service cases). A sealed class fixture confirms that a field in a `final class`
implementor is found by the class-body scan and produces a populated `ReceiverDeclaration`.

**Waiter extraction** covers sync and async waiters, custom waiter config, receiver declaration
from local var and formal parameter, assignment usage detection, a non-`waitUntil` negative case,
and explicit negative cases for field-access receiver and `var`-typed declaration.

**Paginator extraction** covers sync and async paginators, assignment and non-assignment usage,
multiple paginators in one file, receiver declaration from local var and formal parameter, and a
non-paginator negative case.

### 3.2 Matcher Layer

**Service call matching** covers the three disambiguation tiers: FQN type (no import needed),
specific import lookup, and wildcard import + `serviceId`-based matching. Key fixtures include:
`two_service_calls_same_operation` (two typed receivers of different client types calling the same
method, emitting separate `SdkMethodCall`s with distinct `possible_services`); `unknown_method`
(method absent from `method_lookup` → no output); `as_bytes_call` (regression test for
`getObjectAsBytes` → `AsBytes` suffix stripping → `getObject`); and
`try_with_resources_type_disambiguated` (`ReceiverDeclaration` type name enables Tier-1
disambiguation when both `S3Client` and `S3ControlClient` are imported). `instanceof` pattern
binding and sealed class fixtures confirm that Tier-1 disambiguation works when the type is
resolved from a pattern-binding variable or a field in a sealed class implementor.

**Waiter matching** covers the same three disambiguation tiers, plus `waiter_init_skipped`
(`.waiter()` call without a subsequent `waitUntil*` → no output) and
`two_service_waiters_different_operations` (regression test for the per-operation grouping bug
where two services share a waiter name but poll different underlying operations).

**Paginator matching** mirrors service-call coverage: all three disambiguation tiers, two-service
clash, and unknown paginator → empty output.

**Utility matching** covers S3 TransferManager `uploadFile`, S3 Presigner `presignGetObject`,
DynamoDB Enhanced Client `putItem`, SQS Batch `sendMessage`, `receiveMessage`, and
`deleteMessage`; plus negative cases for missing utility import, wrong method name, and missing
SQS batch import.

**Orchestrator** exercises [`JavaMatcher::match_calls`](matcher.rs:60) end-to-end. The
`cross_file_import_isolation` fixture is the key test: two source files extracted together (one
importing `S3Client`, one importing `S3ControlClient`) both call `putObject`, and the matcher
narrows each call using only the imports from its own file.

### 3.3 Entry-Point Layer

7 fixtures exercise the full async pipeline including parallel `JoinSet` execution and
`ExtractionResult::extend` merging. The `multiple_files` fixture is the only test that exercises
the multi-file merge path. Three inline tests cover the error paths: empty input →
`Validation` error; non-Java file → `MethodExtraction` error; and malformed Java source →
`extract_from_file` succeeds and returns partial results (documenting tree-sitter's
error-tolerant behaviour).

---

## 4. Sufficiency Argument

**Import normalisation**: all three code paths (utility import, AWS service import, non-AWS
discard) are exercised. Parameterised unit tests cover every equivalence class in the
service-name mapping.

**Receiver declaration resolution**: all seven scope-walk tiers are covered (block local var,
`try`-with-resources resource, `instanceof` pattern binding, lambda parameter,
method/constructor/compact-constructor formal parameter, class field, and record component),
along with all three lambda shadowing patterns. Negative cases (`var` type, field-access receiver)
confirm the extractor does not fabricate a type when it cannot be determined statically.
Multi-variable declarations and multi-resource `try`-with-resources headers are explicitly tested.

**Disambiguation**: tested at all three tiers (FQN, import lookup, service-id matching) for
service calls, waiters, and paginators. The `apply_import_filter` function has 4 parameterised
cases covering narrowing, pass-through, no-overlap, and partial-overlap.

**Waiter operation grouping**: a dedicated regression test with a comment documenting the original
bug makes the invariant explicit and guarded.

**Utility matching**: both conditions (matching utility import and matching method name) are tested
independently as negative cases. Representative fixtures cover S3 Presigner, DynamoDB Enhanced,
and SQS `deleteMessage`, so a typo in `java-sdk-v2-utilities.json` for any of these features
would be caught.

**Cross-file import isolation**: the `cross_file_import_isolation` orchestrator fixture confirms
that imports from one source file never affect call resolution in another.

**Implementation invariants**:
- [`test_combined_rule_builds_without_error`](extractor.rs) asserts that all extractor
  discriminator labels are unique.
- [`IMPORT_TABLE`](extractors/utility_import_extractor.rs) is sorted deterministically and a
  `debug_assert!` fires if any two entries share a prefix, making first-match semantics stable.

---

## 5. Known Limitations

**Method references** (`s3Client::putObject`) are `method_reference` AST nodes, not
`method_invocation`. The extractor matches only `method_invocation`, so method references are
silently ignored. This exclusion is not documented or tested.

**`AsBytes` suffix stripping** is only tested for S3 (`getObjectAsBytes`). The stripping logic in
[`match_call`](matchers/service_call.rs:80) is generic but no test covers other services.

**Reflection-based calls** are not detectable by AST pattern matching and are not expected to be
handled. This assumption is not documented.
