# Dietary option catalog

This repository retains a generated hello.food dietary-option snapshot at
`assets/dietary/dietary_options.v2.json`. The Rust TUI embeds and reads the
exact reviewed v2 bytes to drive first-run dietary onboarding and the explicit
`heyfood onboard` workflow.

The catalog contains public client contract data: display labels, canonical
selection identifiers, compatibility enum keys, and coarse constraint tags. It
does not contain the proprietary evaluation engine, clinical evidence, scoring
logic, prompts, models, or service data that turn a profile and food into
guidance.

## Current boundary

- `ask`, `item`, Grocery, and other hosted workflows use the dietary profile
  associated with the authenticated hello.food account.
- The draft Rust TUI provides a local, eight-step catalog-selection flow for
  diets, allergies, health conditions, severity, avoided ingredients, activity,
  cuisines, notes, and final review.
- The client sends no dietary selection while the flow is in progress. Literal
  `save` first grants versioned profile-sync consent when required, reads the
  current profile version, and performs one optimistic profile upload.
- Cancellation before profile-upload dispatch is reported as non-mutating for
  the profile (consent may already have been granted). An unobserved response
  after upload dispatch is reported as outcome-unknown and directs the user to
  inspect `/profile` before retrying.
- `heyfood onboard` and the TUI `/onboard` action replace the synchronized
  `_self` profile. Broader profile editing and household-member writes are not
  claimed by this workflow.

## Source and synchronization

The canonical catalog is maintained inside the hello.food product system and
synchronized into public compatibility snapshots by maintainers. The public
repository does not contain the private canonical source or its synchronization
workflow.

Do not hand-edit generated catalog data in a public contribution. Open an issue
describing the contract change; a maintainer can update the canonical source,
regenerate public snapshots, and review compatibility impact.

## Profile schema context

Synced profile schema v5 records canonical source selections separately from
legacy flattened fields. Source arrays preserve selected diet styles,
allergies, health conditions, and additional restrictions; provenance metadata
distinguishes authoritative empty selections from older payloads that require a
deterministic migration.

The Rust onboarding mapper preserves those source selections and provenance
while deriving the compatibility fields expected by the frozen Python
baseline. Exhaustive tests cover every retained v2 catalog option.
