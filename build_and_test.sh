#!/bin/bash

# Vaughan CLI Build and Test Script

echo "🔨 Building Vaughan CLI..."

# Check if Go is installed
if ! command -v go &> /dev/null; then
    echo "❌ Go is not installed or not in PATH"
    echo "Please install Go: https://golang.org/dl/"
    exit 1
fi

# Clean previous builds
echo "🧹 Cleaning previous builds..."
rm -f vaughan 2>/dev/null
rm -f vaughan.exe 2>/dev/null

# Build for current platform
echo "🏗️  Building for $(go env GOOS)/$(go env GOARCH)..."
if go build -o vaughan . 2>/dev/null; then
    echo "✅ Build successful!"
else
    echo "❌ Build failed! Checking for errors..."
    go build . 2>&1 | head -20
    exit 1
fi

# Check if binary was created
if [ -f "./vaughan" ] || [ -f "./vaughan.exe" ]; then
    echo "✅ Binary created successfully!"
    
    # Test basic functionality
    echo "🧪 Testing basic functionality..."
    
    # Test help command
    if ./vaughan --help > /dev/null 2>&1; then
        echo "✅ Help command works!"
    else
        echo "❌ Help command failed"
    fi
    
    # Test version command  
    if ./vaughan --version > /dev/null 2>&1; then
        echo "✅ Version command works!"
    else
        echo "❌ Version command failed"
    fi
    
    echo ""
    echo "🚀 Vaughan CLI is ready to use!"
    echo ""
    echo "Quick start:"
    echo "  1. Set your AI provider: export OPENAI_API_KEY='your-key'"
    echo "  2. Run Vaughan: ./vaughan" 
    echo "  3. Try blockchain commands:"
    echo "     - 'Check gas prices'"
    echo "     - 'Send 0.1 ETH to vitalik.eth'"
    echo "     - 'What is my balance?'"
    
else
    echo "❌ Build failed - no binary created"
    exit 1
fi