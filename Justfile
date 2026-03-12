# prettier-ignore

# Build a release binary.
release:
  cargo build --release

# Install the release binary locally.
install: release
  mkdir -p ~/.local/bin
  cp target/release/git-switch ~/.local/bin/git-switch

# Run the test suite.
test:
  cargo test
