# Security policy

## Supported versions

heyfood is currently pre-release software. Security fixes are applied to the
latest published version and the default development branch. Older pre-release
versions may require upgrading rather than receiving a backport.

## Report a vulnerability privately

Do not open a public issue for a suspected vulnerability.

Use GitHub’s private vulnerability reporting form:

https://github.com/frntrllc/heyfood/security/advisories/new

If that form is unavailable, email `hi@frntr.ai` with the subject
`[heyfood security]` and enough information to reproduce and assess the issue.
Do not send live credentials, access tokens, API keys, or another person’s
dietary or health information. Redact logs and use synthetic test data.

Useful reports include the affected version, operating system and Python
version, prerequisites, reproduction steps, expected impact, and any suggested
mitigation. FRNTR, LLC aims to acknowledge complete reports within three
business days and provide an initial assessment within seven business days.

## High-priority areas

- credential and token storage or disclosure;
- OAuth, device authorization, consent, and scope enforcement;
- config-file permissions, corruption, and cross-process races;
- machine-output leaks through stdout, stderr, logs, or terminal escapes;
- command injection, unsafe URL handling, or untrusted Rich markup; and
- exposure of private dietary, account, location, or meal data.

This repository contains only the open-source CLI. Suspected vulnerabilities in
the proprietary hello.food service should still be reported privately through
the same channels rather than discussed in a public CLI issue.
