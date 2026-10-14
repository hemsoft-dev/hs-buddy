#!/usr/bin/env bash
# Start the Electron app with its Convex backend on macOS.
set -euo pipefail

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  printf '%s\n' \
    'Usage: bash ./run-mac.sh' \
    'Checks prerequisites, installs dependencies, and starts Convex and Electron.' \
    'Choose local development when Convex prompts on first launch.' \
    'Keep this terminal open. Press Ctrl+C to stop.'
  exit 0
fi
if [[ $# -ne 0 ]]; then
  printf 'Usage: bash ./run-mac.sh\n' >&2
  exit 1
fi
if [[ "$(uname -s)" != "Darwin" ]]; then
  printf 'run-mac.sh requires macOS.\n' >&2
  exit 1
fi

cd "$(dirname "${BASH_SOURCE[0]}")"

# Find tools installed in the usual Apple silicon and Intel Homebrew locations.
for dir in "$HOME/.bun/bin" /opt/homebrew/bin /usr/local/bin; do
  if [[ -d "$dir" && ":$PATH:" != *":$dir:"* ]]; then
    export PATH="$PATH:$dir"
  fi
done
if ! command -v node >/dev/null 2>&1 && command -v brew >/dev/null 2>&1; then
  node_prefix="$(brew --prefix node@24 2>/dev/null || true)"
  if [[ -x "$node_prefix/bin/node" ]]; then
    export PATH="$node_prefix/bin:$PATH"
  fi
fi

missing=()
command -v node >/dev/null 2>&1 || missing+=('Node.js: brew install node@24')
command -v bun >/dev/null 2>&1 || missing+=('Bun: https://bun.sh/docs/installation')
command -v gh >/dev/null 2>&1 || missing+=('GitHub CLI: brew install gh')
if ! xcode-select -p >/dev/null 2>&1; then
  missing+=('Xcode Command Line Tools: xcode-select --install')
fi
if [[ -n "${missing[*]-}" ]]; then
  printf 'Install these prerequisites, then run this script again:\n' >&2
  printf '  %s\n' "${missing[@]}" >&2
  exit 1
fi

if ! node --input-type=module -e '
  import { readFileSync } from "node:fs";
  const { engines } = JSON.parse(readFileSync("package.json", "utf8"));
  const minimum = engines.node.replace(/^>=/, "").split(".").map(Number);
  const current = process.versions.node.split(".").map(Number);
  const difference = current.map((part, index) => part - minimum[index]).find(part => part !== 0);
  if (difference < 0) {
    console.error(`Node.js ${engines.node} is required; found ${process.versions.node}.`);
    process.exit(1);
  }
'; then
  printf '%s\n' \
    'Install Node.js 24: brew install node@24' \
    'Then run: export PATH="$(brew --prefix node@24)/bin:$PATH"' >&2
  exit 1
fi

printf 'Installing dependencies from bun.lock...\n'
bun install --frozen-lockfile

printf 'Starting Convex and Electron. Choose local development if prompted.\n'
printf 'Keep this terminal open. Press Ctrl+C to stop.\n'
exec bun x --no-install convex dev --start 'bun run dev'
