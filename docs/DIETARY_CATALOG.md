# Dietary option catalog

heyfood packages a versioned snapshot of the public hello.food dietary option
contract at `src/heyfood_cli/data/dietary_options.json`. The CLI reads this
resource at runtime, so an installed wheel does not depend on a hello.food
monorepo checkout.

The catalog is public client contract data. It contains display labels,
canonical selection identifiers, compatibility enum keys, and coarse
constraint tags needed to construct a profile payload. It does not contain the
proprietary evaluation engine, clinical evidence, scoring logic, prompts,
models, or service data that turn a profile and food into guidance.

## Source and synchronization

Inside the private hello.food monorepo, `shared/dietary_options.json` is the
canonical source. Run:

```bash
python scripts/sync_dietary_options.py
python scripts/sync_dietary_options.py --check
```

The first command updates the mobile and CLI snapshots byte-for-byte. The
second is the CI drift check and never writes files. Changes to identifiers,
enum mappings, or constraint tags must update the canonical source first and
must remain additive unless a reviewed compatibility migration exists.

The standalone public repository contains the generated snapshot but not its
private monorepo source or synchronization script. Public contributions should
not hand-edit the generated JSON; open an issue or propose the contract change
in the pull request, and a maintainer will apply and synchronize the canonical
change.

## Versioning

The top-level integer `version` identifies the catalog contract revision. A
release must include the snapshot expected by its runtime code and tests. A
catalog version change does not by itself change the Python package version,
but it must be described in release notes whenever it affects accepted input or
emitted profile data.

## Profile selection provenance

Synced profile schema v5 records canonical source selections separately from
their legacy compatibility fields:

- `diet_style_ids`, `allergy_ids`, and `health_condition_ids` identify the
  selected catalog entries.
- `additional_restriction_ids` and `additional_medical_constraints` preserve
  valid direct or legacy values that have no selected catalog source.
- `condition_severity_levels` records severity per selected condition; the
  legacy scalar is the maximum and is absent when no condition is selected.
- `selection_provenance_version: 1` makes even empty source arrays
  authoritative. If the field is absent, the service treats the payload as a
  legacy profile and performs a deterministic migration.

heyfood rebuilds flattened `preferences`, `restrictions`, and medical fields
from this source graph before every preview and upload. Clearing a category
therefore removes a derived value only when no other selected source still
requires it. Compatibility labels remain in custom arrays where an older
client has no matching enum; source-aware clients do not present those labels
as user-authored custom values.
