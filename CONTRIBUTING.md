# Contributing to Deskpass

Thanks for helping. Deskpass lets an AI operate a Windows desktop, so changes are held to a careful standard; this page is the short version of how to build, test and submit one. Design background is in the [development guide](docs/development.md), the terms in [CONTEXT.md](CONTEXT.md), and the decisions behind them in [docs/adr](docs/adr/).

## Security issues

Do not open a public issue or pull request for a suspected vulnerability. Report it privately as described in [SECURITY.md](SECURITY.md).

## Prerequisites

Deskpass is a Node.js/TypeScript server (`src/`) plus a Rust helper (`helper/`), and it only builds and runs on Windows (10 2004 or newer, or 11).

- Node.js 22 or newer (`node -v`). npm comes with it.
- Rust, stable channel, with the MSVC toolchain (`rustup default stable-x86_64-pc-windows-msvc`).
- The Visual Studio C++ build tools (the "Desktop development with C++" workload, which includes the MSVC compiler and a Windows SDK). Rust's MSVC target needs them to link.

## Build

```powershell
npm install
npm run build
```

`npm run build` builds the helper (`cargo build --release`), compiles the TypeScript, and copies the helper into `dist/`. To try your build in an MCP client, register `node "<path>\dist\index.js"` as a stdio server; the [development guide](docs/development.md#registering-with-claude-code-for-development) has the command.

To produce the release zip:

```powershell
npm run build
npm run package
```

`npm run package` writes `release/deskpass-<version>.zip` from the `dist/` you just built. It refuses to package a helper that is missing, does not match `dist/helper-build.json`, or is older than the sources in `helper/`.

That zip is for trying a build. A published release is built by the Release workflow instead: set the version in `package.json`, `helper/Cargo.toml` (and `Cargo.lock`), `src/config.ts` and `CHANGELOG.md`, merge, then push the tag `v<version>`. The workflow builds, tests and packages on GitHub's runner and opens a draft release with the zip, its SHA-256 and a build provenance attestation; add the CHANGELOG section to the notes and publish it.

## Test

```powershell
npm test               # TypeScript tests (node --test), against a fake helper
npm run test:rust      # same as: cargo test --manifest-path helper/Cargo.toml
```

Run them one after the other, not at the same time.

Two kinds of tests exist, and they differ in what they need:

- **No desktop needed.** The `npm test` suites that talk to a fake helper, the settings-page and installer tests, the string-table checks, and the Rust unit tests (`cargo test --bins` in `helper/`). These are what CI runs.
- **An interactive desktop needed.** The Rust integration tests in `helper/tests/` and the end-to-end tests in `test/e2e.test.ts` open real windows (titles starting with `CUMCP_`, dialogs, overlays) and send real input to them. Run them on a signed-in, unlocked Windows desktop and leave the mouse and keyboard alone while they run. The Rust desktop tests skip themselves when there is no interactive desktop, and `DESKPASS_NO_DESKTOP=1` forces the skip for them and for the end-to-end tests.

CI (`.github/workflows/ci.yml`, windows-latest) runs: `npm ci`, `npm run build:ts`, `cargo test --no-run` (so every desktop test must still compile), `cargo test --bins`, and `npm test` with `DESKPASS_NO_DESKTOP=1`. Run the same set before you open a pull request, and run the desktop tests locally when you touch the helper, input, overlay, dialogs or the stop keys. To reproduce CI for the npm tests:

```powershell
$env:DESKPASS_NO_DESKTOP = "1"
npm test
```

`npm test` points `LOCALAPPDATA` at a temporary folder, so it does not touch your real Deskpass data. If you run one test file directly with `node --test`, add `--import ./test/isolate-localappdata.mjs`. Tests use `DESKPASS_LANG` to pin the interface language; do the same in a new test that reads UI text.

A change to behavior comes with a test that fails without it.

## Keep the languages and docs in step

Deskpass has two interface languages, English and Traditional Chinese (zh-TW), and two README files.

- **READMEs.** `README.md` (English) and `README.zh-TW.md` (Traditional Chinese) say the same thing; when you change one, change the other in the same pull request. The Traditional Chinese user manual is [docs/user-guide.zh-TW.md](docs/user-guide.zh-TW.md); update it when user-visible behavior changes. English docs (this file, the development guide, CONTEXT, the ADRs, SECURITY, CHANGELOG) contain no Chinese except quoted UI strings.
- **Every new UI string gets both languages.** There is one table per language, with the same keys, in each place that shows text to the user:
  - the settings page: `SETTINGS_TEXT` (zh-TW) and `SETTINGS_TEXT_EN` in `src/settings-strings.ts` (this includes the log page's sentences and the built-in risk tiers' names and reasons);
  - the other server-side text shown to the user, such as the "continue?" question in `src/server.ts` (`CONTINUE_MESSAGES`);
  - the helper's dialogs, border label and corner notice: `EN` and `ZH_TW` in `helper/src/strings.rs`;
  - the installer and uninstaller messages: the `$Messages` tables (`en` and `zh-TW`) at the top of `packaging/install.ps1` and `packaging/uninstall.ps1`.
- Tests enforce this for the settings tables (`test/settings-strings.test.ts`), the helper tables (`helper/src/strings.rs`) and the installer messages (`test/install-script.test.ts`): each pair must have the same keys, and the English table must contain no CJK characters. A string missing in one language fails `npm test` or `cargo test`.
- The language rule (`DESKPASS_LANG`, then the Windows display language) lives in `src/ui-language.ts` and is repeated in the two PowerShell scripts; change them together.
- Tool descriptions and error codes are for the model and stay in English; they are not translated.
- Add a line to [CHANGELOG.md](CHANGELOG.md) for anything a user would notice.

## Commits and pull requests

- Open a pull request against `master` from a topic branch. Keep it to one purpose, with a description that says what changed and why, and how you tested it (which of the test sets above you ran, and whether you ran the desktop tests).
- Commit messages: a short imperative summary line (a prefix such as `feat:`, `fix:` or `docs:` is welcome); the body explains the reason when it is not obvious.
- CI must pass. New code follows the style of its neighbors: TypeScript with `tsc` clean, Rust formatted like the surrounding code, comments in English.
- Changes to approval, risk tiers, the grants file, the stop keys or what is written to the log are security-relevant: say so in the pull request and expect a closer review.

By contributing you agree that your contribution is licensed under the project's [MIT license](LICENSE).
