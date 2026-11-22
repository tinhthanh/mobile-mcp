# Contributing

## Build and test

berth-mobile is a Rust crate in `berth-mobile/` (stable Rust, Xcode command line tools for the OCR helper):

```bash
cd berth-mobile
cargo build --release
cargo clippy --all-targets -- -D warnings
cargo test
```

`scripts/package.sh` builds the universal plugin (needs the `x86_64-apple-darwin` target);
`scripts/package.sh --install` copies it into `~/.berth/plugins/mobile` for a local Shipyard.

Try a change against a phone through MCP without Shipyard: `target/release/berth-mobile mcp` speaks MCP on stdio;
`scripts/run-test.sh "<task>"` (repository root) runs the android-tester agent against it.

## Pull requests

- Keep the screen format and tool behaviour stable: agents rely on them. Add a unit test next to the code
  (`screen.rs`, `screen_ios.rs`, `net.rs` have examples) for parsing changes.
- New downloads must be pinned by SHA-256 in the binary.
- CI (`.github/workflows/berth-mobile.yml`) runs clippy and the tests on macOS; tags `mobile-v*` publish a release.
