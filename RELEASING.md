# Releasing

A pushed `vX.Y.Z` tag runs `.github/workflows/release.yml`. It runs the tests,
then GoReleaser builds rvw for Linux and macOS (amd64 and arm64) and publishes
a GitHub release. The release has the archives, `checksums.txt`, `install.sh`,
and a changelog grouped from conventional commits.

1. Pick the version. rvw is pre-1.0: a `feat!:` since the last tag bumps the
   minor version, anything else bumps the patch version.
2. Set `version` in `.claude-plugin/plugin.json` to the new version, without
   the `v`. The release fails when the tag and this file disagree.
3. Commit it on `main`: `chore: release vX.Y.Z`.
4. Tag and push:

   ```sh
   git tag -a vX.Y.Z -m vX.Y.Z
   git push origin main vX.Y.Z
   ```

5. Check the release page. Then install it with the script:

   ```sh
   curl -fsSL https://github.com/gustavofsantos/rvw/releases/latest/download/install.sh | sh
   ```

A tag with a suffix, such as `v0.2.0-rc.1`, is published as a prerelease.
`releases/latest` skips it, so install it with `install.sh -v v0.2.0-rc.1`.
The plugin check compares the tag without its suffix.

To try the release config without publishing:

```sh
goreleaser release --snapshot --clean   # output in dist/
```
