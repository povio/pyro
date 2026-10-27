// Statistics shared by cdp-perf.mjs and cdp-perf-compare.mjs.

export const sorted = (xs) => [...xs].sort((a, b) => a - b);

// Linear-interpolated quantile, q in [0, 1].
export function quantile(xs, q) {
  const s = sorted(xs);
  if (!s.length) return NaN;
  const pos = (s.length - 1) * q;
  const lo = Math.floor(pos);
  const hi = Math.ceil(pos);
  return s[lo] + (s[hi] - s[lo]) * (pos - lo);
}

export const median = (xs) => quantile(xs, 0.5);
export const mean = (xs) => xs.reduce((a, b) => a + b, 0) / xs.length;
export const mad = (xs) => {
  const m = median(xs);
  return median(xs.map((x) => Math.abs(x - m)));
};

const r = (x, d = 4) => (Number.isFinite(x) ? +x.toFixed(d) : null);

export function summarize(xs) {
  if (!xs.length) return { n: 0 };
  const m = median(xs);
  return {
    n: xs.length,
    min: r(Math.min(...xs)),
    p10: r(quantile(xs, 0.1)),
    median: r(m),
    p90: r(quantile(xs, 0.9)),
    p95: r(quantile(xs, 0.95)),
    max: r(Math.max(...xs)),
    mean: r(mean(xs)),
    // Robust spread relative to the median (MAD scaled to a normal sigma).
    rsd: r((1.4826 * mad(xs)) / m),
  };
}

// Two-sided Mann-Whitney U test: exact for small tie-free samples (such as per-round medians), otherwise the
// normal approximation with tie correction.
// Returns { p, effect } where effect = P(b > a) - P(b < a) in [-1, 1] (Cliff's delta).
export function mannWhitney(a, b) {
  const n1 = a.length;
  const n2 = b.length;
  if (!n1 || !n2) return { p: 1, effect: 0 };
  const all = [...a.map((v) => [v, 0]), ...b.map((v) => [v, 1])].sort((x, y) => x[0] - y[0]);
  const ranks = new Array(all.length);
  let tieTerm = 0;
  for (let i = 0; i < all.length; ) {
    let j = i;
    while (j + 1 < all.length && all[j + 1][0] === all[i][0]) j++;
    const rank = (i + j) / 2 + 1;
    for (let k = i; k <= j; k++) ranks[k] = rank;
    const t = j - i + 1;
    tieTerm += t * t * t - t;
    i = j + 1;
  }
  let r1 = 0;
  for (let i = 0; i < all.length; i++) if (all[i][1] === 0) r1 += ranks[i];
  const u1 = r1 - (n1 * (n1 + 1)) / 2; // pairs where a > b
  const n = n1 + n2;
  if (tieTerm === 0 && n1 * n2 <= 2500) return { p: exactP(n1, n2, u1), effect: (n1 * n2 - 2 * u1) / (n1 * n2) };
  const mu = (n1 * n2) / 2;
  const sigma = Math.sqrt(((n1 * n2) / 12) * (n + 1 - tieTerm / (n * (n - 1))));
  const z = sigma > 0 ? (u1 - mu) / sigma : 0;
  const p = sigma > 0 ? 2 * (1 - normalCdf(Math.abs(z))) : 1;
  // u1 counts a > b (ties as half); effect is positive when b tends to be larger.
  return { p, effect: (n1 * n2 - 2 * u1) / (n1 * n2) };
}

// P-value of U under the null by counting rank arrangements: c[m][k][u] = c[m-1][k][u-k] + c[m][k-1][u].
function exactP(n1, n2, u) {
  const max = n1 * n2;
  let prev = [];
  for (let k = 0; k <= n2; k++) prev.push(Float64Array.from({ length: max + 1 }, (_, v) => (v === 0 ? 1 : 0)));
  for (let m = 1; m <= n1; m++) {
    const cur = [Float64Array.from({ length: max + 1 }, (_, v) => (v === 0 ? 1 : 0))];
    for (let k = 1; k <= n2; k++) {
      const row = new Float64Array(max + 1);
      for (let v = 0; v <= max; v++) row[v] = (v >= k ? prev[k][v - k] : 0) + cur[k - 1][v];
      cur.push(row);
    }
    prev = cur;
  }
  const counts = prev[n2];
  const total = counts.reduce((a, b) => a + b, 0);
  let lo = 0;
  let hi = 0;
  for (let v = 0; v <= max; v++) {
    if (v <= u) lo += counts[v];
    if (v >= u) hi += counts[v];
  }
  return Math.min(1, (2 * Math.min(lo, hi)) / total);
}

// Abramowitz-Stegun 7.1.26 erf approximation; accurate to ~1e-7, plenty for a p-value cut-off.
function normalCdf(z) {
  const t = 1 / (1 + 0.3275911 * (Math.abs(z) / Math.SQRT2));
  const y =
    1 -
    ((((1.061405429 * t - 1.453152027) * t + 1.421413741) * t - 0.284496736) * t + 0.254829592) *
      t *
      Math.exp(-(z * z) / 2);
  return z >= 0 ? (1 + y) / 2 : (1 - y) / 2;
}
