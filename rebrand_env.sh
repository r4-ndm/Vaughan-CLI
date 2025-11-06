#!/bin/bash

# Script to update CRUSH environment variables to VAUGHAN

echo "🔄 Updating CRUSH environment variables to VAUGHAN..."

# Update CRUSH_ to VAUGHAN_
find . -name "*.go" -type f -print0 | xargs -0 sed -i 's|CRUSH_|VAUGHAN_|g'

# Update specific references to Crush → Vaughan in error messages
find . -name "*.go" -type f -print0 | xargs -0 sed -i 's|Crush was unable|Vaughan was unable|g'
find . -name "*.go" -type f -print0 | xargs -0 sed -i 's|crush update-providers|vaughan update-providers|g'

echo "✅ Updated environment variables"
echo "✅ Updated error messages"
echo "✅ Updated help commands"

echo "🎉 Environment variable rebranding complete!"