#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: agent-setup-smoke.sh /absolute/path/to/heyfood /absolute/staging-root" >&2
  exit 64
fi

binary=$1
root=$2
case "$binary:$root" in
  /*:/*) ;;
  *)
    echo "agent setup smoke paths must be absolute" >&2
    exit 64
    ;;
esac
binary="$(cd "$(dirname "$binary")" && pwd -P)/$(basename "$binary")"
mkdir -p -- "$root"
root=$(cd "$root" && pwd -P)

home="$root/home"
state="$root/state"
host_bin="$root/host-bin"
mkdir -p "$home" "$state" "$host_bin"
chmod 700 "$home" "$state" "$host_bin"

make_host() {
  local host=$1
  local version=$2
  local path="$host_bin/$host"
  {
    printf '#!/usr/bin/env bash\n'
    printf 'set -euo pipefail\n'
    printf 'host=%q\n' "$host"
    printf 'version=%q\n' "$version"
    cat <<'HOST'
state="$HOME/.${host}-heyfood-mcp"
if [[ "${1:-}" == "--version" ]]; then
  printf '%s\n' "$version"
  exit 0
fi
if [[ "${1:-}" == "mcp" && "${2:-}" == "get" ]]; then
  if [[ ! -f "$state" ]]; then
    echo "Error: No MCP server named 'heyfood' found." >&2
    exit 1
  fi
  command_path=$(<"$state")
  if [[ "$host" == "codex" ]]; then
    jq -n --arg command "$command_path" '{
      name: "heyfood",
      transport: {
        type: "stdio",
        command: $command,
        args: ["mcp", "serve"],
        env: null,
        env_vars: [],
        cwd: null
      }
    }'
  else
    printf 'heyfood:\n'
    printf '  Scope: User config\n'
    printf '  Type: stdio\n'
    printf '  Command: %s\n' "$command_path"
    printf '  Args: mcp serve\n'
    printf '  Environment:\n'
  fi
  exit 0
fi
if [[ "${1:-}" == "mcp" && "${2:-}" == "add" ]]; then
  if [[ "$host" == "codex" ]]; then
    command_path=${5:?missing Codex MCP command}
  else
    command_path=${9:?missing Claude MCP command}
  fi
  printf '%s\n' "$command_path" >"$state"
  chmod 600 "$state"
  exit 0
fi
if [[ "${1:-}" == "mcp" && "${2:-}" == "remove" ]]; then
  rm -f "$state"
  exit 0
fi
exit 2
HOST
  } >"$path"
  chmod 700 "$path"
}

make_host codex "codex-cli 0.145.0-alpha.18"
make_host claude "2.1.128 (Claude Code)"

export HOME="$home"
export USERPROFILE="$home"
export XDG_CONFIG_HOME="$home/.config"
export XDG_DATA_HOME="$home/.local/share"
export XDG_CACHE_HOME="$home/.cache"
export APPDATA="$home/AppData/Roaming"
export LOCALAPPDATA="$home/AppData/Local"
export HEYFOOD_STATE_DIR="$state"
export PATH="$host_bin:$PATH"

schema_index=$("$binary" agent schema --list)
expected_setup_schema=$(jq -er \
  '[.schemas[].name
    | if . == "setup-plan" then 1
      elif test("^setup-plan-v[0-9]+$") then
        capture("^setup-plan-v(?<version>[0-9]+)$").version | tonumber
      else empty
      end]
   | if length == 0 then error("no setup-plan schemas") else max end' \
  <<<"$schema_index")

dry_run=$(
  "$binary" --json agent setup \
    --target all \
    --scope user \
    --dry-run
)
jq -e --argjson expected_schema "$expected_setup_schema" '
  .schema_version == $expected_schema
  and .ready == true
  and .changed == false
  and (.plan_sha256 | test("^[0-9a-f]{64}$"))
  and ([.hosts[].action] == ["install", "install"])
  and ([.hosts[].mcp.action] == ["install", "install"])
  and ([.hosts[].mcp.environment] | all(length == 0))
  and ([.hosts[].mcp.arguments] | all(. == ["mcp", "serve"]))
' <<<"$dry_run" >/dev/null
plan_sha256=$(jq -er '.plan_sha256' <<<"$dry_run")

applied=$(
  "$binary" --json agent setup \
    --target all \
    --scope user \
    --apply \
    --plan-sha256 "$plan_sha256"
)
jq -e --argjson expected_schema "$expected_setup_schema" \
  '.schema_version == $expected_schema and .ready == true and .changed == true' \
  <<<"$applied" >/dev/null
test -f "$home/.agents/skills/heyfood/SKILL.md"
test -f "$home/.claude/skills/heyfood/SKILL.md"
test -f "$home/.codex-heyfood-mcp"
test -f "$home/.claude-heyfood-mcp"
test "$(cat "$home/.codex-heyfood-mcp")" = "$binary"
test "$(cat "$home/.claude-heyfood-mcp")" = "$binary"

unchanged=$(
  "$binary" --json agent setup \
    --target all \
    --scope user \
    --dry-run
)
jq -e --argjson expected_schema "$expected_setup_schema" '
  .schema_version == $expected_schema
  and .ready == true
  and .changed == false
  and ([.hosts[].action] == ["none", "none"])
  and ([.hosts[].mcp.action] == ["none", "none"])
' <<<"$unchanged" >/dev/null

uninstall=$(
  "$binary" --json agent uninstall \
    --target all \
    --scope user \
    --dry-run
)
jq -e --argjson expected_schema "$expected_setup_schema" '
  .schema_version == $expected_schema
  and .ready == true
  and ([.hosts[].action] == ["uninstall", "uninstall"])
  and ([.hosts[].mcp.action] == ["uninstall", "uninstall"])
' <<<"$uninstall" >/dev/null
uninstall_sha256=$(jq -er '.plan_sha256' <<<"$uninstall")
"$binary" --json agent uninstall \
  --target all \
  --scope user \
  --apply \
  --plan-sha256 "$uninstall_sha256" >/dev/null

test ! -e "$home/.agents/skills/heyfood"
test ! -e "$home/.claude/skills/heyfood"
test ! -e "$home/.codex-heyfood-mcp"
test ! -e "$home/.claude-heyfood-mcp"
if find "$state/receipts" -type f -print -quit 2>/dev/null | grep -q .; then
  echo "receipt-bound uninstall left a receipt behind" >&2
  exit 1
fi

printf '%s\n' \
  "Agent setup installed, verified, repeated, and uninstalled Codex + Claude skill/MCP state."
