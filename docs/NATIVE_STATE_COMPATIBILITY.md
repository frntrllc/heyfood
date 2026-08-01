# Native-state compatibility and managed installation

## Immutable v0.6.2 boundary

The public v0.6.2 release predates the native household compatibility floor.
Its immutable asset set contains four `heyfood` archives and `SHA256SUMS`; it
does not contain a native-state declaration or a standalone
`heyfood-installer` verifier archive.

The checked-in installer therefore keeps `NATIVE_STATE_RELEASE_VERSION` empty
while `SUPPORTED_VERSION` remains `0.6.2`. In that dormant configuration it
uses the released v0.6.2 archive/checksum flow when no compatibility floor
exists. If a verified native-state floor already exists, it refuses v0.6.2
before downloading anything. It never fabricates, targets, replaces, or
expects new assets on the immutable v0.6.2 release.

Choosing the next release version is a separate release decision. The release
that first publishes D2 assets must update the workspace version,
`SUPPORTED_VERSION`, and `NATIVE_STATE_RELEASE_VERSION` to the same reviewed
new version. Source readiness does not authorize that version change, hosted
installer cutover, tag, upload, or release.

## D2 release asset contract

Starting with that future activated release, the complete release set is:

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
`heyfood agent describe` output, and invokes the staged standalone verifier
before the single executable replacement. Any checksum, archive-shape,
version, floor, declaration, manifest, verifier, or interruption failure
leaves the previously installed executable unchanged.

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
`additive_optional_fields` value remains `false`. D2 self-description uses the
closed v2 manifest and v2 doctor schemas. The v1 schemas remain embedded and
addressable as `manifest-v1` and `doctor-v1` for compatibility inspection.

## Rollback boundary

Once the native-state floor exists, managed installation accepts only a
candidate whose maximum native-state version and complete capability set
satisfy that floor. A pre-D2 binary is rejected before executable replacement.
Direct execution of a pre-D2 binary after native migration is unsupported;
the supported rollback is a D2-capable binary operating in its native
rollback-read-only mode.
