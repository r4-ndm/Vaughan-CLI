// Wait probe: text / selector / URL substring (browser_wait polls this).
(() => {
  const text = __VB_TEXT__;
  const selector = __VB_SELECTOR__;
  const urlPart = __VB_URL_PART__;
  if (text && document.body && document.body.innerText.includes(text)) {
    return { ok: true, matched: 'text', url: location.href };
  }
  if (selector && document.querySelector(selector)) {
    return { ok: true, matched: 'selector', url: location.href };
  }
  if (urlPart && location.href.includes(urlPart)) {
    return { ok: true, matched: 'url', url: location.href };
  }
  return { ok: false };
})()
