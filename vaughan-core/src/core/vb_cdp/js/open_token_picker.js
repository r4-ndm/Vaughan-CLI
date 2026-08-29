// Open the token picker on the requested swap leg (skip if already set).
// `avoid` lets the caller retry with the next candidate when the first click
// opened no modal (e.g. a venue tab whose label looks like a ticker).
(() => {
  const symbol = __VB_SYMBOL__;
  const side = "__VB_SIDE__";
  const avoid = __VB_AVOID__;
  const norm = s => (s || '').trim().toUpperCase();
  const aliases = {
    PLS: ['PLS', 'PULSE', 'NATIVE'],
    WPLS: ['WPLS'],
    HEX: ['HEX'],
  };
  const matchSym = text => {
    const u = norm((text || '').split('\n')[0]);
    const al = aliases[symbol] || [symbol];
    return al.some(a => u === a || u.startsWith(a + ' '));
  };
  const inSwap = el => !el.closest('nav, header, footer, [role=navigation]');
  // Venue chrome that matches the ticker shape — never a token button.
  const notToken = t => /^(connect|swap|approve|settings|menu|wallet|trade|limit|max|bal|refresh|flip|copy|usd|sell|buy|route|hop|slippage|gas|impact|account|swap_|main_site|widget|api_docs|telegram|twitter|risk|online|navigation|switch|get|details|quote|search|close|back|import)$/i.test(t);
  const isTickerBtn = el => {
    const t = (el.innerText || el.textContent || '').trim();
    if (!t || t.length > 32) return false;
    const first = t.split('\n')[0].trim();
    if (notToken(first)) return false;
    if (avoid && norm(first) === norm(avoid)) return false;
    if (/select.*token|choose.*token/i.test(t)) return true;
    // Digit-leading tickers exist (9MM, 1INCH) — require a letter anywhere,
    // not necessarily first.
    return /^(?=.*[A-Z])[A-Z0-9]{2,10}$/i.test(first);
  };
  // Some venues (Switch.win) use div/span selectors, not <button> — climb to
  // a clickable ancestor when one exists, else click the element itself.
  const clickable = el => el.closest('button, [role=button]') || el;
  const tickerCandidates = root => {
    const matched = [...root.querySelectorAll('button, [role=button], div, span')]
      .filter(inSwap)
      .filter(isTickerBtn);
    // Keep innermost matches only — a ticker row and its ticker label span are
    // the same selector; clicking the innermost bubbles to the row handler.
    const innermost = matched.filter(el => !matched.some(o => o !== el && el.contains(o)));
    return innermost
      .map(clickable)
      .filter((el, i, arr) => arr.indexOf(el) === i)
      .sort((a, b) => a.getBoundingClientRect().top - b.getBoundingClientRect().top);
  };
  const findLegBtn = () => {
    const sideLabels = side === 'input'
      ? ['sell', 'from', 'you pay', 'pay']
      : ['buy', 'to', 'you receive', 'receive', 'get'];
    const candidates = [...document.querySelectorAll('button, [role=button], div, span, label, p')]
      .filter(inSwap);
    for (const lab of sideLabels) {
      const labelEl = candidates.find(el => {
        const t = (el.innerText || el.textContent || '').trim().toLowerCase();
        return t === lab || t.startsWith(lab + ' ') || t.startsWith(lab + '\n');
      });
      if (!labelEl) continue;
      let node = labelEl;
      for (let i = 0; i < 8 && node; i++) {
        const btns = tickerCandidates(node);
        if (btns.length) return btns[0];
        node = node.parentElement;
      }
    }
    return null;
  };
  let btn = findLegBtn();
  if (!btn) {
    const btns = tickerCandidates(document);
    if (!btns.length) return { ok: false, error: 'no token selector buttons', count: 0 };
    btn = side === 'input' ? btns[0] : (btns.length > 1 ? btns[1] : btns[btns.length - 1]);
  }
  const shown = (btn.innerText || btn.textContent || '').trim();
  if (matchSym(shown)) return { ok: true, already: true, symbol, side, shown: shown.split('\n')[0] };
  btn.click();
  return { ok: true, clicked: true, symbol, side, shown: shown.split('\n')[0] };
})()
