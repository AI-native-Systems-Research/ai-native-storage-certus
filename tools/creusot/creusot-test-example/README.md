# Creusot Test Example

A minimal example demonstrating [Creusot](https://github.com/creusot-rs/creusot) formal verification of Rust code.

## Verified Functions

- `safe_add` — addition with overflow protection proof
- `abs` — absolute value with correctness proof
- `max` — maximum of two values with postcondition proof
- `clamp` — value clamping with range guarantee proof

## Usage

```bash
# Ensure Creusot is installed (see .claude/skills/tools-install-creusot/SKILL.md)
export PATH="$HOME/.local/share/creusot/bin:$PATH"

# Clean first if you previously ran plain `cargo build` (stale artifacts block translation)
cargo clean

# Build and prove in one step
cargo creusot

# Or separately:
#   cargo creusot --only coma    # generates .coma verification conditions
#   why3find prove verif/creusot_test_example_rlib/*.coma
```

Expected output:
```
Library verif.creusot_test_example_rlib.abs: ✔ (1)
Library verif.creusot_test_example_rlib.clamp: ✔ (1)
Library verif.creusot_test_example_rlib.max: ✔ (1)
Library verif.creusot_test_example_rlib.safe_add: ✔ (1)
Proved (4 files) ✔
```
