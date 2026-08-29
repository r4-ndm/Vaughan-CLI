// Try to open a dApp network/chain picker when the target chain is not visible.
(() => {
  const openers = [
    'select network',
    'select chain',
    'wrong network',
    'switch network',
    'choose network',
  ];
  const norm = s => (s || '').toLowerCase().replace(/\s+/g, ' ').trim();
  const clickables = [...document.querySelectorAll('button, a, [role=button], [role=combobox]')]
    .filter(el => {
      const r = el.getBoundingClientRect();
      return r.width > 0 && r.height > 0;
    });
  for (const el of clickables) {
    const t = norm(el.innerText || el.textContent || el.getAttribute('aria-label') || '');
    if (openers.some(o => t.includes(o))) {
      el.click();
      return { ok: true, method: 'opener', label: (el.innerText || '').trim().slice(0, 80) };
    }
  }
  // Combobox / haspopup network switcher in header (no readable label).
  for (const el of clickables) {
    if (el.getAttribute('aria-haspopup') === 'true' || el.getAttribute('role') === 'combobox') {
      const t = norm(el.innerText || '');
      if (t && t.length < 32) {
        el.click();
        return { ok: true, method: 'haspopup', label: t.slice(0, 80) };
      }
    }
  }
  return { ok: false, error: 'chain picker opener not found' };
})()
