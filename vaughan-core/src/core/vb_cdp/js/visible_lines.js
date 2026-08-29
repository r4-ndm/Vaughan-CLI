// Visible text lines from document.body (includes non-interactive quote labels).
(() => {
  const body = document.body;
  if (!body) return { lines: [], url: location.href };
  const skip = /^(https?:\/\/|@)/i;
  const lines = (body.innerText || '')
    .split('\n')
    .map(s => s.trim())
    .filter(s => s.length >= 2 && s.length <= 160 && !skip.test(s));
  return { lines, url: location.href };
})()
