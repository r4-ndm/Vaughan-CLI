// Pick the token row from the (possibly search-filtered) picker modal.
// Reads full row text (ticker + name + 0x… address) — required on 9X where
// many tokens share the "USDC" label.
(() => {
  const symbol = __VB_SYMBOL__;
  const preferAddr = __VB_ADDRESS__;
  const norm = s => (s || '').trim().toUpperCase();
  const addrNorm = a => (a || '').trim().toLowerCase();
  const rowText = el => (el.innerText || el.textContent || '').trim();
  const rowMatchesAddr = (el, addr) => {
    if (!addr) return false;
    const t = rowText(el).toLowerCase();
    const a = addrNorm(addr);
    if (!a.startsWith('0x') || a.length < 10) return false;
    if (t.includes(a)) return true;
    // UI truncates tails (Switch: …1f07; 9X: …06eB48).
    let tailOk = false;
    for (const n of [8, 6, 4]) {
      if (t.includes(a.slice(-n))) {
        tailOk = true;
        break;
      }
    }
    if (!tailOk) return false;
    for (const n of [10, 8, 6]) {
      if (t.includes(a.slice(0, n))) return true;
    }
    return t.includes('0x' + a.slice(2, 6));
  };
  const rowHasContract = t => /0x[a-f0-9]{4,}/i.test(t);
  const rowMatchesSymbol = (t, sym) => {
    if (!t) return false;
    const first = norm(t.split('\n')[0]);
    if (first === sym) return true;
    if (sym === 'PLS' && (first === 'PULSE' || first === 'NATIVE')) return true;
    if (first === 'P' + sym) return true;
    if (first.startsWith(sym + ' ') || first.startsWith(sym + '(')) return true;
    return new RegExp('\\b' + sym.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '\\b', 'i').test(t);
  };
  const collect = (root, out) => {
    if (!root) return;
    root.querySelectorAll('button, a, [role=button], [role=option], li, div, span, p').forEach(e => out.push(e));
    root.querySelectorAll('*').forEach(el => { if (el.shadowRoot) collect(el.shadowRoot, out); });
  };
  const findModalRoot = () => {
    const dialog = document.querySelector(
      '[role=dialog], [class*="Modal" i], [class*="modal" i], [class*="TokenList" i], [class*="token-list" i], [class*="CurrencySearch" i], w3m-modal, appkit-modal'
    );
    if (dialog) return dialog;
    const inputs = [];
    collect(document.body, inputs);
    const search = inputs.find(e =>
      e.tagName === 'INPUT' &&
      ((e.placeholder || '').toLowerCase().includes('search') ||
        (e.placeholder || '').toLowerCase().includes('name'))
    );
    if (search) {
      return search.closest('[role=dialog]') ||
        search.closest('[class*="modal" i]') ||
        search.closest('[class*="Modal" i]') ||
        search.parentElement?.parentElement?.parentElement ||
        document.body;
    }
    return document.body;
  };
  const modalRoot = findModalRoot();
  const pool = [];
  collect(modalRoot, pool);
  if ((document.body?.innerText || '').toUpperCase().includes('SELECT_TOKEN')) {
    collect(document.body, pool);
  }
  const visible = pool.filter(el => {
    const r = el.getBoundingClientRect();
    return r.height > 8 && r.width > 8;
  });
  const rowLike = el => {
    const t = rowText(el);
    if (!t || t.length > 500 || t.length < 12) return false;
    const first = norm(t.split('\n')[0]);
    if (/^(import|custom token|manage|clear|close|cancel|back|select token)$/i.test(first)) return false;
    if (first.startsWith('//') || first.includes('SELECT')) return false;
    const addrCount = (t.match(/0x[a-f0-9]{4,}/gi) || []).length;
    if (addrCount !== 1) return false;
    const r = el.getBoundingClientRect();
    if (r.height > 160 || r.height < 12) return false;
    if (symbol === 'HEX' && (first === '9MM' || first.startsWith('9MM.'))) return false;
    if (symbol === 'PLS' && first === 'HEX') return false;
    if (preferAddr) {
      return rowMatchesAddr(el, preferAddr) && (rowMatchesSymbol(t, symbol) || rowHasContract(t));
    }
    if (!rowMatchesSymbol(t, symbol)) return false;
    return rowHasContract(t);
  };
  let rows = visible.filter(rowLike);
  rows = rows.filter((el, i, arr) => {
    const t = rowText(el);
    return !arr.some((other, j) => {
      if (j === i) return false;
      const ot = rowText(other);
      return ot.includes(t) && ot.length > t.length;
    });
  });
  const addrRows = preferAddr ? rows.filter(r => rowMatchesAddr(r, preferAddr)) : [];
  const candidates = preferAddr ? addrRows : rows;
  candidates.sort((a, b) => {
    const ta = norm(rowText(a).split('\n')[0]);
    const tb = norm(rowText(b).split('\n')[0]);
    if (ta === 'P' + symbol) return -1;
    if (tb === 'P' + symbol) return 1;
    if (ta === symbol) return -1;
    if (tb === symbol) return 1;
    return a.getBoundingClientRect().top - b.getBoundingClientRect().top;
  });
  const summarize = el => {
    const lines = rowText(el).split('\n').map(l => l.trim()).filter(Boolean);
    return {
      ticker: lines[0] || '',
      name: lines[1] || '',
      address: (lines.find(l => /0x[a-f0-9]{4,}/i.test(l)) || '').slice(0, 48),
    };
  };
  const visible_rows = rows.slice(0, 5).map(summarize);
  const pick = candidates[0];
  if (!pick) {
    return {
      ok: false,
      error: preferAddr ? 'no row matches registry address' : 'token row not found',
      symbol,
      preferAddr,
      rows: rows.length,
      addr_rows: addrRows.length,
      visible_rows,
    };
  }
  pick.click();
  const pickedText = rowText(pick);
  return {
    ok: true,
    selected: symbol,
    method: 'picker',
    picked: norm(pickedText.split('\n')[0]),
    picked_name: pickedText.split('\n')[1] || '',
    picked_address: (pickedText.split('\n').find(l => /0x[a-f0-9]{4,}/i.test(l)) || '').trim(),
    matched_address: !!(preferAddr && rowMatchesAddr(pick, preferAddr)),
  };
})()
