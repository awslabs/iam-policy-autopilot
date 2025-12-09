# IAM Policy Autopilot Custom Lints

Custom [dylint](https://github.com/trailofbits/dylint) lints for enforcing project-specific patterns.

## Available Lints

### `node_kind_literal`

Enforces use of constants instead of string literals when comparing with `.kind()` method calls.

**Bad:**
```rust
if node.kind() == "composite_literal" {
    // ...
}
```

**Good:**
```rust
use crate::extraction::go::node_kinds::COMPOSITE_LITERAL;
if node.kind() == COMPOSITE_LITERAL {
    // ...
}
```

## Usage

### Install dylint

```bash
cargo install cargo-dylint dylint-link
```

### Run lints

```bash
# Check all workspace packages
cargo dylint --all --workspace

# Check specific package
cargo dylint --all --package iam-policy-autopilot-policy-generation

# Check all targets (including tests)
cargo dylint --all --workspace -- --all-targets
```

## CI Integration

The lints run automatically on every PR via `.github/workflows/pr-checks.yml`.

## Development

### Test the lints

```bash
cd iam-policy-autopilot-lints
cargo test
```

### Update test expectations

If you modify a lint's output, update the expected stderr file manually:

1. Run `cargo test` to see the diff
2. Update the corresponding `.stderr` file in `ui/` directory
3. Run `cargo test` again to verify

### Add a new lint

1. Create `src/my_new_lint.rs`
2. Add `mod my_new_lint;` to `src/lib.rs`
3. Add test cases in `ui/my_new_lint.rs`
4. Create expected output in `ui/my_new_lint.stderr`
5. Run `cargo test` to verify

See [dylint documentation](https://github.com/trailofbits/dylint) for details on writing lints.
