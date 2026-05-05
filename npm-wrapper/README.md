# oxiremote (npm wrapper)

Thin wrapper around the [OxiRemote](https://github.com/nhtera/oxiremote) Rust binary. Installing this package downloads the right prebuilt binary for your platform from the corresponding GitHub release and exposes it as `oxiremote` on your `$PATH`.

```bash
npm install -g oxiremote
oxiremote
```

The wrapper exists so users with `npm` in their muscle memory get the same one-command install as `curl ... | sh`. The wrapper version always tracks the upstream release version 1:1.

## Environment variables

- `OXIREMOTE_BINARY_URL` — override the base URL the postinstall fetches from (corp proxies, mirrors, air-gapped installs).
- `OXIREMOTE_GITHUB_REPO` — override the source repo (default `nhtera/oxiremote`).
- `OXIREMOTE_VERSION` — pin a specific binary version (otherwise tracks `package.json`).
- `OXIREMOTE_DISABLE_INSTALL=1` (alias `OXIREMOTE_SKIP_INSTALL=1`) — skip postinstall entirely.
- `OXIREMOTE_FORCE_DOWNLOAD=1` — re-download even if the version marker matches.
- `OXIREMOTE_OPTIONAL_INSTALL=1` — exit 0 on failure so a bad release doesn't break unrelated `npm install` runs.
- `OXIREMOTE_SKIP_GLIBC_CHECK=1` — bypass the Linux glibc compatibility check (only meaningful when you know what you're doing).

## How it works

1. `npm install` triggers `scripts/install.js`.
2. The script detects `process.platform`/`arch`, picks the matching Rust target triple, and (unless `bin/oxiremote.version` already matches) downloads `oxiremote-<version>-<target>.tar.gz` (or `.zip` on Windows) plus `oxiremote-<version>-sha256.txt` from the GitHub release.
3. SHA256 is verified, the archive is extracted into `bin/`, a Linux glibc compatibility check runs, and the executable bit is set. The version marker is written so re-installing the same version skips the network entirely.
4. `bin/oxiremote.js` is a tiny shim that `spawn`s the platform binary with inherited stdio.

A `prepublishOnly` lifecycle hook (`scripts/verify-release-assets.js`) HEAD-checks every release URL + verifies the SHA256 manifest names every asset before npm allows publish — so a tag without a matching release can never be published from a clean checkout.

The same checksum manifest is verified by `oxiremote update`, so the wrapper and the self-update path share one trust boundary.

## License

MIT — see the upstream repository.
