# Dietary option catalog

This repository retains a generated hello.food dietary-option snapshot for
legacy Python compatibility and contract history. The current Rust command
surface does not expose profile onboarding or profile editing and does not read
this catalog as an active runtime feature.

The catalog contains public client contract data: display labels, canonical
selection identifiers, compatibility enum keys, and coarse constraint tags. It
does not contain the proprietary evaluation engine, clinical evidence, scoring
logic, prompts, models, or service data that turn a profile and food into
guidance.

## Current boundary

- `ask`, `item`, Grocery, and other hosted workflows can use the dietary profile
  already associated with the authenticated hello.food account.
- The Rust CLI does not currently provide onboarding, profile display, profile
  editing, or catalog-selection commands.
- Hidden legacy profile/onboarding commands return `command_not_available` and
  are not a supported interface.
- The generated snapshot must not be presented as evidence that the current
  native client can edit a profile.

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

Those profile contracts are hosted hello.food behavior. They do not make the
current Rust CLI a profile-management client.
