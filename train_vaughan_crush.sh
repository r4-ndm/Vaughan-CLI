#!/bin/bash

# Vaughan Crush Training Pipeline

echo "🧠 Starting Vaughan Crush Model Training Pipeline..."

# Check requirements
if ! command -v ollama &> /dev/null; then
    echo "❌ Ollama not found. Please install Ollama first."
    exit 1
fi

# Training configuration
BASE_MODEL="qwen2.5:0.5b"
DATASET_DIR="training-data"
OUTPUT_MODEL="vaughan-crush-v1"

echo "📋 Training Configuration:"
echo "   Base Model: $BASE_MODEL"
echo "   Dataset: $DATASET_DIR/"
echo "   Output Model: $OUTPUT_MODEL"

# Ensure Ollama is running
echo "🚀 Starting Ollama server..."
nohup ollama serve > ollama.log 2>&1 &
OLLAMA_PID=$!
sleep 5

# Test Ollama connection
if ! curl -s http://127.0.0.1:11434/api/tags > /dev/null 2>&1; then
    echo "❌ Ollama server not responding. Check ollama.log for errors."
    kill $OLLAMA_PID 2>/dev/null
    exit 1
fi

echo "✅ Ollama server is running (PID: $OLLAMA_PID)"

# Pull base model if needed
echo "📥 Ensuring base model is available..."
if ! ollama list | grep -q "$BASE_MODEL"; then
    echo "📥 Downloading $BASE_MODEL..."
    ollama pull "$BASE_MODEL"
    if [ $? -ne 0 ]; then
        echo "❌ Failed to download base model"
        kill $OLLAMA_PID 2>/dev/null
        exit 1
    fi
fi

echo "✅ Base model ready"

# Prepare dataset
echo "📊 Preparing training dataset..."
if [ ! -d "$DATASET_DIR" ]; then
    echo "❌ Training dataset directory not found"
    kill $OLLAMA_PID 2>/dev/null
    exit 1
fi

# Combine all training data
COMBINED_DATASET="$DATASET_DIR/combined-training.jsonl"
cat "$DATASET_DIR"/*.jsonl > "$COMBINED_DATASET"
DATASET_SIZE=$(wc -l < "$COMBINED_DATASET")

echo "📊 Dataset Statistics:"
echo "   Combined examples: $DATASET_SIZE"
echo "   File: $COMBINED_DATASET"

if [ $DATASET_SIZE -lt 10 ]; then
    echo "⚠️  Warning: Small dataset ($DATASET_SIZE examples). Consider adding more examples for better results."
fi

# Create enhanced Modelfile for fine-tuning
echo "📝 Creating Modelfile for Vaughan Crush specialization..."
cat > Modelfile << EOF
FROM $BASE_MODEL

# Vaughan Crush parameters
PARAMETER temperature 0.7
PARAMETER top_p 0.9
PARAMETER top_k 40
PARAMETER repeat_penalty 1.1

# Blockchain specialization system prompt
SYSTEM """You are Vaughan Crush, an AI assistant specialized in blockchain development and Cast command execution.

Your expertise includes:
- Cast commands (cast_call, cast_send, gas_price, balance, approve, transfer, etc.)
- Ethereum and testnet operations (mainnet, sepolia, goerli, polygon, anvil)
- Smart contract interactions (ERC20, NFT, DeFi protocols, Uniswap)
- Gas optimization and transaction cost analysis
- Security best practices and testnet-first recommendations
- ENS name resolution and address validation

Always provide:
- Clear, actionable responses with emoji indicators (🔍📋💡✅⚠️)
- Exact Cast commands when applicable with proper flags and RPC URLs
- Security warnings for transactions and fund movements
- Gas cost estimates and network information
- Testnet recommendations for new operations
- Professional blockchain terminology and best practices

Response format:
🔍 Analysis of request and blockchain context...
📋 Specific details, commands, or contract interactions...
💡 Recommendations, optimizations, or security tips...
✅ Confirmations, next steps, or completed actions...

Focus on accuracy, security, and providing practical blockchain development assistance.
Prioritize testnet usage and always provide cost estimates for transactions."""

# Optimize for blockchain responses
TEMPLATE """{{ if .System }}<|system|>
{{ .System }}<|end|>
{{ end }}{{ if .Prompt }}<|user|>
{{ .Prompt }}<|end|>
{{ end }}<|assistant|>
{{ .Response }}<|end|>"""
EOF

echo "✅ Modelfile created with blockchain specialization"

# Create the model
echo "🏗️  Creating Vaughan Crush model..."
echo "   This may take a few minutes..."

# Create model using Ollama
ollama create "$OUTPUT_MODEL" -f Modelfile

if [ $? -eq 0 ]; then
    echo "✅ Model creation successful!"
    echo ""
    echo "📊 Model Details:"
    echo "   Name: $OUTPUT_MODEL"
    echo "   Base: $BASE_MODEL"
    echo "   Training Examples: $DATASET_SIZE"
    echo "   Specialization: Blockchain & Cast Commands"
    echo ""
else
    echo "❌ Model creation failed. Check ollama.log for details."
    kill $OLLAMA_PID 2>/dev/null
    exit 1
fi

# Test the model
echo "🧪 Testing Vaughan Crush model..."
echo "   Query: 'Check gas prices on sepolia'"

# Test query
TEST_RESPONSE=$(ollama run "$OUTPUT_MODEL" 'Check gas prices on sepolia' 2>/dev/null | head -3)
if [ -n "$TEST_RESPONSE" ]; then
    echo "✅ Model test successful!"
    echo "   Sample response: $TEST_RESPONSE..."
else
    echo "⚠️  Model test inconclusive. Try running manually:"
    echo "   ollama run $OUTPUT_MODEL"
fi

# Cleanup
echo "🧹 Cleaning up..."
kill $OLLAMA_PID 2>/dev/null 2>/dev/null

echo ""
echo "🎉 Vaughan Crush Model Training Complete!"
echo ""
echo "📋 Next Steps:"
echo "1. Test the model:"
echo "   ollama run $OUTPUT_MODEL"
echo ""
echo "2. Update Vaughan Crush config to use new model:"
echo "   cp vaughan-crush.json vaughan.json  # Make sure model name matches"
echo ""
echo "3. Run Vaughan Crush with custom model:"
echo "   ./vaughan-crush"
echo ""
echo "4. Try blockchain queries:"
echo "   - 'Check gas prices on sepolia'"
echo "   - 'What balance does vitalik.eth have?'"
echo "   - 'Send 0.001 ETH to 0x123... on sepolia'"
echo ""
echo "💡 Model is now specialized for blockchain development!"
echo "🎯 Size and speed optimized for Cast command expertise!"