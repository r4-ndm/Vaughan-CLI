{{if .Error}}
## ⛽ Gas Price Query Failed

**Command:** `{{.Command}}`  
**Network:** {{.Network}}  

**Error:** {{.Error}}

{{else}}
## ⛽ Current Gas Prices

**Network:** {{.Network}}  
**Gas Price:** {{.GasPrice}}  
{{if .BaseFee}}**Base Fee:** {{.BaseFee}}{{end}}  
{{if .PriorityFee}}**Priority Fee:** {{.PriorityFee}}{{end}}  

### 💡 Gas Strategy Suggestions:
- **Slow:** Low gas fees, longer wait times
- **Standard:** Balanced gas fees and speed  
- **Fast:** Higher gas fees, quick confirmation

Use these values with `cast send --gas-price <value>` for transactions.

{{end}}---
*Executed via Vaughan CLI*