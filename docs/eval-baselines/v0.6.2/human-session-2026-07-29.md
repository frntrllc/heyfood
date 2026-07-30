# `v0.6.2` human TUI session — 2026-07-29

## Status

In progress. This record preserves the first release-priority finding from the
current public-artifact session. It does not assign a complete human score
before the remaining session protocol is exercised.

## Environment

- Public installed artifact: `heyfood 0.6.2`
- Qualified product SHA: `4cba1a038f67a5d4c8f075940922bd17e464fb01`
- Platform: macOS
- Account state: connected returning account with a ready dietary profile
- Production mutation: none requested

## Privacy boundary

This evidence retains no account identifier, household or dietary details,
credential material, device code, raw request, raw response, or personalized
answer. The submitted prompt is represented only as a benign
profile-awareness question.

## First-turn observation

The TUI opened in the connected state and invited a first question. After the
profile-awareness question was submitted:

1. the assistant body remained empty and displayed only an ellipsis;
2. the activity line exposed the machine stage `applying_dietary_graph`;
3. no partial answer or actionable progress appeared; and
4. the turn ended with the raw transport failure
   `event stream inactivity deadline expired`.

The interface did not explain whether it was safe to continue, state that no
automatic retry occurred, or offer an immediate recovery action. The result is
a P1 first-turn failure, recorded as
[issue #50](https://github.com/frntrllc/heyfood/issues/50).

## Source-backed interpretation

The Rust client has a finite 30-second SSE inactivity deadline. Its current TUI
renderer displays a `thinking` stage verbatim when the service does not provide
a friendly message, then appends the transport error to the conversation and
classifies the stream finish as completed. The exact service or proxy reason
that no heartbeat chunk reached the client is not yet established.

Increasing the timeout alone is not accepted remediation. The client and
service must prove a healthy heartbeat contract while preserving finite
cancellation, no blind retry, typed failure state, and a usable next turn.

## Human score status

| Dimension | Status | Current evidence |
|---|---|---|
| Discoverability | Pending | Not yet scored |
| Information hierarchy | Pending | Not yet scored |
| Keyboard confidence | Pending | Not yet scored |
| Response clarity | Failing gate | First response exposed machine language and produced no answer |
| Household clarity | Pending | Not yet scored |
| Safety and evidence trust | Pending | Not yet scored |
| Failure recovery | Failing gate | Raw timeout with no safe recovery guidance |
| Long-session comfort | Pending | Not yet scored |

No numeric human score is inferred from the observation. The session owner will
assign the final 1–5 scores after the remaining protocol.

## Planning linkage

- [Issue #48](https://github.com/frntrllc/heyfood/issues/48) owns the focused
  `v0.7.0` session and first-run excellence release.
- [Issue #50](https://github.com/frntrllc/heyfood/issues/50) owns this first-turn
  stream and presentation failure.
- [Issue #47](https://github.com/frntrllc/heyfood/issues/47) remains the separate
  logout-authority remediation lane.

The four-platform automated `100/100` result remains valid for its supported
rubric, but it did not qualify the production-backed human experience observed
here.
