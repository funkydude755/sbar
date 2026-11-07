# Releasing sbar

A Git tag names an exact code snapshot. A GitHub release adds notes and downloads to that tag. A public beta is marked as a prerelease on GitHub; the version number alone does not make that designation.

## Prepare the version

1. Make the fixes on a branch. Update the version in `Cargo.toml` and the sbar entry in `Cargo.lock`, add a dated changelog entry, and update the README installation tag.
2. Run the checks in [CONTRIBUTING.md](CONTRIBUTING.md). Review dependency notices if the lockfile's dependencies changed.
3. Open a pull request with the local validation results, including the operating system and architecture tested, then review and merge it.
4. Confirm the final `main` tree matches the locally tested code before tagging it. If code changed during review or merge, repeat the affected local checks. This repository does not run GitHub CI.

## Prepare the release

From a clean checkout of that tested commit, run `cargo package --locked` and inspect `cargo package --list --locked`. Confirm that no credentials or private files are included.

Choose a new version tag, such as `v0.2.4`. Create an annotated tag at the tested commit and push it. Never move an existing release tag to new code.

On GitHub, open **Releases → Draft a new release**, select the tag, and describe the changes, supported platforms, known limitations, and source-installation steps. For a beta, select **Set as a pre-release**. Keep the draft unpublished until local validation is complete.

GitHub provides source archives for tags. The public [Homebrew tap](https://github.com/funkydude755/homebrew-tap) distributes stable releases and their bottles. Source-only beta releases do not update that stable formula. For a stable release, also update and test the tap formula and rebuild its bottles before advertising the new Homebrew version. Do not advertise crates.io installation unless a package has actually been published there.

## Publish and verify

1. Confirm the tag points to the intended, locally validated commit.
2. Publish the draft release.
3. Verify the repository, release, and source archive without authentication.
4. Build and install from a fresh copy of the released tag, preferably into a temporary prefix. Check `sbar --version`, `sbar --once`, interactive startup, and exit.

If this is the first public release, review the entire Git history, existing issues, pull requests, releases, and workflow logs before changing repository visibility. Confirm the owner's intent to publish under the included license. Making a repository public exposes its history as well as its current files; downloaded copies and public forks can persist.

Enable private vulnerability reporting and protect `main` from force pushes and deletion. Keep the installation instructions on older releases clearly marked if they refer to an unavailable channel.
