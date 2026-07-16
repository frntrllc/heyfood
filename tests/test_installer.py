from __future__ import annotations

import hashlib
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]
INSTALLER = ROOT / "install.sh"


def _supported_python() -> str:
    candidates = [
        shutil.which("python3.13"),
        shutil.which("python3.12"),
        shutil.which("python3.11"),
        sys.executable,
    ]
    for candidate in candidates:
        if not candidate:
            continue
        result = subprocess.run(
            [
                candidate,
                "-c",
                "import sys; raise SystemExit(0 if (3, 11) <= sys.version_info[:2] < (3, 14) else 1)",
            ],
            check=False,
        )
        if result.returncode == 0:
            return candidate
    pytest.skip("installer tests require a supported Python interpreter")


def _write_executable(path: Path, source: str) -> None:
    path.write_text(source, encoding="utf-8")
    path.chmod(0o755)


def _fake_pipx(path: Path) -> None:
    _write_executable(
        path,
        r"""#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" == "--version" ]]; then
  printf '1.7.1\n'
  exit 0
fi

[[ "${1:-}" == "install" ]] || exit 64
if [[ "${HEYFOOD_TEST_PIPX_EXIT:-0}" != "0" ]]; then
  exit "$HEYFOOD_TEST_PIPX_EXIT"
fi

printf '%s\n' "$@" > "$HEYFOOD_TEST_LOG"
mkdir -p "$PIPX_BIN_DIR"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'printf '\''heyfood %s\n'\'' "${HEYFOOD_TEST_INSTALLED_VERSION:-0.2.0}"' \
  > "$PIPX_BIN_DIR/heyfood"
chmod 755 "$PIPX_BIN_DIR/heyfood"
""",
    )


def _environment(tmp_path: Path) -> tuple[dict[str, str], Path]:
    home = tmp_path / "home"
    fake_bin = tmp_path / "fake-bin"
    pipx_bin = tmp_path / "pipx-bin"
    pipx_home = tmp_path / "pipx-home"
    data_home = tmp_path / "data"
    log = tmp_path / "pipx-arguments.txt"
    home.mkdir()
    fake_bin.mkdir()
    _fake_pipx(fake_bin / "pipx")

    env = os.environ.copy()
    for name in (
        "HEYFOOD_VERSION",
        "HEYFOOD_WITH_KEYRING",
        "HEYFOOD_PYTHON",
        "PIPX_BIN_DIR",
        "PIPX_HOME",
        "XDG_BIN_HOME",
        "XDG_DATA_HOME",
    ):
        env.pop(name, None)
    env.update(
        {
            "HOME": str(home),
            "HEYFOOD_PYTHON": _supported_python(),
            "HEYFOOD_TEST_INSTALLED_VERSION": "0.2.0",
            "HEYFOOD_TEST_LOG": str(log),
            "PATH": f"{fake_bin}{os.pathsep}{env.get('PATH', '')}",
            "PIPX_BIN_DIR": str(pipx_bin),
            "PIPX_HOME": str(pipx_home),
            "XDG_DATA_HOME": str(data_home),
        }
    )
    return env, log


def _run(tmp_path: Path, **overrides: str) -> tuple[subprocess.CompletedProcess[str], Path]:
    env, log = _environment(tmp_path)
    env.update(overrides)
    result = subprocess.run(
        ["/bin/bash", str(INSTALLER)],
        cwd=tmp_path,
        env=env,
        check=False,
        capture_output=True,
        text=True,
    )
    return result, log


def test_installer_source_has_fail_closed_security_invariants() -> None:
    source = INSTALLER.read_text(encoding="utf-8")
    checksum = (ROOT / "install.sh.sha256").read_text(encoding="utf-8")
    syntax = subprocess.run(
        ["/bin/bash", "-n", str(INSTALLER)],
        check=False,
        capture_output=True,
        text=True,
    )

    assert syntax.returncode == 0, syntax.stderr
    assert source.startswith("#!/usr/bin/env bash\n\nset -euo pipefail\n")
    assert not re.search(r"^\s*sudo(?:\s|$)", source, flags=re.MULTILINE)
    assert not re.search(r"^\s*eval(?:\s|$)", source, flags=re.MULTILINE)
    assert not any(name in source for name in (".bashrc", ".zshrc", ".profile"))
    assert "https://pypi.org/simple" in source
    assert "--index-url" in source
    assert "--trusted-host" not in source
    assert "HEYFOOD_PACKAGE=\"heyfood-cli\"" in source
    assert '${EUID:-0}' in source
    assert checksum == f"{hashlib.sha256(INSTALLER.read_bytes()).hexdigest()}  install.sh\n"


def test_installs_fixed_public_package_and_verifies_command(tmp_path: Path) -> None:
    result, log = _run(tmp_path)

    assert result.returncode == 0, result.stderr
    assert "Installed heyfood 0.2.0" in result.stdout
    assert "Next: heyfood login" in result.stdout
    assert "Add heyfood to this shell's PATH:" in result.stdout
    assert log.read_text(encoding="utf-8").splitlines() == [
        "install",
        "--quiet",
        "--force",
        "--python",
        _supported_python(),
        "--index-url",
        "https://pypi.org/simple",
        "--pip-args=--disable-pip-version-check --no-input",
        "heyfood-cli",
    ]


