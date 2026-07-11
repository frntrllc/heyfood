# Changelog

All notable changes to heyfood will be documented in this file. The project
uses semantic versioning while the public command, machine-output, config, and
authentication contracts are stabilized.

## Unreleased

No unreleased changes.

## 0.1.0 - 2026-07-11

### Added

- Apache-2.0 package metadata and FRNTR, LLC ownership notices.
- Standalone development, contribution, security, support, and conduct policies.
- Compatibility fixtures for the version 0.1.0 help and raw-output surfaces.
- Saved-location, restaurant selection, menu acquisition, and terminal
  presentation work captured by the committed baseline.
- RFC 8628-style short-code login, least-privilege scopes, secure credential
  storage, named contexts, and saved-location conversational context.
- Terminal-safe hey.food banner resources and accessibility controls.
- macOS/Linux CI across Python 3.11–3.13 plus built-artifact and pipx smoke
  verification.
- Synced-member discovery, explicit local conversation list/resume/clear
  workflows, and zsh/bash/fish shell completion.
- Stderr-only `--verbose` request correlation, timing, context, and auth
  refresh/retry diagnostics with sensitive-field filtering.
- Versioned JSON Schema for verdict, restaurant-fit, menu, recommendation, and
  recipe compatibility results, with canonical safety vocabulary.

### Changed

- The public package, CLI version display, and HTTP User-Agent now share the
  version declared in `src/heyfood_cli/__init__.py`.
- Menu acquisition now warns after 12 seconds and returns control after 30
  seconds with a resumable job reference instead of waiting up to four minutes.
- `--json` is now the canonical ANSI-free machine-output flag across data
  commands; progress and diagnostics use stderr, and `--raw` is a deprecated
  alias for the same writer.
- `--no-input` and non-TTY stdin now prevent prompts; onboarding mutations
  require explicit `--yes`, while dry-run remains network- and persistence-free.
- Client-side validation now matches service bounds for coordinates, radius,
  limits, dates, query lengths, recipe filters, and paired location flags.
- Dietary onboarding now emits schema-v5 source provenance, migrates legacy
  flattened profiles, and clears or replaces categories without erasing
  unrelated selections or retaining stale derived values.
- Recommendation scores are labeled as composite match ranks rather than safety
  verdicts; each human result includes a direct item-evaluation command.
- Typer and Click are bounded to the compatibility-tested minor lines so a
  fresh install preserves the frozen command and help grammar.

Each protected release moves relevant entries from `Unreleased` into a version
section with an ISO date. Removed or incompatible behavior must be called out
explicitly with migration guidance.
