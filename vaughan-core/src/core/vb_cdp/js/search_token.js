// Type the symbol into the picker modal's search box (React-safe native setter).
// Returns ok:false in frames without a search input so the all-frames
// evaluator keeps walking until the frame hosting the modal is found.
(() => {
  const symbol = __VB_SYMBOL__;
  const term = __VB_TERM__;
  const collect = (root, out) => {
    if (!root) return;
    root.querySelectorAll('input').forEach(e => out.push(e));
    root.querySelectorAll('*').forEach(el => { if (el.shadowRoot) collect(el.shadowRoot, out); });
  };
  const inModal = e => e.closest(
    '[role=dialog], [class*="Modal" i], [class*="modal" i], [class*="TokenList" i], [class*="token-list" i], [class*="CurrencySearch" i], w3m-modal, appkit-modal'
  );
  const pool = [];
  collect(document, pool);
  const search = pool.find(e =>
    inModal(e) &&
    ((e.placeholder || '').toLowerCase().includes('search') ||
      ((e.placeholder || '').toLowerCase().includes('name') &&
        (e.placeholder || '').toLowerCase().includes('address')))
  ) || pool.find(e => inModal(e) && e.type === 'text');
  if (!search) {
    const bodyHasPicker = (document.body?.innerText || '').toUpperCase().includes('SELECT_TOKEN');
    if (bodyHasPicker) {
      search = pool.find(e => (e.placeholder || '').toLowerCase().includes('search'));
    }
  }
  if (!search) return { ok: false, searched: false, symbol, reason: 'no search input in modal' };
  search.focus();
  search.click();
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set;
  if (setter) setter.call(search, term);
  else search.value = term;
  search.dispatchEvent(new Event('input', { bubbles: true }));
  search.dispatchEvent(new Event('change', { bubbles: true }));
  return { ok: true, searched: true, symbol, term };
})()
