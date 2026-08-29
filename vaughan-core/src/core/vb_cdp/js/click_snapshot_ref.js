// Click an interactive snapshot element whose label matches the needle.
(() => {
  const els = __VB_INTERACTIVE_ELS__;
  const needle = __VB_NEEDLE__;
  for (let i = 0; i < els.length; i++) {
    const e = els[i];
    const raw = (e.innerText || e.getAttribute('aria-label') || e.textContent || '').trim();
    const first = raw.split('\n')[0].trim().toLowerCase();
    const all = raw.toLowerCase();
    if (first === needle || (all.includes(needle) && raw.length <= 140)) {
      e.click();
      return { ok: true, ref: `e${i}`, label: raw.slice(0, 80), method: 'snapshot-ref' };
    }
  }
  return { ok: false, error: 'ref not found in snapshot', needle };
})()
