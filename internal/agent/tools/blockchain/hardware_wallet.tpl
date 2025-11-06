# Hardware Wallet Support

I'll help you connect and manage your hardware wallets for secure blockchain interactions.

## 🖥️ Hardware Wallet Detection

{{range .}}
### {{.Type}}
- **Status**: {{.Status}}
- **Model**: {{.Model}}
- **Connected**: {{if .Connected}}✅ Yes{{else}}❌ No{{end}}
- **Compatible**: {{if .Compatible}}✅ Yes{{else}}❌ No{{end}}

{{if .Recommendations}}
#### Recommendations:
{{range .Recommendations}}
- {{.}}
{{end}}
{{end}}

{{end}}

## 🔐 Security Best Practices

1. **Keep your recovery phrase secure and offline**
2. **Verify transactions on your hardware device screen**
3. **Use official firmware and software**
4. **Never share your private keys or recovery phrase**
5. **Test with small amounts first**

## 🚀 Getting Started

1. **Connect your hardware wallet** via USB
2. **Install the latest firmware** from the official manufacturer
3. **Enable blockchain support** for your specific network
4. **Test with small transactions** before large transfers

## 📱 Supported Devices

- **Trezor Model T**
- **Ledger Nano S/X**
- **KeepKey**
- **ColdCard** (limited support)

## 🔧 Troubleshooting

If your device isn't detected:
1. Check USB connection and cable
2. Install official manufacturer software
3. Update device firmware
4. Try different USB port
5. Check browser extensions

For detailed setup instructions, run the specific hardware wallet commands for your device.