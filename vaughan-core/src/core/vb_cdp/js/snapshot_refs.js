// Interactive-element snapshot for agents (browser_snapshot).
//
// Input values are masked: the snapshot goes into the agent transcript, and a
// field the user (or a previous browser_type call) filled may hold sensitive
// text. Agents get `hasValue` to tell empty from filled without the contents.
(() => {
  const els = __VB_INTERACTIVE_ELS__;
  const refs = els.map((e, i) => {
    const tag = e.tagName.toLowerCase();
    const isField = tag === 'input' || tag === 'textarea';
    const r = {
      ref: `e${i}`,
      tag,
      role: e.getAttribute('role') || null,
      name: (e.innerText || e.getAttribute('aria-label') || e.placeholder || e.href || '').trim().slice(0, 80)
    };
    if (isField) r.hasValue = !!(e.value && e.value.length);
    return r;
  });
  return { title: document.title, url: location.href, refs };
})()
