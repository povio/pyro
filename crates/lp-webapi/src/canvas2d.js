// Geometry Interfaces (DOMPointReadOnly, DOMPoint, DOMMatrixReadOnly,
// DOMMatrix) and the canvas 2D value types (Path2D, ImageData, CanvasGradient,
// CanvasPattern, TextMetrics) for one realm, made on first use (canvas.rs).
// Path2D records its commands; canvas.rs builds the geometry from them
// (`pathOps`) for isPointInPath/isPointInStroke, so one implementation of the
// path rules serves the context's current path and Path2D.
(function (natives) {
  'use strict';
  const secret = Symbol('lightningpanda');
  const TypeErrorC = TypeError;
  const RangeErrorC = RangeError;
  const DOMExceptionC = DOMException;
  const NumberC = Number;
  const MathC = Math;
  const isFinite = NumberC.isFinite;
  const ArrayIsArray = Array.isArray;
  const Float32ArrayC = Float32Array;
  const Float64ArrayC = Float64Array;
  const Uint8ClampedArrayC = Uint8ClampedArray;
  const defineProperty = Object.defineProperty;
  const getOwnPropertyNames = Object.getOwnPropertyNames;
  const getOwnPropertyDescriptor = Object.getOwnPropertyDescriptor;
  const parseColor = natives.parseColor;

  function illegal() { return new TypeErrorC('Illegal constructor'); }
  function domError(name, message) { return new DOMExceptionC(message, name); }
  // Web IDL `double`: a TypeError for NaN and the infinities.
  function finite(value, what) {
    const n = +value;
    if (!isFinite(n)) throw new TypeErrorC(`${what} is non-finite.`);
    return n;
  }
  function unrestricted(value, fallback) { return value === undefined ? fallback : +value; }
  // Web IDL members are enumerable; a class's are not.
  function webidl(C, tag) {
    for (const proto of [C.prototype, C]) {
      for (const name of getOwnPropertyNames(proto)) {
        if (name === 'constructor' || name === 'prototype' || name === 'length' || name === 'name') continue;
        const desc = getOwnPropertyDescriptor(proto, name);
        desc.enumerable = true;
        defineProperty(proto, name, desc);
      }
    }
    defineProperty(C.prototype, Symbol.toStringTag, { value: tag, configurable: true });
  }

  // ---- DOMPointReadOnly / DOMPoint -------------------------------------
  const pointState = new WeakMap();
  function pt(self) {
    const p = pointState.get(self);
    if (!p) throw new TypeErrorC('Illegal invocation');
    return p;
  }
  class DOMPointReadOnly {
    constructor(x = 0, y = 0, z = 0, w = 1) {
      pointState.set(this, { x: +x, y: +y, z: +z, w: +w });
    }
    static fromPoint(other) { return pointFrom(DOMPointReadOnly, other); }
    get x() { return pt(this).x; }
    get y() { return pt(this).y; }
    get z() { return pt(this).z; }
    get w() { return pt(this).w; }
    matrixTransform(matrix) {
      const m = matrixFromInit(matrix);
      const p = pt(this);
      return transformPoint(m, p.x, p.y, p.z, p.w);
    }
    toJSON() { const p = pt(this); return { x: p.x, y: p.y, z: p.z, w: p.w }; }
  }
  class DOMPoint extends DOMPointReadOnly {
    static fromPoint(other) { return pointFrom(DOMPoint, other); }
    get x() { return pt(this).x; }
    set x(v) { pt(this).x = +v; }
    get y() { return pt(this).y; }
    set y(v) { pt(this).y = +v; }
    get z() { return pt(this).z; }
    set z(v) { pt(this).z = +v; }
    get w() { return pt(this).w; }
    set w(v) { pt(this).w = +v; }
  }
  function dict(value) {
    if (value === undefined || value === null) return {};
    if (typeof value !== 'object' && typeof value !== 'function') throw new TypeErrorC('The provided value is not an object.');
    return value;
  }
  function pointFrom(C, other) {
    const o = dict(other);
    return new C(unrestricted(o.x, 0), unrestricted(o.y, 0), unrestricted(o.z, 0), unrestricted(o.w, 1));
  }
  function transformPoint(m, x, y, z, w) {
    return new DOMPoint(
      m[0] * x + m[4] * y + m[8] * z + m[12] * w,
      m[1] * x + m[5] * y + m[9] * z + m[13] * w,
      m[2] * x + m[6] * y + m[10] * z + m[14] * w,
      m[3] * x + m[7] * y + m[11] * z + m[15] * w,
    );
  }

  // ---- DOMMatrixReadOnly / DOMMatrix -----------------------------------
  // Each matrix keeps m11..m44 in that order (`m[(i - 1) * 4 + (j - 1)]` is
  // mij): a = m11, b = m12, c = m21, d = m22, e = m41, f = m42.
  const matrixState = new WeakMap();
  function st(self) {
    const s = matrixState.get(self);
    if (!s) throw new TypeErrorC('Illegal invocation');
    return s;
  }
  const IDENTITY = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
  function make(C, m, is2D) {
    const o = Object.create(C.prototype);
    matrixState.set(o, { m: m.slice(), is2D });
    return o;
  }
  // C = A · B: the transform that applies B, then A.
  function multiply(a, b) {
    const c = new Array(16);
    for (let i = 0; i < 4; i++) {
      for (let j = 0; j < 4; j++) {
        let sum = 0;
        for (let k = 0; k < 4; k++) sum += b[i * 4 + k] * a[k * 4 + j];
        c[i * 4 + j] = sum;
      }
    }
    return c;
  }
  function from2D(a, b, c, d, e, f) { return [a, b, 0, 0, c, d, 0, 0, 0, 0, 1, 0, e, f, 0, 1]; }
  function translation(x, y, z) { return [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, x, y, z, 1]; }
  function scaling(x, y, z) { return [x, 0, 0, 0, 0, y, 0, 0, 0, 0, z, 0, 0, 0, 0, 1]; }
  const RAD = MathC.PI / 180;
  function axisRotation(x, y, z, degrees) {
    const len = MathC.hypot(x, y, z);
    if (len === 0) return IDENTITY.slice();
    x /= len; y /= len; z /= len;
    const angle = degrees * RAD;
    // Rotations about a major axis use sin and cos directly, as Chrome does.
    const cos = MathC.cos(angle), sin = MathC.sin(angle);
    if (x === 0 && y === 0) {
      const s = z > 0 ? sin : -sin;
      return [cos, s, 0, 0, -s, cos, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
    }
    if (y === 0 && z === 0) {
      const s = x > 0 ? sin : -sin;
      return [1, 0, 0, 0, 0, cos, s, 0, 0, -s, cos, 0, 0, 0, 0, 1];
    }
    if (x === 0 && z === 0) {
      const s = y > 0 ? sin : -sin;
      return [cos, 0, -s, 0, 0, 1, 0, 0, s, 0, cos, 0, 0, 0, 0, 1];
    }
    const sc = MathC.sin(angle / 2) * MathC.cos(angle / 2);
    const sq = MathC.sin(angle / 2) ** 2;
    return [
      1 - 2 * (y * y + z * z) * sq, 2 * (x * y * sq + z * sc), 2 * (x * z * sq - y * sc), 0,
      2 * (x * y * sq - z * sc), 1 - 2 * (x * x + z * z) * sq, 2 * (y * z * sq + x * sc), 0,
      2 * (x * z * sq + y * sc), 2 * (y * z * sq - x * sc), 1 - 2 * (x * x + y * y) * sq, 0,
      0, 0, 0, 1,
    ];
  }
  function invert(m) {
    const [a00, a01, a02, a03, a10, a11, a12, a13, a20, a21, a22, a23, a30, a31, a32, a33] = m;
    const b00 = a00 * a11 - a01 * a10, b01 = a00 * a12 - a02 * a10, b02 = a00 * a13 - a03 * a10;
    const b03 = a01 * a12 - a02 * a11, b04 = a01 * a13 - a03 * a11, b05 = a02 * a13 - a03 * a12;
    const b06 = a20 * a31 - a21 * a30, b07 = a20 * a32 - a22 * a30, b08 = a20 * a33 - a23 * a30;
    const b09 = a21 * a32 - a22 * a31, b10 = a21 * a33 - a23 * a31, b11 = a22 * a33 - a23 * a32;
    const det = b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06;
    if (det === 0 || !isFinite(det)) return null;
    const k = 1 / det;
    return [
      (a11 * b11 - a12 * b10 + a13 * b09) * k, (a02 * b10 - a01 * b11 - a03 * b09) * k,
      (a31 * b05 - a32 * b04 + a33 * b03) * k, (a22 * b04 - a21 * b05 - a23 * b03) * k,
      (a12 * b08 - a10 * b11 - a13 * b07) * k, (a00 * b11 - a02 * b08 + a03 * b07) * k,
      (a32 * b02 - a30 * b05 - a33 * b01) * k, (a20 * b05 - a22 * b02 + a23 * b01) * k,
      (a10 * b10 - a11 * b08 + a13 * b06) * k, (a01 * b08 - a00 * b10 - a03 * b06) * k,
      (a30 * b04 - a31 * b02 + a33 * b00) * k, (a21 * b02 - a20 * b04 - a23 * b00) * k,
      (a11 * b07 - a10 * b09 - a12 * b06) * k, (a00 * b09 - a01 * b07 + a02 * b06) * k,
      (a31 * b01 - a30 * b03 - a32 * b00) * k, (a20 * b03 - a21 * b01 + a22 * b00) * k,
    ];
  }
  const NAMES_3D = ['m13', 'm14', 'm23', 'm24', 'm31', 'm32', 'm34', 'm43'];
  // DOMMatrixInit "validate and fixup", then the matrix it describes.
  function fixup(init, only2D) {
    const o = dict(init);
    const pairs = [['a', 'm11', 0], ['b', 'm12', 1], ['c', 'm21', 4], ['d', 'm22', 5], ['e', 'm41', 12], ['f', 'm42', 13]];
    const m = IDENTITY.slice();
    for (const [short, long, index] of pairs) {
      const s = o[short] === undefined ? undefined : +o[short];
      const l = o[long] === undefined ? undefined : +o[long];
      if (s !== undefined && l !== undefined && !(s === l || (s !== s && l !== l))) {
        throw new TypeErrorC(`The '${short}' property should equal the '${long}' property.`);
      }
      const v = l !== undefined ? l : s;
      if (v !== undefined) m[index] = v;
    }
    let is2D = o.is2D === undefined ? undefined : !!o.is2D;
    if (only2D) return { m, is2D: true };
    const index3D = { m13: 2, m14: 3, m23: 6, m24: 7, m31: 8, m32: 9, m34: 11, m43: 14 };
    let flat = true;
    for (const name of NAMES_3D) {
      if (o[name] !== undefined) {
        const v = +o[name];
        m[index3D[name]] = v;
        if (v !== 0) flat = false;
      }
    }
    for (const [name, index] of [['m33', 10], ['m44', 15]]) {
      if (o[name] !== undefined) {
        const v = +o[name];
        m[index] = v;
        if (v !== 1) flat = false;
      }
    }
    if (is2D === true && !flat) throw new TypeErrorC('The is2D member is set to true but the input matrix is a 3d matrix.');
    if (is2D === undefined) is2D = flat;
    return { m, is2D };
  }
  function matrixFromInit(init) {
    if (matrixState.has(init)) return matrixState.get(init).m;
    return fixup(init, false).m;
  }

  // CSS <transform-list> (absolute lengths and angles only).
  const LENGTH = { px: 1, cm: 96 / 2.54, mm: 96 / 25.4, q: 96 / 101.6, in: 96, pt: 4 / 3, pc: 16 };
  const ANGLE = { deg: 1, rad: 180 / MathC.PI, grad: 0.9, turn: 360 };
  function parseTransform(text) {
    const syntax = () => domError('SyntaxError', `Failed to parse '${text}'.`);
    const source = String(text).trim();
    if (source === '' || source.toLowerCase() === 'none') return { m: IDENTITY.slice(), is2D: true };
    const fn = /^([a-zA-Z0-9]+)\(([^()]*)\)\s*/;
    let rest = source, m = IDENTITY.slice(), is2D = true;
    while (rest.length) {
      const match = fn.exec(rest);
      if (!match) throw syntax();
      rest = rest.slice(match[0].length);
      const name = match[1].toLowerCase();
      const args = match[2].trim() === '' ? [] : match[2].split(',').map((a) => a.trim());
      const value = (arg, kind) => {
        const unit = /^([+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?)([a-z%]*)$/i.exec(arg);
        if (!unit) throw syntax();
        const n = +unit[1], u = unit[2].toLowerCase();
        if (kind === 'number') { if (u) throw syntax(); return n; }
        if (kind === 'length') { if (u === '' && n === 0) return 0; if (!(u in LENGTH)) throw syntax(); return n * LENGTH[u]; }
        if (u === '' && n === 0) return 0;
        if (!(u in ANGLE)) throw syntax();
        return n * ANGLE[u];
      };
      const need = (min, max) => { if (args.length < min || args.length > max) throw syntax(); };
      let t, flat = true;
      switch (name) {
        case 'matrix': need(6, 6); t = from2D(...args.map((a) => value(a, 'number'))); break;
        case 'matrix3d': need(16, 16); t = args.map((a) => value(a, 'number')); flat = false; break;
        case 'translate': need(1, 2); t = translation(value(args[0], 'length'), args[1] ? value(args[1], 'length') : 0, 0); break;
        case 'translatex': need(1, 1); t = translation(value(args[0], 'length'), 0, 0); break;
        case 'translatey': need(1, 1); t = translation(0, value(args[0], 'length'), 0); break;
        case 'translatez': need(1, 1); t = translation(0, 0, value(args[0], 'length')); flat = false; break;
        case 'translate3d': need(3, 3); t = translation(value(args[0], 'length'), value(args[1], 'length'), value(args[2], 'length')); flat = false; break;
        case 'scale': { need(1, 2); const x = value(args[0], 'number'); t = scaling(x, args[1] ? value(args[1], 'number') : x, 1); break; }
        case 'scalex': need(1, 1); t = scaling(value(args[0], 'number'), 1, 1); break;
        case 'scaley': need(1, 1); t = scaling(1, value(args[0], 'number'), 1); break;
        case 'scalez': need(1, 1); t = scaling(1, 1, value(args[0], 'number')); flat = false; break;
        case 'scale3d': need(3, 3); t = scaling(value(args[0], 'number'), value(args[1], 'number'), value(args[2], 'number')); flat = false; break;
        case 'rotate': case 'rotatez': need(1, 1); t = axisRotation(0, 0, 1, value(args[0], 'angle')); flat = name === 'rotate'; break;
        case 'rotatex': need(1, 1); t = axisRotation(1, 0, 0, value(args[0], 'angle')); flat = false; break;
        case 'rotatey': need(1, 1); t = axisRotation(0, 1, 0, value(args[0], 'angle')); flat = false; break;
        case 'rotate3d': need(4, 4); t = axisRotation(value(args[0], 'number'), value(args[1], 'number'), value(args[2], 'number'), value(args[3], 'angle')); flat = false; break;
        case 'skew': { need(1, 2); const ax = value(args[0], 'angle'), ay = args[1] ? value(args[1], 'angle') : 0; t = from2D(1, MathC.tan(ay * RAD), MathC.tan(ax * RAD), 1, 0, 0); break; }
        case 'skewx': need(1, 1); t = from2D(1, 0, MathC.tan(value(args[0], 'angle') * RAD), 1, 0, 0); break;
        case 'skewy': need(1, 1); t = from2D(1, MathC.tan(value(args[0], 'angle') * RAD), 0, 1, 0, 0); break;
        case 'perspective': { need(1, 1); const d = value(args[0], 'length'); t = IDENTITY.slice(); if (d !== 0) t[11] = -1 / d; flat = false; break; }
        default: throw syntax();
      }
      if (!flat) is2D = false;
      m = multiply(m, t);
    }
    return { m, is2D };
  }
  function initToState(init) {
    if (init === undefined) return { m: IDENTITY.slice(), is2D: true };
    if (typeof init === 'string') return parseTransform(init);
    if (init !== null && typeof init === 'object' && typeof init[Symbol.iterator] === 'function') {
      const values = Array.from(init, (v) => +v);
      if (values.length === 6) return { m: from2D(...values), is2D: true };
      if (values.length === 16) return { m: values, is2D: false };
      throw new TypeErrorC(`The sequence must contain 6 elements for a 2D matrix or 16 elements for a 3D matrix.`);
    }
    return parseTransform(String(init));
  }

  function number(v, fallback) { return v === undefined ? fallback : +v; }
  class DOMMatrixReadOnly {
    constructor(init) {
      const s = initToState(init);
      matrixState.set(this, s);
    }
    static fromMatrix(other) { const s = fixup(other, false); return make(DOMMatrixReadOnly, s.m, s.is2D); }
    static fromFloat32Array(array) { return fromArray(DOMMatrixReadOnly, array, Float32ArrayC); }
    static fromFloat64Array(array) { return fromArray(DOMMatrixReadOnly, array, Float64ArrayC); }
    get a() { return st(this).m[0]; }
    get b() { return st(this).m[1]; }
    get c() { return st(this).m[4]; }
    get d() { return st(this).m[5]; }
    get e() { return st(this).m[12]; }
    get f() { return st(this).m[13]; }
    get is2D() { return st(this).is2D; }
    get isIdentity() { const m = st(this).m; return m.every((v, i) => v === IDENTITY[i]); }
    translate(tx, ty, tz) { return copy(this).translateSelf(tx, ty, tz); }
    scale(sx, sy, sz, ox, oy, oz) { return copy(this).scaleSelf(sx, sy, sz, ox, oy, oz); }
    scaleNonUniform(sx = 1, sy = 1) { return copy(this).scaleSelf(sx, sy, 1, 0, 0, 0); }
    scale3d(scale, ox, oy, oz) { return copy(this).scale3dSelf(scale, ox, oy, oz); }
    rotate(rx, ry, rz) { return copy(this).rotateSelf(rx, ry, rz); }
    rotateFromVector(x, y) { return copy(this).rotateFromVectorSelf(x, y); }
    rotateAxisAngle(x, y, z, angle) { return copy(this).rotateAxisAngleSelf(x, y, z, angle); }
    skewX(sx) { return copy(this).skewXSelf(sx); }
    skewY(sy) { return copy(this).skewYSelf(sy); }
    multiply(other) { return copy(this).multiplySelf(other); }
    flipX() { const s = st(this); return make(DOMMatrix, multiply(s.m, scaling(-1, 1, 1)), s.is2D); }
    flipY() { const s = st(this); return make(DOMMatrix, multiply(s.m, scaling(1, -1, 1)), s.is2D); }
    inverse() { return copy(this).invertSelf(); }
    transformPoint(point) {
      const o = dict(point);
      return transformPoint(st(this).m, number(o.x, 0), number(o.y, 0), number(o.z, 0), number(o.w, 1));
    }
    toFloat32Array() { return new Float32ArrayC(st(this).m); }
    toFloat64Array() { return new Float64ArrayC(st(this).m); }
    toJSON() {
      const json = {};
      for (const name of MATRIX_ATTRIBUTES) json[name] = this[name];
      return json;
    }
    toString() {
      const s = st(this);
      if (!s.m.every(isFinite)) throw domError('InvalidStateError', 'Cannot be serialized with NaN or Infinity values.');
      if (s.is2D) return `matrix(${[s.m[0], s.m[1], s.m[4], s.m[5], s.m[12], s.m[13]].join(', ')})`;
      return `matrix3d(${s.m.join(', ')})`;
    }
  }
  const MATRIX_ATTRIBUTES = ['a', 'b', 'c', 'd', 'e', 'f'];
  const LONG = ['m11', 'm12', 'm13', 'm14', 'm21', 'm22', 'm23', 'm24', 'm31', 'm32', 'm33', 'm34', 'm41', 'm42', 'm43', 'm44'];
  LONG.forEach((name, index) => {
    defineProperty(DOMMatrixReadOnly.prototype, name, {
      get() { return st(this).m[index]; }, enumerable: true, configurable: true,
    });
  });
  MATRIX_ATTRIBUTES.push(...LONG, 'is2D', 'isIdentity');
  function fromArray(C, array, ArrayType) {
    if (!(array instanceof ArrayType)) throw new TypeErrorC(`The provided value is not of type '${ArrayType.name}'.`);
    if (array.length === 6) return make(C, from2D(...array), true);
    if (array.length === 16) return make(C, Array.from(array), false);
    throw new TypeErrorC('The sequence must contain 6 elements for a 2D matrix or 16 elements for a 3D matrix.');
  }
  function copy(self) { const s = st(self); return make(DOMMatrix, s.m, s.is2D); }

  class DOMMatrix extends DOMMatrixReadOnly {
    static fromMatrix(other) { const s = fixup(other, false); return make(DOMMatrix, s.m, s.is2D); }
    static fromFloat32Array(array) { return fromArray(DOMMatrix, array, Float32ArrayC); }
    static fromFloat64Array(array) { return fromArray(DOMMatrix, array, Float64ArrayC); }
    multiplySelf(other) {
      const s = st(this);
      const o = matrixState.has(other) ? matrixState.get(other) : fixup(other, false);
      s.m = multiply(s.m, o.m);
      if (!o.is2D) s.is2D = false;
      return this;
    }
    preMultiplySelf(other) {
      const s = st(this);
      const o = matrixState.has(other) ? matrixState.get(other) : fixup(other, false);
      s.m = multiply(o.m, s.m);
      if (!o.is2D) s.is2D = false;
      return this;
    }
    translateSelf(tx = 0, ty = 0, tz = 0) {
      const s = st(this);
      tx = +tx; ty = +ty; tz = +tz;
      s.m = multiply(s.m, translation(tx, ty, tz));
      if (tz !== 0) s.is2D = false;
      return this;
    }
    scaleSelf(sx = 1, sy, sz = 1, ox = 0, oy = 0, oz = 0) {
      const s = st(this);
      sx = +sx; sy = sy === undefined ? sx : +sy; sz = +sz; ox = +ox; oy = +oy; oz = +oz;
      s.m = multiply(multiply(multiply(s.m, translation(ox, oy, oz)), scaling(sx, sy, sz)), translation(-ox, -oy, -oz));
      if (sz !== 1 || oz !== 0) s.is2D = false;
      return this;
    }
    scale3dSelf(scale = 1, ox = 0, oy = 0, oz = 0) {
      const s = st(this);
      scale = +scale; ox = +ox; oy = +oy; oz = +oz;
      s.m = multiply(multiply(multiply(s.m, translation(ox, oy, oz)), scaling(scale, scale, scale)), translation(-ox, -oy, -oz));
      if (scale !== 1) s.is2D = false;
      return this;
    }
    rotateSelf(rx = 0, ry, rz) {
      const s = st(this);
      rx = +rx;
      if (ry === undefined && rz === undefined) { rz = rx; rx = 0; ry = 0; }
      ry = ry === undefined ? 0 : +ry; rz = rz === undefined ? 0 : +rz;
      if (rx !== 0 || ry !== 0) s.is2D = false;
      s.m = multiply(multiply(multiply(s.m, axisRotation(0, 0, 1, rz)), axisRotation(0, 1, 0, ry)), axisRotation(1, 0, 0, rx));
      return this;
    }
    rotateFromVectorSelf(x = 0, y = 0) {
      const s = st(this);
      x = +x; y = +y;
      const angle = x === 0 && y === 0 ? 0 : MathC.atan2(y, x) / RAD;
      s.m = multiply(s.m, axisRotation(0, 0, 1, angle));
      return this;
    }
    rotateAxisAngleSelf(x = 0, y = 0, z = 0, angle = 0) {
      const s = st(this);
      x = +x; y = +y; z = +z;
      s.m = multiply(s.m, axisRotation(x, y, z, +angle));
      if (x !== 0 || y !== 0) s.is2D = false;
      return this;
    }
    skewXSelf(sx = 0) {
      const s = st(this);
      s.m = multiply(s.m, from2D(1, 0, MathC.tan(+sx * RAD), 1, 0, 0));
      return this;
    }
    skewYSelf(sy = 0) {
      const s = st(this);
      s.m = multiply(s.m, from2D(1, MathC.tan(+sy * RAD), 0, 1, 0, 0));
      return this;
    }
    invertSelf() {
      const s = st(this);
      const inverse = invert(s.m);
      if (inverse) s.m = inverse;
      else { s.m = s.m.map(() => NaN); s.is2D = false; }
      return this;
    }
    setMatrixValue(text) {
      const parsed = parseTransform(String(text));
      const s = st(this);
      s.m = parsed.m; s.is2D = parsed.is2D;
      return this;
    }
  }
  const SHORT_INDEX = { a: 0, b: 1, c: 4, d: 5, e: 12, f: 13 };
  for (const name of Object.keys(SHORT_INDEX)) {
    const index = SHORT_INDEX[name];
    defineProperty(DOMMatrix.prototype, name, {
      get() { return st(this).m[index]; },
      set(v) { st(this).m[index] = +v; },
      enumerable: true, configurable: true,
    });
  }
  LONG.forEach((name, index) => {
    const flatValue = index === 10 || index === 15 ? 1 : 0;
    const is3D = NAMES_3D.includes(name) || index === 10 || index === 15;
    defineProperty(DOMMatrix.prototype, name, {
      get() { return st(this).m[index]; },
      set(v) {
        const s = st(this);
        s.m[index] = +v;
        if (is3D && +v !== flatValue) s.is2D = false;
      },
      enumerable: true, configurable: true,
    });
  });
  webidl(DOMPointReadOnly, 'DOMPointReadOnly');
  webidl(DOMPoint, 'DOMPoint');
  webidl(DOMMatrixReadOnly, 'DOMMatrixReadOnly');
  webidl(DOMMatrix, 'DOMMatrix');

  // ---- Path2D ----------------------------------------------------------
  // Opcodes (canvas.rs `replay`): 0 moveTo x y, 1 lineTo x y, 2 quadraticCurveTo,
  // 3 bezierCurveTo, 4 arcTo x1 y1 x2 y2 r, 5 arc x y r start end ccw,
  // 6 ellipse x y rx ry rotation start end ccw, 7 rect x y w h, 8 closePath,
  // 9 roundRect x y w h + 8 radii, 10 push transform a b c d e f, 11 pop
  // transform, 12 SVG path data (a string).
  const paths = new WeakMap();
  function ops(self) {
    const o = paths.get(self);
    if (!o) throw new TypeErrorC('Illegal invocation');
    return o;
  }
  function allFinite(args) {
    for (let i = 0; i < args.length; i++) if (!isFinite(args[i])) return false;
    return true;
  }
  function record(self, op, args) {
    const o = ops(self);
    if (!allFinite(args)) return;
    o.push(op, ...args);
  }
  class Path2D {
    constructor(path) {
      const o = [];
      paths.set(this, o);
      if (path === undefined) return;
      if (paths.has(path)) o.push(...paths.get(path));
      else o.push(12, String(path));
    }
    addPath(path, transform) {
      const own = ops(this);
      const other = paths.get(path);
      if (!other) throw new TypeErrorC("Failed to execute 'addPath' on 'Path2D': parameter 1 is not of type 'Path2D'.");
      const m = fixup(transform, true).m;
      const t = [m[0], m[1], m[4], m[5], m[12], m[13]];
      if (!allFinite(t)) return;
      own.push(10, ...t, ...other, 11);
    }
    closePath() { ops(this).push(8); }
    moveTo(x, y) { record(this, 0, [+x, +y]); }
    lineTo(x, y) { record(this, 1, [+x, +y]); }
    quadraticCurveTo(cpx, cpy, x, y) { record(this, 2, [+cpx, +cpy, +x, +y]); }
    bezierCurveTo(c1x, c1y, c2x, c2y, x, y) { record(this, 3, [+c1x, +c1y, +c2x, +c2y, +x, +y]); }
    arcTo(x1, y1, x2, y2, r) {
      const args = [+x1, +y1, +x2, +y2, +r];
      if (!allFinite(args)) return;
      if (args[4] < 0) throw domError('IndexSizeError', `The radius provided (${args[4]}) is negative.`);
      record(this, 4, args);
    }
    arc(x, y, r, start, end, ccw = false) {
      const args = [+x, +y, +r, +start, +end];
      if (!allFinite(args)) return;
      if (args[2] < 0) throw domError('IndexSizeError', `The radius provided (${args[2]}) is negative.`);
      record(this, 5, [...args, ccw ? 1 : 0]);
    }
    ellipse(x, y, rx, ry, rotation, start, end, ccw = false) {
      const args = [+x, +y, +rx, +ry, +rotation, +start, +end];
      if (!allFinite(args)) return;
      if (args[2] < 0) throw domError('IndexSizeError', `The major-axis radius provided (${args[2]}) is negative.`);
      if (args[3] < 0) throw domError('IndexSizeError', `The minor-axis radius provided (${args[3]}) is negative.`);
      record(this, 6, [...args, ccw ? 1 : 0]);
    }
    rect(x, y, w, h) { record(this, 7, [+x, +y, +w, +h]); }
    roundRect(x, y, w, h, radii = 0) {
      const args = [+x, +y, +w, +h];
      const r = roundRectRadii(radii);
      if (!allFinite(args) || r === null) return;
      record(this, 9, [...args, ...r]);
    }
  }
  webidl(Path2D, 'Path2D');
  // CanvasPath roundRect(): the radii as [tl.x, tl.y, tr.x, tr.y, br.x, br.y,
  // bl.x, bl.y], or null when one is not finite (nothing is drawn).
  function roundRectRadii(radii) {
    const list = radii !== null && typeof radii === 'object' && typeof radii[Symbol.iterator] === 'function'
      ? Array.from(radii) : [radii];
    if (list.length < 1 || list.length > 4) {
      throw new RangeErrorC(`${list.length} radii provided. Between one and four radii are necessary.`);
    }
    const normalized = [];
    for (const r of list) {
      let x, y;
      if (r !== null && typeof r === 'object') { x = number(r.x, 0); y = number(r.y, 0); } else { x = y = +r; }
      if (!isFinite(x) || !isFinite(y)) return null;
      if (x < 0 || y < 0) throw new RangeErrorC(`Radius value ${x < 0 ? x : y} is negative.`);
      normalized.push([x, y]);
    }
    const [a, b = a, c = a, d = b] = normalized;
    const corners = list.length === 1 ? [a, a, a, a] : list.length === 2 ? [a, b, a, b] : list.length === 3 ? [a, b, c, b] : [a, b, c, d];
    return [].concat(...corners);
  }

  // ---- ImageData -------------------------------------------------------
  const images = new WeakMap();
  function img(self) {
    const s = images.get(self);
    if (!s) throw new TypeErrorC('Illegal invocation');
    return s;
  }
  const COLOR_SPACES = ['srgb', 'display-p3'];
  function colorSpace(settings) {
    const o = dict(settings);
    if (o.colorSpace === undefined) return 'srgb';
    const space = String(o.colorSpace);
    if (!COLOR_SPACES.includes(space)) throw new TypeErrorC(`The provided value '${space}' is not a valid enum value of type PredefinedColorSpace.`);
    return space;
  }
  function dimension(value, what) {
    const n = finite(value, what) >>> 0;
    return n;
  }
  const MAX_PIXELS = 0x10000000;
  class ImageData {
    constructor(a, b, c, d) {
      if (arguments.length < 2) throw new TypeErrorC(`Failed to construct 'ImageData': 2 arguments required, but only ${arguments.length} present.`);
      if (a instanceof Uint8ClampedArrayC) {
        const length = a.length;
        if (length === 0 || length % 4 !== 0) throw domError('InvalidStateError', 'The input data length is not a multiple of 4.');
        const width = dimension(b, 'width');
        if (width === 0) throw domError('IndexSizeError', 'The source width is zero or not a number.');
        if ((length / 4) % width !== 0) throw domError('IndexSizeError', 'The input data length is not a multiple of (4 * width).');
        const height = length / 4 / width;
        if (c !== undefined && dimension(c, 'height') !== height) throw domError('IndexSizeError', 'The input data length is not equal to (4 * width * height).');
        images.set(this, { width, height, data: a, colorSpace: colorSpace(d) });
        return;
      }
      const width = dimension(a, 'width'), height = dimension(b, 'height');
      if (width === 0) throw domError('IndexSizeError', 'The source width is 0.');
      if (height === 0) throw domError('IndexSizeError', 'The source height is 0.');
      if (width * height > MAX_PIXELS) throw new RangeErrorC('Out of memory at ImageData creation.');
      images.set(this, { width, height, data: new Uint8ClampedArrayC(width * height * 4), colorSpace: colorSpace(c) });
    }
    get width() { return img(this).width; }
    get height() { return img(this).height; }
    get data() { return img(this).data; }
    get colorSpace() { return img(this).colorSpace; }
  }
  webidl(ImageData, 'ImageData');

  // ---- CanvasGradient / CanvasPattern / TextMetrics ----------------------
  const gradients = new WeakMap();
  class CanvasGradient {
    constructor(token, kind, args) {
      if (token !== secret) throw illegal();
      gradients.set(this, { kind, args, stops: [] });
    }
    addColorStop(offset, color) {
      const g = gradients.get(this);
      if (!g) throw new TypeErrorC('Illegal invocation');
      if (arguments.length < 2) throw new TypeErrorC(`Failed to execute 'addColorStop' on 'CanvasGradient': 2 arguments required, but only ${arguments.length} present.`);
      const o = finite(offset, "The provided double value");
      if (o < 0 || o > 1) throw domError('IndexSizeError', `The provided value (${o}) is outside the range (0.0, 1.0).`);
      const parsed = parseColor(String(color));
      if (parsed === null) throw domError('SyntaxError', `The value provided ('${color}') could not be parsed as a color.`);
      g.stops.push([o, parsed]);
    }
  }
  webidl(CanvasGradient, 'CanvasGradient');
  const patterns = new WeakMap();
  class CanvasPattern {
    constructor(token, repetition) {
      if (token !== secret) throw illegal();
      patterns.set(this, { repetition, transform: [1, 0, 0, 1, 0, 0] });
    }
    setTransform(transform) {
      const p = patterns.get(this);
      if (!p) throw new TypeErrorC('Illegal invocation');
      const m = fixup(transform, true).m;
      p.transform = [m[0], m[1], m[4], m[5], m[12], m[13]];
    }
  }
  webidl(CanvasPattern, 'CanvasPattern');
  const metrics = new WeakMap();
  const METRICS = [
    'width', 'actualBoundingBoxLeft', 'actualBoundingBoxRight', 'fontBoundingBoxAscent', 'fontBoundingBoxDescent',
    'actualBoundingBoxAscent', 'actualBoundingBoxDescent', 'emHeightAscent', 'emHeightDescent', 'hangingBaseline',
    'alphabeticBaseline', 'ideographicBaseline',
  ];
  class TextMetrics {
    constructor(token, values) {
      if (token !== secret) throw illegal();
      metrics.set(this, values);
    }
  }
  METRICS.forEach((name, index) => {
    defineProperty(TextMetrics.prototype, name, {
      get() {
        const values = metrics.get(this);
        if (!values) throw new TypeErrorC('Illegal invocation');
        return values[index];
      },
      enumerable: true, configurable: true,
    });
  });
  webidl(TextMetrics, 'TextMetrics');

  return {
    DOMPointReadOnly, DOMPoint, DOMMatrixReadOnly, DOMMatrix, WebKitCSSMatrix: DOMMatrix,
    Path2D, ImageData, CanvasGradient, CanvasPattern, TextMetrics,
    // Helpers for canvas.rs.
    gradient(kind, args) { return new CanvasGradient(secret, kind, args); },
    pattern(repetition) { return new CanvasPattern(secret, repetition); },
    textMetrics(values) { return new TextMetrics(secret, values); },
    imageData(width, height, settings) { return new ImageData(width, height, settings); },
    // [a, b, c, d, e, f] of a DOMMatrix2DInit (validated), for setTransform.
    matrix2D(init) {
      const m = matrixState.has(init) ? matrixState.get(init).m : fixup(init, true).m;
      return [m[0], m[1], m[4], m[5], m[12], m[13]];
    },
    matrix(a, b, c, d, e, f) { return make(DOMMatrix, from2D(a, b, c, d, e, f), true); },
    pathOps(value) { return paths.get(value) || null; },
    isFillStyle(value) { return gradients.has(value) || patterns.has(value); },
    imageSize(value) { const s = images.get(value); return s ? [s.width, s.height, s.colorSpace] : null; },
    roundRectRadii,
  };
})
