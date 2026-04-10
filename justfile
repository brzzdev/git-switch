# prettier-ignore

[private]
default:
  just --list

# Format the source code.
format:
  cargo fmt

# Install the release binary locally.
install: build-release
  mkdir -p ~/.local/bin
  cp target/release/git-switch ~/.local/bin/git-switch

# Install shell completions.
install-completions:
  #!/usr/bin/env sh
  case "$(basename "$SHELL")" in \
    zsh) \
      mkdir -p ~/.zsh/completions && \
      cp completions/_git-switch ~/.zsh/completions/_git-switch && \
      echo "Installed zsh completion to ~/.zsh/completions/_git-switch" && \
      echo "Ensure ~/.zsh/completions is in your fpath. Add to ~/.zshrc:" && \
      echo '  fpath=(~/.zsh/completions $fpath)' && \
      echo '  autoload -Uz compinit && compinit' ;; \
    bash) \
      mkdir -p ~/.local/share/bash-completion/completions && \
      cp completions/git-switch.bash ~/.local/share/bash-completion/completions/git-switch && \
      echo "Installed bash completion." ;; \
    fish) \
      mkdir -p ~/.config/fish/completions && \
      cp completions/git-switch.fish ~/.config/fish/completions/git-switch.fish && \
      echo "Installed fish completion." ;; \
    *) \
      echo "Unsupported shell: $SHELL" && exit 1 ;; \
  esac

# Build a release binary.
build-release:
  cargo build --release

# Tag a release, bump Cargo.toml, push, and create a GitHub release.
release tag:
  #!/usr/bin/env sh
  set -e
  version="{{tag}}"
  version="${version#v}"
  sed -i '' "s/^version = \".*\"/version = \"$version\"/" Cargo.toml
  cargo check --quiet
  git add Cargo.toml Cargo.lock
  git commit -m "chore: bump version to $version"
  git push
  echo "Now create a GitHub release: gh release create v$version --title v$version --notes '...'"

# Run the test suite.
test:
  cargo test

# Install git pre-commit hooks (clippy + fmt check).
tools:
  #!/usr/bin/env sh
  hook="$(git rev-parse --git-dir)/hooks/pre-commit"
  printf '%s\n' '#!/usr/bin/env sh' 'set -e' 'cargo fmt --check' 'cargo clippy -- -D warnings' > "$hook"
  chmod +x "$hook"
  echo "Installed pre-commit hook."
