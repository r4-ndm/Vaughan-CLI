// Probe whether the dApp UI already shows one of the wallet's chain labels
// as selected (active tab/chip in a network picker). Generic — no venue names.
(() => {
  const terms = __VB_TERMS_JSON__;
  const norm = s => (s || '').toLowerCase().replace(/\s+/g, ' ').trim();
  const targets = terms.map(norm).filter(Boolean);
  if (!targets.length) return { ok: false, error: 'no terms' };

  const clickables = [...document.querySelectorAll(
    'button, a, [role=button], [role=option], [role=menuitem], li, div'
  )].filter(el => {
    const r = el.getBoundingClientRect();
    if (r.width < 8 || r.height < 8 || r.bottom < 0 || r.top > innerHeight) return false;
    const t = norm(el.innerText || el.textContent || '');
    return t.length > 0 && t.length < 48;
  });

  const matchesTerm = t => targets.some(term => t === term || t.startsWith(term + ' ') || t.includes(term));

  for (const el of clickables) {
    const t = norm(el.innerText || el.textContent || '');
    if (!matchesTerm(t)) continue;
    const active =
      el.getAttribute('aria-selected') === 'true' ||
      el.getAttribute('aria-current') === 'true' ||
      el.getAttribute('data-active') === 'true' ||
      /\bactive\b/i.test(el.className || '') ||
      /\bselected\b/i.test(el.className || '');
    if (active) {
      return { ok: true, already: true, matched: t };
    }
  }
  return { ok: true, already: false };
})()
