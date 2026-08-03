---
name: component-like-existing
description: Bootstrap a new component from an existing one, preserving its provided interfaces and receptacles under a different component name.
argument-hint: "[existing-component-name, new-component-name]"
---

Bootstrapping a new component named $1 from the existing component named $0 involves the
following steps. The intent is to clone the *contract* of $0 — the interfaces it provides
and the receptacles it consumes — while giving it a new component name and its own crate.
Provided interfaces and receptacles are **preserved as-is**: they reference the existing
shared interface definitions in `components/interfaces`. Do **not** create new `I...`
interface files for the clone.

1. The source component is $0 and the new component is $1. Locate the source component's
   sub-directory under `components/` (the lower-case, hyphen-ized form of $0, or the
   directory whose `define_component!` block declares $0). If it does not exist, return an
   error and stop.

2. If $1 was not supplied, interactively ask the user for the new component name and stop
   until provided. If a directory for $1 (lower-case, hyphen-ized) already exists under
   `components/`, return an error and stop.

3. Read the source component's `define_component!` block and record its `provides:`
   interface list and its `receptacles:` block verbatim. These are preserved unchanged in
   the clone.

4. Create a new sub-directory under `components/` whose name is the lower-case,
   hyphen-ized form of $1.

5. Copy the source component's implementation into the new directory — `src/` and
   `Cargo.toml`. Do **not** copy generated or spec directories: `target/`, `specs/`,
   `.specify/`, `info/`, or the source's `.claude/` directory.

6. In the copied source, rename the component type from the $0 name to the $1 name
   throughout (e.g. `FooBarComponent` → `BazComponent`), and update crate-name references,
   `//!` module docs, and doc-comment examples to match. Keep the `provides:` and
   `receptacles:` entries exactly as recorded in step 3 — the clone provides the same
   interfaces and consumes the same receptacles as $0.

7. In the new `Cargo.toml`, set `[package] name` to the lower-case, hyphen-ized form of $1.
   Preserve all `[dependencies]` from the source (they reflect the interfaces and
   receptacles being kept).

8. Register the new crate in the workspace root `Cargo.toml`:
   - Add the new component path to `members`.
   - If the source component is listed in `default-members`, add the new one there too.
   - Add a `[workspace.dependencies]` entry: `<crate-name> = { path = "components/<dir>" }`,
     mirroring any options (e.g. `default-features = false`) used by the source's entry.

9. Add a permissions file `.claude/settings.json` in the new sub-directory that allows
   access to the component itself, `components/component-framework`, `components/interfaces`,
   and the directories of any components corresponding to its receptacles. Avoid granting
   access to components that are not directly used. Model it on the source component's
   `.claude/settings.json` if present.

10. Copy skills — except those named `component-make-new`, `component-make-new-actor`, or
    `component-like-existing` — from `.claude/skills` into the new component directory's
    `.claude/skills`.

11. Run `specify init . --ai claude` in the new component source directory.

12. Run `specify extension add spec-kit-sync --from https://github.com/bgervin/spec-kit-sync/archive/refs/heads/master.zip`
    in the new component directory.

13. Verify the clone builds with `cargo build -p <new-crate-name>`. Report any errors.
