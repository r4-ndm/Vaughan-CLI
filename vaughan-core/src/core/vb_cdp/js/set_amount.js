// Set the sell amount on the swap form (React-safe native setter + events).
(() => {
  const amount = __VB_AMOUNT__;
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
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set;
  if (setter) setter.call(target, '');
  else target.value = '';
  target.dispatchEvent(new Event('input', { bubbles: true }));
  if (setter) setter.call(target, amount);
  else target.value = amount;
  target.dispatchEvent(new Event('input', { bubbles: true }));
  target.dispatchEvent(new Event('change', { bubbles: true }));
  target.dispatchEvent(new KeyboardEvent('keyup', { key: 'Enter', bubbles: true }));
  return { ok: true, amount, value: target.value };
})()
