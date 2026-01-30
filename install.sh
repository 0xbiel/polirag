#!/bin/bash

set -e

echo "Installing polirag..."

# Install the binary to ~/.cargo/bin
cargo install --path .

# Check if ~/.cargo/bin is in PATH
if [[ ":$PATH:" != *":$HOME/.cargo/bin:"* ]]; then
    echo ""
    echo "⚠️  ~/.cargo/bin is not in your PATH"
    echo "Add the following line to your ~/.zshrc:"
    echo ""
    echo "    export PATH=\"\$HOME/.cargo/bin:\$PATH\""
    echo ""
    echo "Then run: source ~/.zshrc"
else
    echo ""
    echo "✅ Installation complete! You can now run 'polirag' from anywhere."
fi
