// Re-focus the marked type target and select its contents so the next
// Input.insertText replaces the value instead of appending.
(() => {
  const e = document.querySelector('[data-vb-type-target="1"]');
  if (!e) return { ok: false, error: 'type target not found' };
  e.focus();
  e.click();
  try { e.select(); } catch (_) {}
  try { e.setSelectionRange(0, (e.value || '').length); } catch (_) {}
  return { ok: true };
})()
