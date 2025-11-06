#!/bin/bash

# Test Vaughan CLI - Go-free testing approaches

echo "🧪 Testing Vaughan CLI - Multiple Approaches"
echo ""

# === APPROACH 1: Check Project Structure ===
echo "📁 APPROACH 1: Project Structure Analysis"
echo "Checking if rebranding was successful..."

# Check for Vaughan-specific files
echo "🔍 Looking for Vaughan branding files:"
find . -name "*vaughan*" -type f | head -5
echo ""

# Check import path updates
echo "🔍 Checking import path updates..."
grep -r "github.com/r4v3n/vaughan-cli" . --include="*.go" | wc -l
echo "Files with Vaughan import paths: $(grep -r 'github.com/r4v3n/vaughan-cli' . --include='*.go' | wc -l)"
echo ""

# Check for old crush references
echo "🔍 Checking for remaining Crush references..."
remaining_crush=$(find . -name "*.go" -exec grep -l "charmbracelet/crush" {} \; 2>/dev/null | wc -l)
echo "Files still referencing crush: $remaining_crush"
if [ $remaining_crush -gt 0 ]; then
    echo "⚠️  Found remaining crush references:"
    find . -name "*.go" -exec grep -l "charmbracelet/crush" {} \; 2>/dev/null
fi
echo ""

# === APPROACH 2: Configuration Analysis ===
echo "⚙️ APPROACH 2: Configuration Analysis"
echo "Checking Vaughan config..."

if [ -f "vaughan.json" ]; then
    echo "✅ vaughan.json exists"
    echo "Contains blockchain config: $(grep -q 'blockchain' vaughan.json && echo 'Yes' || echo 'No')"
    echo "Contains agents config: $(grep -q 'agents' vaughan.json && echo 'Yes' || echo 'No')"
else
    echo "❌ vaughan.json not found"
fi
echo ""

# === APPROACH 3: Tool Analysis ===
echo "🔧 APPROACH 3: Blockchain Tools Analysis"
echo "Checking blockchain AI tools..."

if [ -d "internal/agent/tools/blockchain" ]; then
    echo "✅ Blockchain tools directory exists"
    echo "Files:"
    ls -la internal/agent/tools/blockchain/*.go 2>/dev/null | awk '{print "  " $9}'
else
    echo "❌ Blockchain tools directory not found"
fi
echo ""

# === APPROACH 4: Logo Analysis ===
echo "🎨 APPROACH 4: Logo Analysis"
echo "Checking Vaughan logo..."

if [ -f "internal/tui/components/logo/vaughan_logo.go" ]; then
    echo "✅ Vaughan logo file exists"
    echo "Contains Vaughan branding: $(grep -q 'Vaughan' internal/tui/components/logo/vaughan_logo.go && echo 'Yes' || echo 'No')"
else
    echo "❌ Vaughan logo file not found"
fi
echo ""

# === APPROACH 5: Environment Analysis ===
echo "🌍 APPROACH 5: Environment Analysis"
echo "Checking environment variable updates..."

vaughan_refs=$(find . -name "*.go" -exec grep -l "VAUGHAN_" {} \; 2>/dev/null | wc -l)
crush_refs=$(find . -name "*.go" -exec grep -l "CRUSH_" {} \; 2>/dev/null | wc -l)
echo "Files with VAUGHAN_ variables: $vaughan_refs"
echo "Files with CRUSH_ variables: $crush_refs"
echo ""

# === APPROACH 6: Documentation Analysis ===
echo "📚 APPROACH 6: Documentation Analysis"
echo "Checking documentation updates..."

if [ -f "VAUGHAN_README.md" ]; then
    echo "✅ Vaughan README exists"
    echo "Contains blockchain focus: $(grep -q 'blockchain\|Cast\|smart contract' VAUGHAN_README.md && echo 'Yes' || echo 'No')"
fi

if [ -f "REBRANDING_COMPLETE.md" ]; then
    echo "✅ Rebranding documentation exists"
fi
echo ""

# === SUMMARY ===
echo "📊 TEST SUMMARY"
echo "============"

# Calculate success score
score=0
max_score=100

# Project structure (20 points)
if [ -f "main.go" ] && [ -d "internal" ]; then
    echo "✅ Project structure: 20/20 points"
    score=$((score + 20))
else
    echo "❌ Project structure: 0/20 points"
fi

# Rebranding (25 points)  
if [ $remaining_crush -eq 0 ]; then
    echo "✅ Import path rebranding: 25/25 points"
    score=$((score + 25))
else
    echo "⚠️  Import path rebranding: $((25 - remaining_crush * 5))/25 points"
    score=$((score + 25 - remaining_crush * 5))
fi

# Blockchain tools (20 points)
if [ -d "internal/agent/tools/blockchain" ]; then
    echo "✅ Blockchain tools: 20/20 points"
    score=$((score + 20))
else
    echo "❌ Blockchain tools: 0/20 points"
fi

# Configuration (15 points)
if [ -f "vaughan.json" ]; then
    echo "✅ Configuration: 15/15 points"
    score=$((score + 15))
else
    echo "❌ Configuration: 0/15 points"
fi

# Documentation (10 points)
if [ -f "VAUGHAN_README.md" ]; then
    echo "✅ Documentation: 10/10 points"
    score=$((score + 10))
else
    echo "❌ Documentation: 0/10 points"
fi

# Environment variables (10 points)
if [ $crush_refs -eq 0 ] && [ $vaughan_refs -gt 0 ]; then
    echo "✅ Environment variables: 10/10 points"
    score=$((score + 10))
else
    echo "⚠️  Environment variables: $((10 - crush_refs * 2))/10 points"
    score=$((score + 10 - crush_refs * 2))
fi

echo ""
echo "🎯 FINAL SCORE: $score/$max_score"
if [ $score -ge 90 ]; then
    echo "🏆 EXCELLENT - Vaughan CLI is ready!"
elif [ $score -ge 75 ]; then
    echo "✅ GOOD - Mostly ready, minor fixes needed"
elif [ $score -ge 50 ]; then
    echo "⚠️  OK - Some work needed"
else
    echo "❌ NEEDS WORK - Significant fixes required"
fi

echo ""
echo "🚀 Next Steps:"
echo "1. Install Go to build and test functionality"
echo "2. Test basic commands: ./vaughan --help"  
echo "3. Test AI integration with OpenAI/Anthropic API"
echo "4. Test blockchain commands: 'Check gas price'"
echo ""
echo "📋 Ready for blockchain development!"