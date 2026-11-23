# Development

## Testing against real servers (macOS)

```sh
scripts/backend-test.sh NAME
```

`NAME` is `ollama`, `llamacpp` or `lmstudio`. The script installs the backend with Homebrew or `lms`, starts it and loads a small Qwen3 0.6B model. It then runs the ignored live tests against it, and stops whatever it started.

Downloads stay cached. `scripts/backends/NAME.sh purge` removes them.

These scripts work only on macOS.

## Releases

A merged PR releases only when it is labelled:

| Label | Release |
| --- | --- |
| `release:patch` | Patch |
| `release:minor` | Minor |
| `release:major` | Major |
| `release:skip` | None at merge (the default when unlabelled) |

A weekly run releases a patch if anything merged since the last release, labelled or not. It can also be started by hand from the Actions tab.

Leave most PRs unlabelled. `release:minor` is only for a big rollout the maintainer announces. `release:major` waits for 1.0.

A PR that adds a session store migration always gets `release:patch`, with the migration at `main`'s version without `-dev`. Migrations count a dev build as its release, so each format change needs a release of its own.

On release, `.github/workflows/release.yml` commits the release version to `Cargo.toml` and `Cargo.lock` and tags that commit. It then commits the next `-dev` version (the release's patch + 1) to `main`.

Never edit the version by hand. `main` always holds the next version with `-dev`, and each tag's `Cargo.toml` holds that tag's version.

The workflow pushes with the release GitHub App's token (`RELEASE_APP_CLIENT_ID`, `RELEASE_APP_PRIVATE_KEY`). The app is on the bypass list of `main`'s ruleset.
