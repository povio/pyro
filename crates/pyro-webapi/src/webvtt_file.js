// Private WebVTT file parser. The resource loader supplies UTF-8 decoded text.
// The track owns cue ordering and turns region indices into shared VTTRegions.
(function parseWebVTTFile(text) {
  const lines = text.replace(/^\uFEFF/, '').replace(/\0/g, '\uFFFD').replace(/\r\n?/g, '\n').split('\n');
  if (!/^WEBVTT(?:[ \t]|$)/.test(lines[0])) return null;

  const regions = [], cues = [], styles = [];
  const regionIds = new Map();

  function percentage(value) {
    if (!/^\d+(?:\.\d+)?%$/.test(value)) return null;
    const number = Number(value.slice(0, -1));
    return number <= 100 ? number : null;
  }
  function settings(input, apply) {
    for (const token of input.split(/[ \t\n\f]+/)) {
      const colon = token.indexOf(':');
      if (colon > 0 && colon + 1 < token.length) apply(token.slice(0, colon), token.slice(colon + 1));
    }
  }
  function addRegion(body) {
    const region = {id: '', width: 100, lines: 3, regionAnchorX: 0, regionAnchorY: 100,
      viewportAnchorX: 0, viewportAnchorY: 100, scroll: ''};
    settings(body, (name, value) => {
      if (name === 'id') region.id = value;
      else if (name === 'scroll' && value === 'up') region.scroll = value;
      else if (name === 'width') {
        const n = percentage(value);
        if (n !== null) region.width = n;
      } else if (name === 'lines' && /^\d+$/.test(value)) {
        region.lines = Math.min(Number(value), 0xffffffff);
      } else if (name === 'regionanchor' || name === 'viewportanchor') {
        const parts = value.split(',');
        if (parts.length !== 2) return;
        const x = percentage(parts[0]), y = percentage(parts[1]);
        if (x === null || y === null) return;
        const prefix = name === 'regionanchor' ? 'regionAnchor' : 'viewportAnchor';
        region[prefix + 'X'] = x;
        region[prefix + 'Y'] = y;
      }
    });
    regionIds.set(region.id, regions.length);
    regions.push(Object.freeze(region));
  }
  function cueSettings(input) {
    const result = {};
    settings(input, (name, value) => {
      if (name === 'region') {
        if (regionIds.has(value)) result.region = regionIds.get(value);
        else delete result.region;
      } else if (name === 'vertical' && ['rl', 'lr'].includes(value)) {
        result.vertical = value;
        delete result.region;
      } else if (name === 'align' && ['start', 'center', 'end', 'left', 'right'].includes(value)) {
        result.align = value;
      } else if (name === 'size') {
        const number = percentage(value);
        if (number === null) return;
        result.size = number;
        if (number !== 100) delete result.region;
      } else if (name === 'line' || name === 'position') {
        const parts = value.split(',');
        const [numberText, alignment] = parts;
        const isLine = name === 'line';
        const valid = isLine ? ['start', 'center', 'end'] : ['line-left', 'center', 'line-right'];
        if (parts.length > 2 || (alignment !== undefined && !valid.includes(alignment))) return;
        const snap = isLine && !numberText.endsWith('%');
        if (snap && !/^-?\d+(?:\.\d+)?$/.test(numberText)) return;
        const number = snap ? Number(numberText) : percentage(numberText);
        if (number === null || !Number.isFinite(number)) return;
        result[name] = number || 0;
        if (alignment !== undefined) result[name + 'Align'] = alignment;
        if (isLine) {
          result.snapToLines = snap;
          delete result.region;
        }
      }
    });
    return Object.freeze(result);
  }
  function timestamp(value) {
    const parts = value.split(':');
    const hours = parts.length === 3 ? Number(parts.shift()) : 0;
    const [minutes, seconds] = parts.map(Number);
    if (minutes >= 60 || seconds >= 60) return null;
    const time = hours * 3600 + minutes * 60 + seconds;
    return Number.isFinite(time) ? time : null;
  }
  const clock = /(?:\d+:)?\d{2}:\d{2}\.\d{3}/.source;
  const timing = new RegExp('^[ \\t\\f]*(' + clock + ')[ \\t\\f]*-->[ \\t\\f]*(' + clock + ')(?![0-9])([\\s\\S]*)$');
  function parseTiming(line, id) {
    const match = timing.exec(line);
    if (!match) return null;
    const startTime = timestamp(match[1]), endTime = timestamp(match[2]);
    if (startTime === null || endTime === null) return null;
    return {id, startTime, endTime, settings: cueSettings(match[3]), text: ''};
  }

  let i = 1;
  // A timing arrow encountered in the header starts a new block even if
  // the author omitted the blank separator line.
  while (i < lines.length && lines[i] !== '' && !lines[i].includes('-->')) i++;
  while (i < lines.length) {
    if (lines[i] === '') { i++; continue; }
    const first = lines[i++];
    let cue = null, region = false, style = false;
    if (first.includes('-->')) cue = parseTiming(first, '');
    else if (i < lines.length && lines[i].includes('-->')) cue = parseTiming(lines[i++], first);
    else if (!cues.length && i < lines.length && lines[i] !== '') {
      region = /^REGION[ \t\f]*$/.test(first);
      style = /^STYLE[ \t\f]*$/.test(first);
    }
    const start = i;
    while (i < lines.length && lines[i] !== '' && !lines[i].includes('-->')) i++;
    if (cue || region || style) {
      const body = lines.slice(start, i).join('\n');
      if (cue) {
        cue.text = body;
        cues.push(Object.freeze(cue));
      } else if (region) addRegion(body);
      else styles.push(body);
    }
  }
  return Object.freeze({regions: Object.freeze(regions), cues: Object.freeze(cues), styles: Object.freeze(styles)});
})
