// Connected-state probe for wallet gating: an address chip (0x1234…abcd) or
// the absence of a Connect CTA means the dApp sees a connected account.
(() => {
  const text = (document.body && document.body.innerText) || '';
  const connectVisible = /connect[\s_]?wallet/i.test(text);
  const chip = /0x[0-9a-fA-F]{4}…?[0-9a-fA-F]{0,4}/.test(text);
  return { ok: true, connect_visible: connectVisible, address_chip: chip, connected: chip || !connectVisible };
})()
