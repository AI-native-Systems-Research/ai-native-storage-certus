---
name: tools-install-creusot
description: Install the Creusot Rust verification tool
---

This skill should not need sudo privileges.

## Prerequisites (must already be installed)
- curl
- Rust toolchain (rustup/cargo)
- opam (initialized with `opam init`)
- pip

If any prerequisite is missing, inform the user and stop.

## Installation Steps

1. Install z3-solver via pip if not already present:
   ```
   pip install z3-solver
   ```

2. Clone the Creusot repo to ~/creusot (if not already cloned):
   ```
   git clone https://github.com/creusot-rs/creusot ~/creusot
   ```

3. Run the install script with `--external z3` (uses the pip-installed z3):
   ```
   cd ~/creusot && ./INSTALL --external z3
   ```
   This installs cargo-creusot, creusot-rustc, Why3, why3find, Alt-Ergo, CVC4, and CVC5.

4. Fix why3find package resolution (required for proof discharge):
   ```
   mkdir -p ~/.local/share/creusot/_opam/lib/why3find/packages
   ln -sf ~/.local/share/creusot/share/why3find/packages/creusot \
          ~/.local/share/creusot/_opam/lib/why3find/packages/creusot
   ```

5. Add Creusot bin path to ~/.bash_profile:
   ```
   export PATH="$HOME/.local/share/creusot/bin:$PATH"
   ```

6. Verify installation:
   - Run `cargo creusot version` — should show version info for all components
   - Create a test project, build, and prove:
     ```
     cargo creusot new /tmp/creusot-verify-test --creusot-std ~/creusot/creusot-std
     cd /tmp/creusot-verify-test
     cargo creusot build
     why3find prove verif/<crate_name>_rlib/*.coma
     ```
   - Confirm output shows "Proved ... ✔"
   - Clean up: `rm -rf /tmp/creusot-verify-test`

## Notes
- Z3 4.16.0 (from pip) is newer than the recommended 4.15.3 — this produces a warning but works fine.
- The `--external z3` flag tells Creusot to use the system z3 rather than downloading its own.
