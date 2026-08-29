// Focus snapshot ref N and mark it as the agent's type target.
// Clears any stale mark first so read-back always hits the latest element.
(() => {
  document.querySelectorAll('[data-vb-type-target]').forEach(x => x.removeAttribute('data-vb-type-target'));
  const els = __VB_INTERACTIVE_ELS__;
  const e = els[__VB_IDX__];
  if (!e) return { ok: false, error: 'ref not found' };
  e.focus();
  e.click();
  e.setAttribute('data-vb-type-target', '1');
  if (__VB_CLEAR__) {
    // Select existing text so Input.insertText replaces it (real input
    // pipeline) instead of appending at the caret.
    try { e.select(); } catch (_) {}
    try { e.setSelectionRange(0, (e.value || '').length); } catch (_) {}
  }
  return { ok: true, ref: 'e__VB_IDX__', value_before: e.value ?? null };
})()
