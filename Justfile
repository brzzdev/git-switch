# prettier-ignore

# Build a release binary and install it locally.
release:
  cargo build --release
  mkdir -p ~/.local/bin
  cp target/release/git-switch ~/.local/bin/git-switch

# Run the test suite.
test:
  cargo test
