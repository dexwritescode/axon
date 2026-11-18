#!/usr/bin/env bash
# After a release of $VERSION, open a PR that sets the workspace version to
# the next patch with -dev. The patch, not the minor: any label's release then
# sorts at or above main's version.
set -euo pipefail
: "${VERSION:?}"

IFS=. read -r major minor patch <<<"$VERSION"
next="$major.$minor.$((patch + 1))-dev"
branch="next-dev/$next"

perl -pi -e "s/^version = \".*\"\$/version = \"$next\"/" Cargo.toml
cargo update --workspace

git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
git switch -c "$branch"
git commit -m "Version $next" Cargo.toml Cargo.lock
git push origin "$branch"
gh pr create --head "$branch" --label release:skip \
    --title "Version $next" --body "Main moves to $next after the v$VERSION release."
