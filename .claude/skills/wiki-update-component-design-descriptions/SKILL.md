---
name: wiki-update-component-design-descriptions
description: Update the component description files for the knowledge base
---

Update the individual component description .md files that are used for high-level design checks.  These reside in knowledge/components/ directory.  Each component has a separate .md file. The description should detail the component interfaces, receptacles and a description of its purpose and semantics.

Additionally, for each component, collect and include:

1. **Interface definitions** — Read the corresponding interface `.rs` file from `components/interfaces/src/` (e.g., `iblock_device.rs`, `idispatcher.rs`, `imemory_tier.rs`). Extract the full `define_interface!` block(s) showing trait method signatures with their doc comments, parameter types, and return types.

2. **Verified Properties** — Extract the "Verified Properties" comment block from the interface `.rs` file. This lists formally proved invariants (e.g., P1, P2, ..., P10) with their short descriptions, the total property count, and the number of verification conditions discharged. Include these in a dedicated "## Verified Properties" section in the component's `.md` file.

The interface definitions should appear in an "## Interface Definition" section showing the full trait signature. The verified properties section should list each property identifier (P1–PN) with its name and one-line description, preserving the format from the source comments.

