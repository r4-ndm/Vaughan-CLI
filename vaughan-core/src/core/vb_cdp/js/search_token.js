// Type the symbol into the picker modal's search box (React-safe native setter).
// Returns ok:false in frames without a search input so the all-frames
// evaluator keeps walking until the frame hosting the modal is found.
(() => {
  const symbol = __VB_SYMBOL__;
  const term = __VB_TERM__;
  const collect = (root, out) => {
    root.querySelectorAll('input').forEach(e => out.push(e));
    root.querySelectorAll('*').forEach(el => { if (el.shadowRoot) collect(el.shadowRoot, out); });
  };
  const pool = [];
  collect(document, pool);
  const inModal = e => e.closest('[role=dialog], [class*="Modal" i], [class*="modal" i], [class*="TokenList" i], [class*="token-list" i], [class*="CurrencySearch" i]');
  const search = pool.find(e =>
    (e.placeholder || '').toLowerCase().includes('search') ||
    e.type === 'search' ||
    ((e.placeholder || '').toLowerCase().includes('name') && (e.placeholder || '').toLowerCase().includes('address'))
  ) || pool.find(e => e.type === 'text' && inModal(e));
  if (!search) return { ok: false, searched: false, symbol, reason: 'no search input in frame' };
  search.focus();
  search.click();
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set;
  if (setter) setter.call(search, term);
  else search.value = term;
  search.dispatchEvent(new Event('input', { bubbles: true }));
  search.dispatchEvent(new Event('change', { bubbles: true }));
  return { ok: true, searched: true, symbol, term };
})()
