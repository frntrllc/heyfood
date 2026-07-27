# TUI continuous evaluation

## Objective

Continuously evaluate the user experience of the **public installed `heyfood`
artifact**, preserve privacy-safe evidence, turn reproducible failures into one
deduplicated GitHub issue per experience category, and prove recovery after a
fix.

This is post-release product evaluation. It does not reopen the completed
`v0.5.0` recovery-release gate.

## The operating loop

```text
public archive
  -> checksum, attestation, signature/notarization smoke
  -> clean install under a real PTY
  -> deterministic user journeys and negative paths
  -> weighted rubric and evidence report
  -> deduplicated issue on regression
  -> fix plus a stronger deterministic assertion
  -> four-target rerun
  -> automatic issue closure on complete recovery
```

The loop runs daily in `.github/workflows/continuous-tui-eval.yml` and can be
dispatched for a named public version. It evaluates the four supported macOS
and Linux archives. A job that stops before producing evidence is itself a
triaged `evaluation-infrastructure` finding; missing reports cannot make the
run appear green.

## Evaluation layers

| Layer | Cadence | Environment | Purpose | Mutation policy |
|---|---|---|---|---|
| PR contract | Every change | Source and packaged fixture | Prevent compile, command, rendering, terminal, and evaluator regressions | Synthetic only |
| Public artifact | Daily and on demand | Exact GitHub Release archive, real PTY, deterministic backend | Exercise supported journeys exactly as installed users receive them | Isolated synthetic account |
| Production availability | Six-hourly after a dedicated canary identity exists | Exact public binary, production API | Detect auth, streaming, capability, and read-path outages | Read-only plus proposal cancel; never accept |
| Human experience session | Weekly and before a feature release | Public binary in a real terminal | Judge discoverability, hierarchy, language, comfort, and trust | Dedicated evaluation account |

The public-artifact layer is fully implemented. Production monitoring must not
reuse a founder or employee household. It activates only after a dedicated
least-privilege canary identity is provisioned with:

- an isolated household containing synthetic dietary data;
- a revocable CLI session with the minimum supported scopes;
- no Kroger or Health provider token;
- a versioned Grocery fixture where all automated proposals are cancelled;
- credentials stored only in a protected `native-eval` environment.

