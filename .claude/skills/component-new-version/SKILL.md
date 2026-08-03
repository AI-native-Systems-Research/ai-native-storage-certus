---
name: component-new-version
description: Create a new version of a component
argument-hint: "[component-name]"
---

Creating a new version of component named $0 involves the following steps:

1. Check that component $0 exists. If it does not exist, ask the user to clarify what component to create a new version of.

2. Create a new directory as a sibling, with the next version label. For example, if the latest version is v1, then the next directory should be called v2.

3. Copy the implementation from the prior version. Do not copy other directories such as info.

4. Rename the component name to reflect the new version, e.g., FoobarComponentV2

5. Copy permissions file .claude/settings.json, in the newly created sub-directory, that allows access to the component itself, components/component-framework and any other directories corresponding to components that are listed as receptacles.  We want to avoid giving access to other components that are not directly used.

6. Copy into the new component directory's `.claude/skills` only the skills whose names match an `include` pattern in `.claude/component-local-skills.json` (patterns support a trailing `*` wildcard; other entries match exactly). This allowlist is the single source of truth for which skills are component-local — do not hard-code the list here. Skip any skill that does not match.

7. Run 'specify init . --ai claude' in the component source directory.

8. Run 'specify extension add spec-kit-sync --from https://github.com/bgervin/spec-kit-sync/archive/refs/heads/master.zip' in the component directory.




