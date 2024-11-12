#!/bin/bash

# Set error handling
set -e

# Check if contract name was provided
if [ -z "$1" ]; then
    echo "Usage: $0 <contract-name>"
    echo "Example: $0 DemoLending"
    exit 1
fi

CONTRACT_NAME="$1"

# Check if forge is available
if ! command -v forge &> /dev/null; then
    echo "Error: forge not found. Please install foundry"
    exit 1
fi

# Rebuild contracts
echo "Building contracts..."
forge build --root ./contract-mocks

# Get deployed bytecode using forge and remove 0x prefix
BYTECODE=$(forge inspect "$CONTRACT_NAME" bytecode --root ./contract-mocks | sed 's/^0x//')

if [ -z "$BYTECODE" ]; then
    echo "Error: Could not get bytecode. Make sure $CONTRACT_NAME contract exists and is compiled"
    exit 1
fi

# Copy to clipboard using pbcopy
echo -n "$BYTECODE" | pbcopy

# Verify clipboard has content
if [ -z "$(pbpaste)" ]; then
    echo "Error: Failed to copy to clipboard"
    exit 1
fi

echo "Success! Deployed bytecode for $CONTRACT_NAME has been copied to clipboard (without 0x prefix)"
echo "Bytecode length: ${#BYTECODE} characters"