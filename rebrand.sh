#!/bin/bash

# Script to replace all crush references with vaughan-cli

echo "🔄 Rebranding Crush fork to Vaughan CLI..."

# Replace import paths
find . -name "*.go" -type f -print0 | xargs -0 sed -i 's|github\.com/charmbracelet/crush|github\.com/r4v3n/vaughan-cli|g'

echo "✅ Updated import paths"

# Replace other references
find . -name "*.go" -type f -print0 | xargs -0 sed -i 's|Crush|Vaughan|g'

echo "✅ Updated Crush references to Vaughan"

echo "🎉 Rebranding complete!"