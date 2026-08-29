// Read back the marked type target's value — the post-mask truth, which can
// differ from what was typed (some dApp masks reformat on input).
(() => {
  const e = document.querySelector('[data-vb-type-target="1"]');
  if (!e) return { ok: false, error: 'type target not found' };
  return { ok: true, value: e.value ?? null };
})()
