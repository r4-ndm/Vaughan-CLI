// Click an element by snapshot ref index (browser_click).
(() => {
  const els = __VB_INTERACTIVE_ELS__;
  const e = els[__VB_IDX__];
  if (!e) return { ok: false, error: 'ref not found' };
  e.click();
  return { ok: true, ref: 'e__VB_IDX__' };
})()
