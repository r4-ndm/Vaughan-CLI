// Probe: is a token-picker modal (or its search input) present in this frame?
(() => {
  const q = '[role=dialog], [class*="Modal" i], [class*="modal" i], [class*="TokenList" i], [class*="token-list" i], [class*="CurrencySearch" i], w3m-modal, appkit-modal, input[placeholder*="search" i]';
  const walk = root => {
    const el = root.querySelector(q);
    if (el && el.getBoundingClientRect().height > 0) return true;
    const all = root.querySelectorAll('*');
    for (const n of all) {
      if (n.shadowRoot && walk(n.shadowRoot)) return true;
    }
    return false;
  };
  return { ok: walk(document) };
})()
