#!/usr/bin/env python3
"""Exercise the pipx-installed heyfood executable from a clean config home."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile


def _run(binary: Path, *args: str, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(binary), *args],
        cwd=env["XDG_CONFIG_HOME"],
        env=env,
        capture_output=True,
        text=True,
        timeout=20,
        check=False,
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    parser.add_argument("version")
    args = parser.parse_args()
    if not args.binary.is_file():
        raise SystemExit(f"heyfood executable not found: {args.binary}")

    with tempfile.TemporaryDirectory(prefix="heyfood-smoke-") as config_home:
        env = dict(os.environ)
        env.update(
            {
                "CI": "1",
                "NO_COLOR": "1",
                "TERM": "dumb",
                "XDG_CONFIG_HOME": config_home,
            }
        )

        version = _run(args.binary, "--version", env=env)
        assert version.returncode == 0, version.stderr
        assert version.stdout.strip() == f"heyfood {args.version}"

        help_result = _run(args.binary, "--help", env=env)
        assert help_result.returncode == 0, help_result.stderr
        assert "Usage: heyfood" in help_result.stdout
        assert "--no-banner" in help_result.stdout

        login_help = _run(args.binary, "login", "--help", env=env)
        assert login_help.returncode == 0, login_help.stderr
        assert "--device" in login_help.stdout

        members_help = _run(args.binary, "members", "--help", env=env)
        assert members_help.returncode == 0, members_help.stderr
        assert "list" in members_help.stdout

        conversation_help = _run(args.binary, "conversation", "--help", env=env)
        assert conversation_help.returncode == 0, conversation_help.stderr
        assert "resume" in conversation_help.stdout
        assert "clear" in conversation_help.stdout

        for shell, marker in {
            "zsh": "compdef",
            "bash": "complete",
            "fish": "complete --command",
        }.items():
            completion_env = dict(env)
            completion_env["_HEYFOOD_COMPLETE"] = f"source_{shell}"
            completion = _run(args.binary, env=completion_env)
            assert completion.returncode == 0, completion.stderr
            assert marker in completion.stdout

        doctor = _run(args.binary, "doctor", "--json", env=env)
        assert doctor.returncode == 1, doctor.stderr
        assert "\x1b" not in doctor.stdout
        document = json.loads(doctor.stdout)
        assert document["ok"] is False
        assert document["checks"]["session"]["type"] == "login_required"
        assert document["checks"]["channel"]["type"] == "login_required"

        config_path = Path(config_home) / "heyfood" / "config.json"
        assert not config_path.exists(), "read-only smoke commands created configuration"

    print(f"pipx-installed heyfood {args.version} smoke test passed")


if __name__ == "__main__":
    main()
