# Beta and usability acceptance protocol

This protocol collects only task success, navigation choice, elapsed time and defect severity. Do
not record security codes, position values, account balances, API keys or journal text.

## Five-trading-day Beta

Use one row per actual trading day. A day counts only when the release build is used during at
least one open A-share or Hong Kong session.

| Day | Date | Build commit | A/H session used | Duration | P0 | P1 | Performance report retained | Result |
|---:|---|---|---|---:|---:|---:|---|---|
| 1 | | | | | | | | pending |
| 2 | | | | | | | | pending |
| 3 | | | | | | | | pending |
| 4 | | | | | | | | pending |
| 5 | | | | | | | | pending |

Acceptance requires five rows, zero unresolved P0/P1 defects and an A6 performance report covering
at least one continuous hour. Attach only sanitized diagnostic files.

## Five-participant usability test

Recruit five target users who have not been coached on the new navigation. Assign anonymous IDs
P1–P5. Run the tasks in this order:

1. From launch, choose where to view today's market status.
2. Open Opportunities and save one candidate to the watchlist.
3. In Research, decide whether the evidence calls for action and state the invalidation condition.
4. In Portfolio, identify the largest concentration risk and its security.
5. Create a decision plan from the decision card.

Record one row per task:

| Participant | Task | First destination correct | Completed | Duration seconds | Needed help | Sanitized observation |
|---|---:|---|---|---:|---|---|
| P1 | 1 | | | | | |

Do not write the chosen security or portfolio value in the observation. Use descriptions such as
“could not find the risk section”.

## Acceptance calculations

- First-navigation accuracy: participants whose first destination for task 1 was Today / 5; target
  at least 80%.
- Per-task success: completed rows / 5; every core task target at least 90%.
- Opportunities median: median task-2 duration; target at most 90 seconds.
- Decision median: median task-3 duration; target at most 45 seconds.
- Portfolio-risk median: median task-4 duration; target at most 30 seconds.
- Plan completeness: plans containing trigger, invalidation and review date / created plans; target
  at least 90%.

The existing local `task-metrics.json` can corroborate elapsed navigation time, but it cannot prove
task success or replace participant observation.