The fail-closed production workflow is
`.github/workflows/production-tui-canary.yml`. It remains inert unless the
repository variable `HEYFOOD_PRODUCTION_CANARY_ENABLED` is exactly `true`.
The job also rejects every ref except `refs/heads/main`, and the `native-eval`
environment's deployment-branch policy must independently allow only `main`.
No workflow may silently fall back to a real user account. Provisioning and
activation are tracked in
[heyfood issue #30](https://github.com/frntrllc/heyfood/issues/30).

The protected `native-eval` environment owns two secrets:

- `HEYFOOD_CANARY_STATE_BUNDLE_B64` is one atomic, base64-encoded JSON envelope
  containing only the dedicated account's `auth.native` and
  `credentials.native` documents. The job decodes it into a fresh owner-only
  state directory and never searches the runner or a maintainer's home
  directory.
- `HEYFOOD_CANARY_ROTATOR_APP_PRIVATE_KEY`, paired with the environment
  variable `HEYFOOD_CANARY_ROTATOR_APP_ID`, mints a short-lived non-personal
  GitHub App installation token. The workflow writes refreshed rotating
  credentials back as one encrypted environment-secret update before
  destroying the decoded files.

GitHub does not offer per-secret write permission. The rotator App therefore
has the broader repository `Environments: write` permission and can technically
write or delete other environment secrets in this repository. This residual
scope is explicit: the App is installed only on `frntrllc/heyfood`, has no
personal identity, mints a short-lived token only inside `native-eval`, and the
workflow calls exactly one fixed secret name. The main-only job condition,
selected-branch environment policy, protected private key, GitHub audit log,
and immediate token revocation are the defense-in-depth controls. A dedicated
OIDC broker enforcing the environment and secret name is the future
least-privilege replacement; the current design must not be described as
per-secret authorization.

The canary downloads and verifies the exact public Linux x86-64 archive. It
executes an authenticated agent turn, reads the active Grocery list, prepares a
synthetically screened proposal, explicitly cancels it, and proves the list
version and content did not change. It never sends an accept decision and
never installs a Kroger or Health provider token.

Only bounded status, latency, endpoint class, public version, archive digest,
and sanitized error type enter uploaded evidence. Raw prompts, responses,
names, emails, dietary details, tokens, conversation IDs, household IDs, and
list IDs remain in a private temporary directory and are destroyed. A
deterministic contract, credential, rotation, cleanup, or safety failure opens
an incident immediately. An availability failure must occur in two consecutive
completed canary runs for the same public release version, target, and archive
digest before it opens a product incident. Incident fingerprints and recovery
are exact-artifact-bound so a replaced or different archive cannot satisfy the
consecutive-failure or closure rule.

Activation order is deliberately strict:

1. Create the dedicated synthetic hello.food account and isolated household.
2. Add a synthetic profile and versioned Grocery fixture; add no provider
   integration.
3. Connect the public CLI once with the supported Grocery read/write scopes
   and the explicit owner-only file store in a temporary directory.
4. Encode the two resulting state documents into the version-1 atomic bundle
   and save it in `native-eval`.
5. Restrict `native-eval` deployments to the protected `main` branch.
6. Install the repository-only canary-rotator GitHub App and its protected
   private key; acknowledge the documented `Environments: write` residual
   scope.
7. Dispatch the workflow from `main` and inspect the privacy-safe evidence.
8. Set `HEYFOOD_PRODUCTION_CANARY_ENABLED=true` only after that first run
   passes; the six-hour schedule then becomes authoritative.

To revoke the canary, first set
`HEYFOOD_PRODUCTION_CANARY_ENABLED=false`, revoke the dedicated CLI device and
session in the hello.food authorization control plane, and delete both
`native-eval` secrets. Never repurpose the synthetic account or its household
for a human session.

## Supported-experience rubric

The executable rubric is
`tests/eval/tui-post-release-rubric.v2.json`. A supported public release must
score **100/100**:

| Category | Weight | Failure severity |
|---|---:|---|
| Bare-launch orientation and sign-in/create-account choice | 5 | P2 |
| First account connection, dietary onboarding, first response | 15 | P0 |
| Returning credential reload and usable second session | 15 | P0 |
| Household targeting and Grocery review/edit/cancel/accept/conflicts | 30 | P0 |
| Cancellation, uncertain outcomes, failure recovery, terminal restoration | 20 | P0 |
| 40/80/120-column presentation, no-color mode, exact packaged artifact | 15 | P1 |

The score is intentionally limited to supported behavior. Windows
distribution, Health-aware planning, native voice, and item-level Menu Watch
diff detail are listed in the report as roadmap coverage debt; they are not
converted into fake passing assertions. The published `v0.5.0` archive may
therefore retain the known first-run-orientation P2 until a successor artifact
contains and proves the automatic account-choice flow.

## Excellence standard

The next supported artifact is promotable only when all of these statements
are true:

- every supported installed archive scores 100/100 on the current rubric;
- no open P0 or P1 product finding applies to the candidate;
- the packaged executable—not a Cargo target—passes the clean-user,
  returning-user, household Grocery, failure-safety, and artifact matrix;
- first-frame latency is p95 below 100 ms across 30 warm probes;
- input-to-frame latency is p95 below 25 ms across 2,000 inputs with 500
  retained conversation entries;
- semantic content survives 40-, 80-, and 120-column layouts, `NO_COLOR`, a
  normal exit, application interrupt, body error, and panic;
- every human-session dimension scores at least 4/5 before a feature release;
- every public capability or landing-page demonstration has installed-artifact
  evidence for the behavior it claims.

The latency budgets are enforced by the source qualification suite in
`crates/heyfood-bin/tests/phase0_qualification.rs`. The installed matrix owns
artifact identity and real-terminal behavior. Neither substitutes for the
other.

## Human session protocol

Human evaluation is required because ANSI assertions cannot judge whether a
screen feels obvious or calm. Use the public binary and record only
privacy-safe observations.

1. Launch bare `heyfood`; assess time to orientation and command discovery.
2. Open help, return to the composer, and submit a neutral dinner question.
3. Cancel a streamed response and immediately submit another turn.
4. Change the household target, verify the active target is unmistakable, then
   reset it.
5. Open Grocery, inspect safety reasons, evidence, substitutions, and stable
   item references.
6. Prepare a Grocery change and cancel it. Confirm the copy clearly states
   that nothing changed.
7. Resize through 40, 80, and 120 columns; scroll history; edit a multiline
   prompt.
8. Exercise a typed backend failure or network interruption and recover in the
   same session.
9. Exit normally, relaunch, then interrupt the application. Verify the shell,
   cursor, echo, and scrollback are restored.
10. Score each dimension from 1 to 5: discoverability, information hierarchy,
    keyboard confidence, response clarity, household clarity, safety/evidence
    trust, failure recovery, and long-session comfort.

Any score of 1–2 is an issue. A score of 3 is P2 product debt. Scores of 4–5
pass. Attach terminal type and dimensions, but remove names, emails, dietary
details, prompts, tokens, and conversation IDs. The repository issue form
enforces this evidence hygiene.

## Triage and service levels

| Severity | Meaning | Response |
|---|---|---|
| P0 | Registration/core use blocked, unsafe mutation, misleading authority, credential failure, or terminal left broken | Stop promotion; owner immediately; fix or rollback |
| P1 | Core supported journey materially blocked or unusable on a supported platform | Owner same day; fix before the next feature release |
| P2 | Significant friction, confusing copy, or incomplete documented coverage with a workaround | Schedule in the current milestone |
| P3 | Polish, spacing, minor discoverability, or optional enhancement | Prioritized backlog |

Automated issues use the stable fingerprint
`tui-post-release-v2:<category>`. A repeated failure updates the existing issue
instead of creating noise. A complete four-target recovery run closes the
issue with a link to the evidence.

## Evidence contract

Every automated run uploads for 30 days:

- the rubric report (`post-release-eval.json`);
- the installed-artifact semantic evidence (`installed-core-matrix.json`);
- privacy-safe ANSI captures at supported widths and failure/exit paths.

Reports identify the public version, target, archive digest, score, category
results, stable findings, synthetic/production provenance, and limitations.
They never contain credentials, email addresses, household names from real
accounts, dietary profiles, raw production prompts, or provider data.

A verified baseline summary is committed under
`docs/eval-baselines/<version>/`; raw ANSI remains in ephemeral workflow
artifacts.

## Improvement rule

Closing an experience defect requires all four:

1. a product or contract correction;
2. a regression assertion at the lowest useful layer;
3. the installed public-artifact path passing for every supported target;
4. the next scheduled evaluation demonstrating recovery.

If the evaluator was wrong, correct the versioned rubric and explain the
contract change in the issue. Never weaken an assertion solely to restore a
green score.
