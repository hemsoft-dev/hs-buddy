#!/usr/bin/env bash
# Bounded apt-get for CI runners and the native runtime container. A slow or
# unreachable mirror fails the step with a clear message instead of using up
# the job's time limit. apt keeps finished downloads in its archive cache, so
# a second attempt continues where the first attempt's limit cut it short.
# Sourcing declares the functions only.

# Usage: buddy_apt [sudo] update|install ARGUMENTS...
buddy_apt() {
  local privilege=()
  if [[ "${1:-}" == sudo ]]; then
    privilege=(sudo)
    shift
  fi
  local step="$1"
  shift
  local limit=600
  if [[ "$step" == update ]]; then limit=300; fi
  local attempt status=0
  for attempt in 1 2; do
    if ((attempt > 1)) && [[ "$step" == install ]]; then
      # A limit that stopped dpkg mid-unpack leaves it interrupted; finish that first.
      ${privilege[@]+"${privilege[@]}"} dpkg --configure -a || true
    fi
    # The `+` form expands an empty array under `set -u` in bash 3.2 (macOS) too.
    ${privilege[@]+"${privilege[@]}"} timeout --kill-after=30 "$limit" apt-get \
      -o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30 \
      "$step" "$@" && return 0 || status=$?
  done
  local reason="exit $status"
  if ((status == 124 || status == 137)); then reason="no result within ${limit}s"; fi
  local annotation=''
  if [[ "${GITHUB_ACTIONS:-}" == true ]]; then annotation='::error title=apt-get failed::'; fi
  printf '%sapt-get %s failed after 2 attempts (%s). Step: apt-get %s. Mirrors: %s\n' \
    "$annotation" "$step" "$reason" "$step${*:+ $*}" "$(buddy_apt_mirrors)" >&2
  return 1
}

buddy_apt_mirrors() {
  local sites
  # \$(SITE) is apt's format placeholder, not a shell expansion.
  sites="$(apt-get indextargets --no-release-info --format "\$(SITE)" 2>/dev/null | sort -u | paste -sd ' ' -)"
  printf '%s' "${sites:-unknown}"
}
