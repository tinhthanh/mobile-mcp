# Contributing to mobile-mcp-lite

Thanks for your interest in contributing! This document covers the basics for getting set up and submitting changes.

## Code of Conduct

This project adheres to the [Contributor Covenant](CODE_OF_CONDUCT.md). By participating, you agree to keep the community welcoming and respectful.

## Reporting bugs and requesting features

- Search [existing issues](https://github.com/tinhthanh/mobile-mcp/issues) before opening a new one.
- Open one issue per bug or feature request.
- For bugs, include: mobile-mcp-lite version, phone model and Android version, whether the UiAutomator2 server is installed, the tool call and its output (trim personal data), and the actual vs. expected behavior.
- For larger feature proposals, please open an issue to discuss the design before sending a PR.

## Reporting security issues

Please **do not** file public GitHub issues for security vulnerabilities — see [SECURITY.md](SECURITY.md) for the private reporting process.

## Development setup

Requirements: Node.js 22+, python3, Android Platform Tools and a USB-connected Android phone for end-to-end checks. See [docs/SETUP.md](docs/SETUP.md).

```bash
git clone https://github.com/tinhthanh/mobile-mcp.git
cd mobile-mcp
npm ci --ignore-scripts
npm run build

# lint
npm run lint

# unit tests (no device needed; Playwright is only the runner)
npm test

# end-to-end on a connected phone
scripts/run-test.sh "Mở Cài đặt, vào Display: Dark mode đang On hay Off?"
```

`npm run watch` recompiles on change during development. A pre-commit hook (husky) runs `npm run lint` automatically.

## Submitting changes

- Branch off `main` and open a pull request when ready.
- Keep PRs focused — one concern per PR.
- Run `npm run build`, `npm run lint`, and `npm test` locally before pushing.
- CI (`.github/workflows/build.yml`) runs an npm audit, lint, and build on every PR — make sure it's green.

## License

By contributing, you agree that your contributions will be licensed under the [Apache License 2.0](LICENSE).