def test_validates_pinned_keyring_requirement(tmp_path: Path) -> None:
    result, log = _run(
        tmp_path,
        HEYFOOD_VERSION="0.2.0",
        HEYFOOD_WITH_KEYRING="1",
    )

    assert result.returncode == 0, result.stderr
    assert log.read_text(encoding="utf-8").splitlines()[-1] == (
        "heyfood-cli[keyring]==0.2.0"
    )


def test_runs_safely_when_streamed_to_bash(tmp_path: Path) -> None:
    env, log = _environment(tmp_path)

    result = subprocess.run(
        ["/bin/bash"],
        cwd=tmp_path,
        env=env,
        input=INSTALLER.read_text(encoding="utf-8"),
        check=False,
        capture_output=True,
        text=True,
    )

    assert result.returncode == 0, result.stderr
    assert "Installed heyfood 0.2.0" in result.stdout
    assert log.read_text(encoding="utf-8").splitlines()[-1] == "heyfood-cli"


def test_rejects_version_injection_before_invoking_pipx(tmp_path: Path) -> None:
    marker = tmp_path / "must-not-exist"
    result, log = _run(
        tmp_path,
        HEYFOOD_VERSION=f"0.2.0;touch {marker}",
    )

    assert result.returncode != 0
    assert "must be an exact release" in result.stderr
    assert not log.exists()
    assert not marker.exists()


def test_rejects_relative_install_directories(tmp_path: Path) -> None:
    result, log = _run(tmp_path, PIPX_BIN_DIR="relative/bin")

    assert result.returncode != 0
    assert "PIPX_BIN_DIR must be an absolute path" in result.stderr
    assert not log.exists()


def test_fails_when_the_installed_command_does_not_verify(tmp_path: Path) -> None:
    result, _ = _run(
        tmp_path,
        HEYFOOD_TEST_INSTALLED_VERSION="not-a-release",
    )

    assert result.returncode != 0
    assert "installed command returned an unexpected version" in result.stderr


def test_rejects_an_unsupported_selected_python(tmp_path: Path) -> None:
    env, log = _environment(tmp_path)
    unsupported = tmp_path / "unsupported-python"
    _write_executable(unsupported, "#!/usr/bin/env bash\nexit 1\n")
    env["HEYFOOD_PYTHON"] = str(unsupported)

    result = subprocess.run(
        ["/bin/bash", str(INSTALLER)],
        cwd=tmp_path,
        env=env,
        check=False,
        capture_output=True,
        text=True,
    )

    assert result.returncode != 0
    assert "selected interpreter must be Python 3.11, 3.12, or 3.13" in result.stderr
    assert not log.exists()


def test_bootstraps_pipx_in_isolated_user_data_without_network(tmp_path: Path) -> None:
    home = tmp_path / "home"
    fake_bin = tmp_path / "fake-bin"
    data_home = tmp_path / "data"
    pipx_bin = tmp_path / "pipx-bin"
    pipx_home = tmp_path / "pipx-home"
    log = tmp_path / "pipx-arguments.txt"
    home.mkdir()
    fake_bin.mkdir()
    fake_python = fake_bin / "python3.11"
    _write_executable(
        fake_python,
        r"""#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" == "-c" ]]; then
  if [[ "${2:-}" == *"platform.python_version"* ]]; then
    printf '3.11.9\n'
  fi
  exit 0
fi

if [[ "${1:-}" == "-m" && "${2:-}" == "venv" ]]; then
  mkdir -p "$3/bin"
  cp "$0" "$3/bin/python"
  chmod 755 "$3/bin/python"
  exit 0
fi

if [[ "${1:-}" == "-m" && "${2:-}" == "pip" ]]; then
  touch "$(dirname "$0")/pipx-ready"
  exit 0
fi

if [[ "${1:-}" == "-m" && "${2:-}" == "pipx" ]]; then
  shift 2
  if [[ "${1:-}" == "--version" ]]; then
    [[ -f "$(dirname "$0")/pipx-ready" ]] || exit 1
    printf '1.7.1\n'
    exit 0
  fi
  [[ "${1:-}" == "install" ]] || exit 64
  printf '%s\n' "$@" > "$HEYFOOD_TEST_LOG"
  mkdir -p "$PIPX_BIN_DIR"
  printf '%s\n' \
    '#!/usr/bin/env bash' \
    'printf '\''heyfood 0.2.0\n'\''' \
    > "$PIPX_BIN_DIR/heyfood"
  chmod 755 "$PIPX_BIN_DIR/heyfood"
  exit 0
fi

exit 64
""",
    )

    env = os.environ.copy()
    env.update(
        {
            "HOME": str(home),
            "HEYFOOD_PYTHON": str(fake_python),
            "HEYFOOD_TEST_LOG": str(log),
            "PATH": f"{fake_bin}{os.pathsep}/usr/bin:/bin",
            "PIPX_BIN_DIR": str(pipx_bin),
            "PIPX_HOME": str(pipx_home),
            "XDG_DATA_HOME": str(data_home),
        }
    )
    result = subprocess.run(
        ["/bin/bash", str(INSTALLER)],
        cwd=tmp_path,
        env=env,
        check=False,
        capture_output=True,
        text=True,
    )

    bootstrap_python = data_home / "heyfood" / "installer" / "pipx" / "bin" / "python"
    assert result.returncode == 0, result.stderr
    assert bootstrap_python.is_file()
    assert "Preparing an isolated pipx bootstrap" in result.stdout
    assert "Installing the isolated pipx bootstrap from PyPI." in result.stdout
    assert log.read_text(encoding="utf-8").splitlines()[-1] == "heyfood-cli"
