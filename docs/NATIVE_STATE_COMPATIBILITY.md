# Native-state compatibility and managed installation

## Immutable v0.6.2 boundary

The public v0.6.2 release predates the native household compatibility floor.
Its immutable asset set contains four `heyfood` archives and `SHA256SUMS`; it
does not contain a native-state declaration or a standalone
`heyfood-installer` verifier archive.

The checked-in installer now coordinates `SUPPORTED_VERSION` and
`NATIVE_STATE_RELEASE_VERSION` at `0.7.0`. It requires the v0.7.0 native-state
asset set and never fabricates, targets, replaces, or expects new assets on the
immutable v0.6.2 release. The current v0.7.0 installer accepts only its exact
supported version. Invoking it with `HEYFOOD_VERSION=0.6.2` is rejected before
any release download or executable replacement and leaves the installed v0.7.0
binary, compatibility floor, and account state unchanged.

The archived v0.6.2 installer and binary predate the future compatibility
floor. They do not enforce it, and direct execution of either after native
migration is unsupported and unprotected. They must not be used as a managed
downgrade path.

This source activation does not itself authorize a tag, upload, or public
release; those remain protected release-workflow decisions.

## Native-state release asset contract

Starting with v0.7.0, the complete release set is:

- four `heyfood-v<VERSION>-<TARGET>.tar.gz` product archives;
- four `heyfood-installer-v<VERSION>-<TARGET>.tar.gz` verifier archives;
- one `heyfood-v<VERSION>-native-state.json` declaration; and
- one `SHA256SUMS` binding the exact nine assets above.

Both macOS executables are signed and notarized before packaging. Per-target
smoke extracts the final archives, checks the platform signatures where
applicable, runs the final `heyfood` binary, and invokes the final packaged
verifier. Publication attests the exact product archives, verifier archives,
declaration, and checksum manifest. Public smoke downloads and repeats those
checks.

The managed installer checksum-verifies both host-target archives and the
declaration, stages both executables, captures the candidate's bounded
explicit `heyfood agent describe --schema-version 2` output, and invokes the
staged standalone verifier before the single executable replacement. The
ordinary `heyfood agent describe` compatibility surface is not repurposed for
installer metadata. Any checksum, archive-shape, version, floor, declaration,
manifest, verifier, or interruption failure leaves the previously installed
executable unchanged.

The verifier reads the declaration and floor with a 4 KiB ceiling and the
candidate manifest with a 1 MiB ceiling. It performs structural JSON parsing,
rejects duplicate keys at every depth, requires exactly one top-level
`native_state_compatibility` value through object-key uniqueness, and compares
that value to the exact canonical release declaration. Nested fields,
JSON-encoded text lookalikes, missing fields, unknown declaration fields,
noncanonical declaration bytes, invalid JSON, and oversized documents fail
closed.

## Manifest compatibility

The published agent manifest v1 remains closed and unchanged; its declared
`additive_optional_fields` value remains `false`. `heyfood agent describe`,
bare `heyfood agent`, `heyfood agent doctor`, and the MCP manifest tool keep
returning v1 so an already-installed v0.6.2 Agent Skill continues to work after
the binary is upgraded. Their schemas remain addressable as `manifest` and
`doctor`.

Native-state metadata is an explicit opt-in: `heyfood agent describe
--schema-version 2` returns the closed v2 manifest containing the exact
`native_state_compatibility` declaration, and `heyfood agent doctor
--schema-version 2` binds diagnostics to that manifest. Those schemas are
addressable as `manifest-v2` and `doctor-v2`. The managed installer and release
smoke request v2 explicitly; installed skills are neither silently replaced
nor required to understand v2 during the binary upgrade.

## Managed-install and archived-code boundary

Once the native-state floor exists, managed installation accepts only a
candidate whose maximum native-state version and complete capability set
satisfy that floor. The current v0.7.0 installer independently rejects every
requested version other than v0.7.0 before download, so a request for v0.6.2
cannot reach executable replacement. The compatibility floor cannot constrain
an independently executed archived installer or binary; archived v0.6.2 code
after migration remains unsupported and unprotected. A future supported
rollback must use a separately qualified native-state-compatible binary in its
native rollback-read-only mode.
