// Pick the token row from the (possibly search-filtered) picker modal.
// Runs after search_token.js + a settle delay, so rows are collected fresh.
(() => {
  const symbol = __VB_SYMBOL__;
  const norm = s => (s || '').trim().toUpperCase();
  const collect = (root, out) => {
    root.querySelectorAll('button, a, [role=button], [role=option], li, div, span, p').forEach(e => out.push(e));
    root.querySelectorAll('*').forEach(el => { if (el.shadowRoot) collect(el.shadowRoot, out); });
  };
  const pool = [];
  const modal = document.querySelector(
    '[role=dialog], [class*="Modal" i], [class*="modal" i], [class*="TokenList" i], [class*="token-list" i], [class*="CurrencySearch" i], w3m-modal, appkit-modal'
  );
  if (modal) collect(modal, pool);
  else collect(document.body, pool);
  const rowMatches = el => {
    if (!el.getBoundingClientRect().height) return false;
    const t = (el.innerText || el.textContent || '').trim();
    if (!t || t.length > 100) return false;
    const first = norm(t.split('\n')[0]);
    if (/^(import|custom token|manage|clear|close|cancel|back)$/i.test(first)) return false;
    if (symbol === 'HEX' && (first === '9MM' || first.startsWith('9MM.'))) return false;
    if (symbol === 'PLS' && first === 'HEX') return false;
    if (first === symbol) return true;
    if (symbol === 'PLS' && (first === 'PULSE' || first === 'NATIVE')) return true;
    // "INC (0x2fa8...C95d)" style rows: ticker followed by address tail.
    if (first.startsWith(symbol + ' ') || first.startsWith(symbol + '(')) return true;
    return new RegExp('\\b' + symbol.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '\\b', 'i').test(t);
  };
  const rows = pool.filter(rowMatches);
  rows.sort((a, b) => {
    const ta = norm((a.innerText || '').split('\n')[0]);
    const tb = norm((b.innerText || '').split('\n')[0]);
    if (ta === symbol) return -1;
    if (tb === symbol) return 1;
    if (symbol === 'PLS') {
      if (ta === 'PLS') return -1;
      if (tb === 'PLS') return 1;
    }
    return a.getBoundingClientRect().top - b.getBoundingClientRect().top;
  });
  const pick = rows[0];
  if (!pick) return { ok: false, error: 'token row not found', symbol, rows: rows.length };
  pick.click();
  return { ok: true, selected: symbol, method: 'picker', picked: norm((pick.innerText || '').split('\n')[0]) };
})()
