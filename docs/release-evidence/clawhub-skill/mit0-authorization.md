# MIT-0 distribution authorization — hey.food ClawHub skill package

**Date:** 2026-08-01
**Status:** Authorized
**Applies to:** the hey.food agent skill published to ClawHub under the slug `heyfood`, owned by publisher `@heyfood`

---

## Authorization

FRNTR, LLC, as sole copyright holder, authorizes distribution of the hey.food
agent skill package via ClawHub under the **MIT-0** license.

**Scope:** the published ClawHub skill package only — `SKILL.md` and its
`references/` documents, sourced from `agent-integrations/skills/heyfood`. This
authorization does not extend to the `heyfood` client, its source, or any other
work in `frntrllc/heyfood`.

**The repository remains Apache-2.0.** This is a scoped dual-licensing
authorization for one distribution channel, not a relicensing of the project.

**Authorized by:** Justin Hambleton, on behalf of FRNTR, LLC — 2026-08-01

---

## Basis

ClawHub publishes all skills under MIT-0. There is no license selection flag on
`clawhub skill publish`, and every published skill inspected reports
`MIT-0 (Free to use, modify, and redistribute. No attribution required.)`. It is
a platform term, not a per-package choice — declining it means not publishing.

Verified 2026-08-01 against the ClawHub CLI (v0.23.1) and a sample of published
skills.

## Effect of the license change

Relative to the repository's Apache-2.0 license, MIT-0 differs in two ways that
were considered before authorizing:

| | Apache-2.0 | MIT-0 |
|---|---|---|
| Attribution required | Yes | **No** |
| Express patent grant | Yes | **No** |

The package is documentation — instructions intended to be copied into agent
contexts — so both differences were judged immaterial to this artifact.

Publication is effectively irrevocable for anyone who has already obtained a
published version. Future access can be withdrawn; distributed copies cannot be
recalled.

## Excluded from the published package

`agents/openai.yaml` is OpenAI/Codex presentation metadata and is **not** part of
the ClawHub payload. Publish from a staging copy with `agents/` removed.

## Related

- Skill content: `agent-integrations/skills/heyfood`
- Listing copy and publish command: `docs/plans/2026-08-01-heyfood-clawhub-listing-copy.md` (hellofood monorepo)
- Publisher identity: `@heyfood`, created 2026-08-01
