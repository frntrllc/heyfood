# Contributing to heyfood

Thank you for helping make food and dietary guidance easier to use from a
terminal. heyfood is a safety-sensitive developer tool, so clarity,
reproducibility, and conservative language matter as much as code quality.

## Before you start

- Read [DEVELOPMENT.md](DEVELOPMENT.md) and run the complete test suite.
- Search existing issues before opening a new one.
- For a material feature or contract change, open an issue before investing in
  implementation. Describe the user problem, proposed interface, compatibility
  impact, and how the change will be tested.
- Never include real access tokens, API keys, private dietary profiles, meal
  histories, health information, or production responses in an issue, fixture,
  commit, or pull request.
- Report vulnerabilities through [SECURITY.md](SECURITY.md), not a public issue.

## Pull requests

Keep each pull request focused and include:

1. the problem and intended developer experience;
2. tests covering success, failure, non-interactive behavior, and output
   contracts where relevant;
3. compatibility and migration notes for command, JSON, config, or auth changes;
4. documentation updates for user-visible behavior; and
5. confirmation that no secrets or personal data are present.

Changes to food-safety language must preserve the project rule: when the system
is uncertain, it must not overstate confidence. Use “generally safer,” “risky,”
“avoid,” or “unable to evaluate” as appropriate; do not introduce an absolute
“safe” conclusion.

Generated or substantially AI-assisted contributions are welcome when they are
reviewed by the contributor. Disclose material automated generation in the pull
request, verify the result, and ensure it does not reproduce code or content
under incompatible terms.

## Contribution terms

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in heyfood is provided under the Apache License 2.0, consistent
with Section 5 of that license. You represent that you have the right to submit
the contribution under those terms. FRNTR, LLC does not currently require a
separate contributor license agreement.

By participating, you agree to follow the
[Code of Conduct](CODE_OF_CONDUCT.md).
