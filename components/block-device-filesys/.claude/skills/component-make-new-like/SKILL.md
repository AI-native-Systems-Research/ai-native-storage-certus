---
name: component-make-new-like
description: Create a new component skeleton based on an existing component.
argument-hint: "[source-component target-component]"
---

Ask the user to describe how the new component $1 will differ from the existing component $0, and what changes should be made up front.

1. Create a new component, named $1, based on an existing component $0.  The new component should support the same interfaces and receptacles as the originating component.

2. Copy tests and benchmarks from the originating component to the new component.

3. Copy specifications from .specify and specs directories.

4. Add a permissions file .claude/settings.json, in the newly created sub-directory, that allows access to the component itself, components/component-framework and any other directories corresponding to components that are listed as receptacles.  We want to avoid giving access to other components that are not directly used.

5. Copy skills, except those named 'component-make-new' or 'component-make-new-factor' from .claude/skills into the new component directory's .claude/skills.



