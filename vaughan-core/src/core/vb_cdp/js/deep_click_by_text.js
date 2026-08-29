// Deep click by visible text — shadow DOM + aria-label; used inside each frame.
// Invoked as (fn)(needleJson, skipBannerRegex).
(needle, skipBanner) => {
  const hits = [];
  const walk = (root) => {
    const stack = [root];
    while (stack.length) {
      const node = stack.pop();
      if (node.nodeType !== 1) continue;
      const el = node;
      const rect = el.getBoundingClientRect();
      const t = (el.innerText || el.textContent || el.getAttribute('aria-label') || '').trim();
      const tl = t.toLowerCase();
      const first = tl.split('\n')[0].trim();
      if (rect.height > 0 && rect.width > 0 && t && t.length <= 140) {
        if (!skipBanner.test(tl) && (first === needle || tl.includes(needle))) {
          hits.push({ el, t, len: t.length });
        }
      }
      if (el.shadowRoot) stack.push(el.shadowRoot);
      for (let i = el.children.length - 1; i >= 0; i--) stack.push(el.children[i]);
    }
  };
  walk(document.documentElement);
  hits.sort((a, b) => {
    const al = a.t.toLowerCase(), bl = b.t.toLowerCase();
    if (al === needle) return -1;
    if (bl === needle) return 1;
    return al.length - bl.length;
  });
  const hit = hits[0];
  if (!hit) return { ok: false, error: 'text not found', needle };
  hit.el.click();
  return { ok: true, label: hit.t.slice(0, 80), method: 'deep-dom' };
}
