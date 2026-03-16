# prettier-ignore

# List available recipes.
list:
  just --list

# Build a release binary.
release:
  cargo build --release

# Install the release binary locally.
install: release
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

# Run the test suite.
test:
  cargo test
