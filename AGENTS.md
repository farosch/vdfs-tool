# Agent Guide: vdfs-tool

This directory is the Rust implementation of `vdfs-tool`, a command-line utility
for inspecting, unpacking, and packing Samsung VDFS4 and VDFS1002 filesystem
images.

## Working in this repository

- Keep changes focused and preserve existing command-line and on-disk behavior.
- Follow the module and error-handling conventions in
  [CONVENTIONS.md](CONVENTIONS.md).
- Put format-neutral code in `src/`; keep VDFS4 code under `src/vdfs4/` and
  VDFS1002 code under `src/vdfs1002/`.
- Treat image data as untrusted input. Return `anyhow::Result`, check bounds,
  and avoid panics or unchecked arithmetic.
- Use existing codecs and shared helpers rather than duplicating format logic.
- Preserve existing test fixtures and synthetic fixture conventions.

## Tests and validation

Every code or behavior change needs regression coverage. Add unit tests beside
the implementation and integration tests under `tests/` when exercising the
binary or real fixture workflow.

Before finishing:

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets
```

Cross-platform build changes must keep Unix-only dependencies and APIs behind
`cfg(unix)` or target-specific Cargo dependencies.

Always pass `--locked`. Use `cargo fmt` to apply formatting, and keep new code
free of Clippy warnings. Update the test count in [DEVELOP.md](DEVELOP.md)
when the unit-test count changes.

## Documentation

Update all directly relevant documentation in the same change:

- [README.md](README.md) for user-facing commands and format support.
- [DEVELOP.md](DEVELOP.md) for build, test, fixture, and release instructions.
- [TODO.md](TODO.md) for unresolved implementation or real-format validation
  leads.
- [CONVENTIONS.md](CONVENTIONS.md) only when repository conventions change.

Do not claim a format behavior or device authorization result without a
reproducible fixture or controlled test.

## Branch and pull-request workflow

- Never commit directly to `main` or `develop`.
- Create a feature branch from `develop`.
- Submit all repository changes to `develop` through a pull request.
- PRs targeting `develop` must come from a feature branch.
- Do not open, merge, or rebase pull requests unless the user explicitly asks
  or approves that operation.
- Merge `develop` into `main` only manually and only after CI passes.
- PRs targeting `main` must come from `develop`.
- A push to `main` triggers the automatic release pipeline. Releases use the
  next patch version after the highest `vMAJOR.MINOR.PATCH` tag.

## Security and format research

Unpackers and packers handle untrusted image structures. Preserve negative
results, malformed-input regressions, and codec boundaries.

For authentication or authorization findings:

- State the trust assumption being tested.
- Keep synthetic identities and keys separate from operator credentials.
- Reproduce findings in synthetic fixtures or an explicitly authorized lab
  target.
- Do not describe host-side parsing success as device-side authorization.
- Document confirmed, disproven, and inconclusive results in tests and
  [TODO.md](TODO.md).
