---
name: component-sync-specs
description: Ensure a component implementation is synchronized with its specifications.
argument-hint: "[component-name, component-name, ...]"
---

Work hard and think hard about this task. Spec-sync demands careful, thorough
analysis: verify every FR/SC against the actual implementation, corroborate each
drift finding with concrete file:line evidence, and never rubber-stamp. Do not
rush — reason deeply about each discrepancy before proposing a change.

For each component identified in $ARGUMENTS, run the following:
1. /speckit-sync-analyze
2. /speckit-sync-propose --interactive
3. /speckit-sync-apply

