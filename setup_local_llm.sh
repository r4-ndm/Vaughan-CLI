#!/bin/bash

# Setup Local LLM for Vaughan Crush

echo "🤖 Setting up Local LLM for Vaughan Crush..."

# Check if Ollama is installed
if ! command -v ollama &> /dev/null; then
    echo "❌ Ollama not found. Installing..."
    curl -fsSL https://ollama.ai/install.sh | sh
    echo "✅ Ollama installed! Please restart your terminal and run this script again."
    exit 1
fi

echo "✅ Ollama found!"

# Start Ollama server
echo "🚀 Starting Ollama server..."
nohup ollama serve > /dev/null 2>&1 &
sleep 5

# Test if Ollama is responding
if ! curl -s http://127.0.0.1:11434/api/tags > /dev/null 2>&1; then
    echo "❌ Ollama server not responding. Please check installation."
    exit 1
fi

echo "✅ Ollama server running!"

# Pull a tiny model
echo "📥 Pulling tiny model (this may take a minute)..."
ollama pull qwen2.5:0.5b

if [ $? -eq 0 ]; then
    echo "✅ Model downloaded successfully!"
else
    echo "❌ Failed to download model. Trying alternative..."
    ollama pull llama3.2:1b
fi

# Copy local config
echo "⚙️ Setting up local configuration..."
cp vaughan-local.json vaughan.json

echo ""
echo "🎉 Local LLM Setup Complete!"
echo ""
echo "📋 Next Steps:"
echo "1. Start Vaughan Crush:"
echo "   ./vaughan-crush"
echo ""
echo "2. Try blockchain commands:"
echo "   - 'Check gas prices on sepolia'"
echo "   - 'What balance does vitalik.eth have?'"  
echo "   - 'Send 0.001 ETH to 0x123... on sepolia'"
echo ""
echo "🔧 Model: Qwen 2.5 0.5B (local, free, fast)"
echo "🌐 Network: Sepolia (testnet, free gas)"
echo "💰 Cost: $0 (completely free!)"
echo ""
echo "⚠️  Note: First response may be slow as model loads into memory"
echo "🔄 Note: Model runs locally, no data leaves your machine"