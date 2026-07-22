#!/usr/bin/env bash

set -euo pipefail

# HEYFOOD_NATIVE_INSTALLATION_SUSPENDED=1

printf '%s\n' \
  'heyfood installer: installation is suspended because v0.4.0 and v0.4.1 were published before release authorization.' \
  'heyfood installer: do not install or use those releases; follow the repository support/security channels for updates.' \
  >&2
exit 1
