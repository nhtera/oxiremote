# oxiremote (npm wrapper)

Thin wrapper around the [OxiRemote](https://github.com/nhtera/oxiremote) Rust binary. Installing this package downloads the right prebuilt binary for your platform from the corresponding GitHub release and exposes it as `oxiremote` on your `$PATH`.

```bash
npm install -g oxiremote
oxiremote
```

The wrapper exists so users with `npm` in their muscle memory get the same one-command install as `curl ... | sh`. The wrapper version always tracks the upstream release version 1:1.

## Environment variables

- `OXIREMOTE_BINARY_URL` — override the base URL the postinstall fetches from (corp proxies, mirrors, air-gapped installs).
- `OXIREMOTE_SKIP_INSTALL=1` — skip the postinstall download (useful in CI lanes where you provide the binary out of band).

## How it works

1. `npm install` triggers `scripts/install.js`.
2. The script detects `process.platform`/`arch`, picks the matching Rust target triple, downloads `oxiremote-<version>-<target>.tar.gz` (or `.zip` on Windows) plus `oxiremote-<version>-sha256.txt` from the GitHub release.
3. SHA256 is verified, the archive is extracted into `bin/`, and the executable bit is set.
4. `bin/oxiremote.js` is a tiny shim that `spawn`s the platform binary with inherited stdio.

The same checksum manifest is verified by `oxiremote update`, so the wrapper and the self-update path share one trust boundary.

## License

MIT — see the upstream repository.
