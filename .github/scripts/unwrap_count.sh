#!/usr/bin/env bash
# Report-only (#320): clippy::unwrap_used outside test code against the count
# in .github/unwrap-count. Without --all-targets, clippy builds no test target
# and no #[cfg(test)] module.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
spans=$(cargo clippy --workspace --locked --message-format=json -- -W clippy::unwrap_used |
  jq -r 'select(.reason == "compiler-message" and .message.code.code == "clippy::unwrap_used")
    | .message.spans[0] | "\(.file_name):\(.line_start):\(.column_start)"' | sort -u)
[ -n "$spans" ] && printf '%s\n' "$spans"
n=$(printf '%s' "$spans" | grep -c . || true)
base=$(cat .github/unwrap-count)
echo "unwraps outside tests: $n (checked in: $base)"
if [ "$n" -gt "$base" ]; then
  echo "::warning::unwraps outside tests went up from $base to $n"
fi
