# Active Execution Plans

One markdown file per active plan. Each plan is a first-class artifact, checked in, with a progress log.

A plan looks like:

```
---
title: <short title>
status: in_progress
opened: YYYY-MM-DD
owner: <name>
links:
  - spec: ../../product-specs/v1-livepeer-indexer.md#<section>
---

## Goal
<one paragraph>

## Approach
<bullets>

## Progress log
- YYYY-MM-DD: <what changed>
```

Plans move to `../completed/` with `status: done` and a final summary when finished.
