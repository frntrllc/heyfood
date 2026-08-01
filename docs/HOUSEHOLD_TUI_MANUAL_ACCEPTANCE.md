# Native household TUI manual acceptance

This is the required human-attached-terminal acceptance pass for the native
household lifecycle. Do not automate these steps, capture the terminal, or
record household labels, stable IDs, profile answers, account identifiers, or
credentials.

## Preconditions

- Use a disposable test account in an attached terminal.
- Use the exact candidate executable intended for qualification.
- Enable the reviewed native-household rollout for that candidate.
- Confirm no repair, teardown, or post-logout recovery is pending.
- Record only `PASS`, `FAIL`, or a content-free failure category for each row.

## Journey

1. Launch the TUI and complete owner onboarding if the disposable account
   requires it.
2. Run `/household`. Confirm the panel shows the owner and the same current
   context as the TUI chrome.
3. Run `/household add`. Complete relationship, display label, age band, and
   all eight version-1 dietary-profile steps. Review and save.
4. Confirm one success is shown only after the member and declared profile are
   committed, the new member is selected, and panel/chrome agree.
5. Exit normally, relaunch the exact executable for the same account, and run
   `/household`. Confirm the selected member context survived restart.
6. Submit an ordinary turn while the member is selected. Confirm it fails
   locally with the hosted-context limitation and does not prompt for consent
   or begin microphone capture.
7. Run `/for everyone`. Confirm panel and chrome show `Everyone`, then confirm
   an ordinary turn fails with the same local hosted-context limitation.
8. Run `/for me`. Confirm panel and chrome return to the owner context and an
   ordinary owner turn follows the existing hosted flow.
9. Start `/household add` again, enter only non-sensitive synthetic draft
   values, then cancel before save. Run `/household` and confirm no additional
   member was created.
10. Exit normally and confirm the terminal presentation is restored.

## Content-free result record

| Row | Result | Allowed failure category |
| --- | --- | --- |
| Owner panel/chrome agreement |  | `presentation_mismatch` |
| Atomic add and profile save |  | `local_commit_failed` |
| Selected member after save |  | `context_apply_failed` |
| Restart continuity |  | `restart_continuity_failed` |
| Member hosted-turn preflight |  | `member_preflight_failed` |
| Everyone selection/preflight |  | `everyone_preflight_failed` |
| Return to owner context |  | `owner_context_failed` |
| Pre-save cancellation |  | `cancellation_failed` |
| Terminal restoration |  | `terminal_restoration_failed` |

The candidate is not release-ready until every row is `PASS`. A failure report
must contain only the category above and the candidate version/digest.
