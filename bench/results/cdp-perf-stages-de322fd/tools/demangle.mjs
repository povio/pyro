// Rough Rust v0 demangler: keeps the identifier path (last 4 parts), drops generics/hashes.
export function demangle(s) {
  if (!s.startsWith('_R')) return s;
  const parts = []; let i = 2;
  while (i < s.length) {
    const m = /^(\d+)(_?)/.exec(s.slice(i));
    if (m && s[i] !== '0') { const n = Number(m[1]); const st = i + m[0].length; const id = s.slice(st, st + n); if (id.length === n && /^[A-Za-z_]/.test(id)) { parts.push(id); i = st + n; continue; } }
    i++;
  }
  return parts.slice(-4).join('::');
}
