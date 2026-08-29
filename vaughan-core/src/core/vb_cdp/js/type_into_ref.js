// Focus a snapshot ref and type text (browser_type); clear replaces the value first.
(() => {
  const els = __VB_INTERACTIVE_ELS__;
  const e = els[__VB_IDX__];
  if (!e) return { ok: false, error: 'ref not found' };
  e.focus();
  e.click();
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set;
  if (__VB_CLEAR__) {
    if (setter) setter.call(e, '');
    else e.value = '';
    e.dispatchEvent(new Event('input', { bubbles: true }));
  }
  if (setter) setter.call(e, __VB_TEXT__);
  else e.value = __VB_TEXT__;
  e.dispatchEvent(new Event('input', { bubbles: true }));
  e.dispatchEvent(new Event('change', { bubbles: true }));
  return { ok: true, ref: 'e__VB_IDX__', typed_len: __VB_TYPED_LEN__ };
})()
