# 🤖 Local LLM Setup for Vaughan Crush

## 🎯 **Why Local LLM?**
- **$0 Cost**: Completely free after setup
- **Privacy**: All data stays on your machine
- **Speed**: Fast responses once loaded
- **No API Limits**: Use as much as you want
- **Blockchain Perfect**: Small queries, fast responses

## 🚀 **Quick Setup (One Command)**

```bash
cd ~/Desktop/Vaughan-CLI/vaughan-cli
./setup_local_llm.sh
```

This script will:
1. ✅ Check if Ollama is installed (install if needed)
2. 🚀 Start local Ollama server
3. 📥 Download tiny model (0.5B parameters)
4. ⚙️ Configure Vaughan Crush to use local model
5. 🎉 Ready to use!

## 🛠️ **Manual Setup**

### **1. Install Ollama**
```bash
# Linux/macOS
curl -fsSL https://ollama.ai/install.sh | sh

# Or with package manager
brew install ollama
sudo apt install ollama
```

### **2. Start Ollama Server**
```bash
ollama serve &
```

### **3. Pull Small Model**
```bash
# Tiny 0.5B parameter model (perfect for blockchain queries)
ollama pull qwen2.5:0.5b

# Alternative: 1B parameter model
ollama pull llama3.2:1b
```

### **4. Configure Vaughan Crush**
Copy `vaughan-local.json` to `vaughan.json`:
```bash
cp vaughan-local.json vaughan.json
```

## 🎯 **Configuration Details**

The local config uses:
- **Model**: Qwen 2.5 0.5B (600MB download)
- **API**: OpenAI-compatible endpoint at `http://127.0.0.1:11434/v1/`
- **Cost**: $0 (completely free)
- **Privacy**: 100% local processing
- **Default Network**: Sepolia (testnet, free gas)

## 🚀 **Usage Examples**

```bash
./vaughan-crush
```

Then try:
```
User: "What's the current gas price on sepolia?"
Vaughan Crush: [Checks gas prices, gives recommendations]

User: "What's vitalik.eth's balance?"
Vaughan Crush: [Uses cast_call to check balance]

User: "Send 0.001 ETH to 0x123... on sepolia"
Vaughan Crush: [Shows gas cost, asks for confirmation]
```

## 📊 **Model Performance**

| Model | Size | RAM | Speed | Best For |
|--------|-------|------|-------|-----------|
| qwen2.5:0.5b | 600MB | ~2s | Quick blockchain queries |
| llama3.2:1b | 800MB | ~3s | More complex interactions |

## 🔄 **If It's Slow**

First response may be slow because:
- Model loads into RAM (~3-5 seconds)
- Subsequent responses are fast (~1-2 seconds)

## 🆘 **Troubleshooting**

### **"Ollama server not responding"**
```bash
# Kill existing processes
pkill ollama

# Start fresh
ollama serve &
```

### **"Model not found"**
```bash
# List available models
ollama list

# Download if missing
ollama pull qwen2.5:0.5b
```

### **"Connection refused"**
```bash
# Test Ollama API directly
curl http://127.0.0.1:11434/api/tags

# Should return JSON with model list
```

## 💡 **Pro Tips**

1. **Use Testnet**: Default is Sepolia (free gas)
2. **Small Queries**: Local models perfect for "check gas", "get balance"
3. **Be Patient**: First response loads model, subsequent are fast
4. **Privacy**: No blockchain data leaves your machine

**Perfect Setup For:**
- Learning blockchain development
- Testing interactions
- Privacy-focused development
- Cost-free experimentation
- Educational purposes

🎉 **You now have a completely free, private AI blockchain assistant!**