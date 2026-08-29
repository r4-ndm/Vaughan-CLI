// Last-resort write into the marked type target via the React native-setter
// hack. Some masked inputs misparse this (observed: value ÷1000 on two
// venues) — only used when real-pipeline insertion failed verification.
(() => {
  const e = document.querySelector('[data-vb-type-target="1"]');
  if (!e) return { ok: false, error: 'type target not found' };
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set;
  if (setter) setter.call(e, '');
  else e.value = '';
  e.dispatchEvent(new Event('input', { bubbles: true }));
  if (setter) setter.call(e, __VB_TEXT__);
  else e.value = __VB_TEXT__;
  e.dispatchEvent(new Event('input', { bubbles: true }));
  e.dispatchEvent(new Event('change', { bubbles: true }));
  return { ok: true, value: e.value ?? null };
})()
