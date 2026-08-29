// Click the primary quote/swap CTA after tokens + amount (e.g. Switch.win "Switch Now").
(() => {
  const inSwap = el => !el.closest('nav, header, footer, [role=navigation]');
  const labels = [
    /^switch now$/i,
    /^swap now$/i,
    /^swap$/i,
    /^get quote$/i,
    /^review swap$/i,
    /^route$/i,
    /^refresh quote$/i,
  ];
  const walk = (root, out) => {
    root.querySelectorAll('button, [role=button], a').forEach(e => out.push(e));
    root.querySelectorAll('*').forEach(el => { if (el.shadowRoot) walk(el.shadowRoot, out); });
  };
  const btns = [];
  walk(document, btns);
  const candidates = btns.filter(inSwap).filter(b => {
    const t = (b.innerText || b.textContent || '').trim();
    const first = t.split('\n')[0].trim();
    if (!t || t.length > 48) return false;
    if (/^(limit|usd|25%|50%|max|refresh|swap details|details)$/i.test(first)) return false;
    if (/^switch$/i.test(first) && b.getBoundingClientRect().width < 120) return false;
    if (/connect|wallet|settings|menu|approve/i.test(first) && !/switch now/i.test(t)) return false;
    if (/switch now|swap now|get quote|review swap|refresh quote/i.test(t)) return true;
    return labels.some(re => re.test(first));
  });
  candidates.sort((a, b) => {
    const ta = (a.innerText || '').trim();
    const tb = (b.innerText || '').trim();
    if (/switch now/i.test(ta)) return -1;
    if (/switch now/i.test(tb)) return 1;
    if (/swap now/i.test(ta)) return -1;
    if (/swap now/i.test(tb)) return 1;
    return b.getBoundingClientRect().width - a.getBoundingClientRect().width;
  });
  const btn = candidates[0];
  if (!btn) return { ok: false, error: 'no swap/quote button (Switch Now, Swap, …)' };
  btn.click();
  return { ok: true, label: (btn.innerText || '').trim().slice(0, 40) };
})()
