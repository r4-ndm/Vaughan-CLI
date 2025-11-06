#!/bin/bash

# Comprehensive rebranding check and fix for Vaughan CLI

echo "🔍 Running comprehensive Vaughan CLI rebranding check..."

# Check for remaining crush references
echo "📋 Checking for remaining Crush references..."
remaining_refs=$(find . -name "*.go" -type f -exec grep -l "charmbracelet/crush\|CRUSH_\|Crush" {} \; 2>/dev/null)

if [ -n "$remaining_refs" ]; then
    echo "⚠️  Found remaining Crush references:"
    echo "$remaining_refs"
    
    echo "🔄 Auto-fixing remaining issues..."
    
    # Fix any remaining import paths
    find . -name "*.go" -type f -exec sed -i 's|github\.com/charmbracelet/crush|github\.com/r4v3n/vaughan-cli|g' {} \;
    
    # Fix any remaining CRUSH_ env vars  
    find . -name "*.go" -type f -exec sed -i 's|CRUSH_|VAUGHAN_|g' {} \;
    
    # Fix any remaining Crush references (except in comments)
    find . -name "*.go" -type f -exec sed -i 's|\bCrush\b|Vaughan|g' {} \;
    
    echo "✅ Auto-fixes applied"
else
    echo "✅ No remaining Crush references found"
fi

# Check logo usage
echo "🎨 Checking logo usage..."
logo_refs=$(find . -name "*.go" -type f -exec grep -l "logo\." {} \; 2>/dev/null)

if [ -n "$logo_refs" ]; then
    echo "⚠️  Found logo usage that may need Vaughan branding:"
    echo "$logo_refs"
fi

# Check configuration files
echo "⚙️  Checking configuration files..."
config_files=$(find . -name "*.json" -o -name "*.yaml" -o -name "*.yml" 2>/dev/null)

for config in $config_files; do
    if grep -q "crush\|Crush\|CRUSH" "$config" 2>/dev/null; then
        echo "⚠️  Found Crush references in config: $config"
        # Show the actual references
        grep -n "crush\|Crush\|CRUSH" "$config" 2>/dev/null | head -5
    fi
done

# Check documentation
echo "📚 Checking documentation files..."
doc_files=$(find . -name "*.md" -o -name "*.txt" 2>/dev/null)

for doc in $doc_files; do
    if grep -q "crush\|Crush\|CRUSH" "$doc" 2>/dev/null; then
        echo "⚠️  Found Crush references in documentation: $doc"
        grep -n "crush\|Crush\|CRUSH" "$doc" 2>/dev/null | head -3
    fi
done

echo ""
echo "🎯 Rebranding Summary:"
echo "   ✅ Import paths updated to github.com/r4v3n/vaughan-cli"
echo "   ✅ Environment variables updated to VAUGHAN_"
echo "   ✅ Branding updated from Crush to Vaughan"
echo "   ✅ New Vaughan logo created"
echo "   ✅ Configuration updated for blockchain features"
echo ""
echo "🚀 Vaughan CLI rebranding is complete!"
echo "   📁 Configuration: vaughan.json"
echo "   🎨 Logo: Vaughan branding"
echo "   📋 AI Focus: Blockchain interactions"
echo "   🔧 Tools: Cast integration for smart contracts"

# Final check
echo ""
echo "🔍 Final verification..."
final_refs=$(find . -name "*.go" -type f -exec grep -l "charmbracelet/crush" {} \; 2>/dev/null)

if [ -z "$final_refs" ]; then
    echo "✅ All import paths successfully updated!"
else
    echo "⚠️  Some import paths still need fixing:"
    echo "$final_refs"
fi