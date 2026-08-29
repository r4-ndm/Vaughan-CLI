// Find the swap amount input, mark it as the agent's type target, focus it.
// Sell leg is the topmost amount-shaped input on standard swap forms.
(() => {
  document.querySelectorAll('[data-vb-type-target]').forEach(x => x.removeAttribute('data-vb-type-target'));
  const inSwap = el => !el.closest('nav, header, footer, [role=navigation]');
  const inputs = [...document.querySelectorAll('input')]
    .filter(inSwap)
    .filter(e => !['checkbox', 'radio', 'hidden', 'search'].includes(e.type))
    .filter(e => e.getAttribute('role') !== 'switch')
    .filter(e => !e.closest('[role=dialog]') || e.placeholder?.toLowerCase().includes('search'))
    .sort((a, b) => a.getBoundingClientRect().top - b.getBoundingClientRect().top);
  const isAmount = e => {
    const ph = (e.placeholder || '').toLowerCase();
    const val = (e.value || '').trim();
    const inp = e.inputMode || '';
    if (ph.includes('search')) return false;
    if (/^#[0-9a-f]{3,8}$/i.test(val)) return false;
    if (inp === 'decimal' || inp === 'numeric') return true;
    return ph.includes('0.0') || ph.includes('amount') || val === '' || /^[0-9.]+$/.test(val);
  };
  const target = inputs.find(isAmount) || inputs[0];
  if (!target) return { ok: false, error: 'no amount input' };
  target.focus();
  target.click();
  target.setAttribute('data-vb-type-target', '1');
  try { target.select(); } catch (_) {}
  try { target.setSelectionRange(0, (target.value || '').length); } catch (_) {}
  return { ok: true, value_before: target.value ?? null };
})()
