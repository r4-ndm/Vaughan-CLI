{{if .Error}}
## ❌ Transaction Failed

**Command:** `{{.Command}}`  
**Network:** {{.Network}}  
**To:** {{.To}}  
**Value:** {{.Value}}  

**Error:** {{.Error}}

{{else}}
## ✅ Transaction Sent

**Command:** `{{.Command}}`  
**Network:** {{.Network}}  
**To:** {{.To}}  
**Value:** {{.Value}}  
**Transaction Hash:** `{{.TxHash}}`  
{{if .GasUsed}}**Gas Used:** {{.GasUsed}}{{end}}

You can track this transaction using:
```bash
cast tx {{.TxHash}} --rpc-url {{.Network}}
```

{{end}}---
*Executed via Vaughan CLI*