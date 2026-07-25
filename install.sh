#!/usr/bin/env bash

set -euo pipefail

# HEYFOOD_NATIVE_INSTALLATION_SUSPENDED=1

printf '%s\n' \
  'heyfood installer: installation is suspended because v0.4.0 and v0.4.1 were published before release authorization.' \
  'heyfood installer: do not install or use those releases; follow the repository support/security channels for updates.' \
  'heyfood installer: v0.5.0 candidate qualification covers macOS Apple Silicon, macOS Intel, Linux ARM64, and Linux x64.' \
  'heyfood installer: Windows distribution is deferred to v0.5.1; ordinary Windows source CI continues.' \
  >&2
exit 1
