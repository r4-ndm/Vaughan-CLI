// Ordered interactive elements for snapshot / click / type (keep in sync).
// Walks shadow roots so wallet modals (Web3Modal / w3m) appear in refs.
(() => {
  const sel = 'a,button,input,textarea,select,[role=button],[role=link],[role=option]';
  const all = [];
  const seen = new Set();
  const walk = (root) => {
    root.querySelectorAll(sel).forEach(e => {
      if (!seen.has(e)) { seen.add(e); all.push(e); }
    });
    root.querySelectorAll('*').forEach(el => {
      if (el.shadowRoot) walk(el.shadowRoot);
    });
  };
  walk(document);
  const inputs = all.filter(e => ['INPUT', 'TEXTAREA', 'SELECT'].includes(e.tagName));
  const rest = all.filter(e =>
    !['INPUT', 'TEXTAREA', 'SELECT'].includes(e.tagName) &&
    !e.closest('nav, header, [role=navigation]')
  );
  return [...inputs, ...rest].slice(0, 50);
})()
