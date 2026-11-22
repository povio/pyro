// Geometry Interfaces (DOMPointReadOnly, DOMPoint, DOMMatrixReadOnly,
// DOMMatrix) and the canvas 2D value types (Path2D, ImageData, CanvasGradient,
// CanvasPattern, TextMetrics) for one realm, made on first use (canvas.rs).
// Path2D records its commands; canvas.rs builds the geometry from them
// (`pathOps`) for isPointInPath/isPointInStroke, so one implementation of the
// path rules serves the context's current path and Path2D.
(function (natives) {
  'use strict';
  const secret = Symbol('pyro');
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
        if (desc.get) defineProperty(desc.get, 'name', {value: `get ${name}`, configurable: true});
        if (desc.set) defineProperty(desc.set, 'name', {value: `set ${name}`, configurable: true});
        desc.enumerable = true;
        defineProperty(proto, name, desc);
      }
    }
    defineProperty(C.prototype, Symbol.toStringTag, { value: tag, configurable: true });
  }

  // ---- DOMPointReadOnly / DOMPoint -------------------------------------
  const pointState = new WeakMap();
  const mutableGeometry = new WeakSet();
  let rectParts, quadPoints, quadFromPoints;
  function pt(self) {
    const p = pointState.get(self);
    if (!p) throw new TypeErrorC('Illegal invocation');
    return p;
  }
  class DOMPointReadOnly {
    constructor(x = 0, y = 0, z = 0, w = 1) {
      pointState.set(this, { x: +x, y: +y, z: +z, w: +w });
    }
    static fromPoint(other = {}) { return pointFrom(DOMPointReadOnly, other); }
    get x() { return pt(this).x; }
    get y() { return pt(this).y; }
    get z() { return pt(this).z; }
    get w() { return pt(this).w; }
    matrixTransform(matrix = {}) {
      const m = matrixFromInit(matrix);
      const p = pt(this);
      return transformPoint(m, p.x, p.y, p.z, p.w);
    }
    toJSON() { const p = pt(this); return { x: p.x, y: p.y, z: p.z, w: p.w }; }
  }
  class DOMPoint extends DOMPointReadOnly {
    constructor(...args) { super(...args); mutableGeometry.add(this); }
    static fromPoint(other = {}) { return pointFrom(DOMPoint, other); }
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
  function rectFrom(C, other) {
    const source = dict(other);
    const height = unrestricted(source.height, 0), width = unrestricted(source.width, 0);
    return new C(unrestricted(source.x, 0), unrestricted(source.y, 0), width, height);
  }
  let setRect;
  class DOMRectReadOnly {
    #x; #y; #width; #height;
    constructor(x = 0, y = 0, width = 0, height = 0) {
      this.#x = +x; this.#y = +y; this.#width = +width; this.#height = +height;
    }
    static fromRect(other = {}) { return rectFrom(DOMRectReadOnly, other); }
    get x() { return this.#x; }
    get y() { return this.#y; }
    get width() { return this.#width; }
    get height() { return this.#height; }
    get left() { return MathC.min(this.#x, this.#x + this.#width); }
    get right() { return MathC.max(this.#x, this.#x + this.#width); }
    get top() { return MathC.min(this.#y, this.#y + this.#height); }
    get bottom() { return MathC.max(this.#y, this.#y + this.#height); }
    toJSON() {
      return { x: this.#x, y: this.#y, width: this.#width, height: this.#height,
        top: MathC.min(this.#y, this.#y + this.#height), right: MathC.max(this.#x, this.#x + this.#width),
        bottom: MathC.max(this.#y, this.#y + this.#height), left: MathC.min(this.#x, this.#x + this.#width) };
    }
    static { rectParts = o => #x in o ? [o.#x, o.#y, o.#width, o.#height] : null; setRect = (rect, key, value) => {
      switch (key) {
        case 'x': rect.#x = +value; break;
        case 'y': rect.#y = +value; break;
        case 'width': rect.#width = +value; break;
        case 'height': rect.#height = +value; break;
      }
    }; }
  }
  class DOMRect extends DOMRectReadOnly {
    constructor(...args) { super(...args); mutableGeometry.add(this); }
    static fromRect(other = {}) { return rectFrom(DOMRect, other); }
    get x() { return super.x; } set x(value) { setRect(this, 'x', value); }
    get y() { return super.y; } set y(value) { setRect(this, 'y', value); }
    get width() { return super.width; } set width(value) { setRect(this, 'width', value); }
    get height() { return super.height; } set height(value) { setRect(this, 'height', value); }
  }
  class DOMQuad {
    #points;
    static {
      quadPoints = o => #points in o ? o.#points : null;
      quadFromPoints = points => {
        const quad = new DOMQuad(secret);
        quad.#points = points;
        return quad;
      };
    }
    constructor(p1 = {}, p2 = {}, p3 = {}, p4 = {}) {
      if (p1 === secret) return;
      this.#points = [pointFrom(DOMPoint, p1), pointFrom(DOMPoint, p2), pointFrom(DOMPoint, p3), pointFrom(DOMPoint, p4)];
    }
    static fromRect(other = {}) {
      const {x, y, width, height} = rectFrom(DOMRect, other);
      return new DOMQuad({x,y}, {x:x+width,y}, {x:x+width,y:y+height}, {x,y:y+height});
    }
    static fromQuad(other = {}) {
      const source = dict(other);
      return new DOMQuad(source.p1, source.p2, source.p3, source.p4);
    }
    get p1() { return this.#points[0]; } get p2() { return this.#points[1]; }
    get p3() { return this.#points[2]; } get p4() { return this.#points[3]; }
    getBounds() {
      const xs = this.#points.map(point => pt(point).x), ys = this.#points.map(point => pt(point).y);
      const x = MathC.min(...xs), y = MathC.min(...ys);
      return new DOMRect(x, y, MathC.max(...xs) - x, MathC.max(...ys) - y);
    }
    toJSON() { const [p1,p2,p3,p4] = this.#points; return {p1,p2,p3,p4}; }
  }
  class DOMRectList {
    #rects;
    constructor(...args) {
      if (args[0] !== secret) throw illegal();
      this.#rects = args[1];
      this.#rects.forEach((value, index) => defineProperty(this, index, {value, enumerable: true, configurable: true}));
    }
    get length() { return this.#rects.length; }
    item(index) {
      if (!arguments.length) throw new TypeErrorC('An index is required');
      return this.#rects[index >>> 0] ?? null;
    }
  }
  defineProperty(DOMRectList.prototype, Symbol.iterator, {value: Array.prototype.values, writable: true, configurable: true});
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
    if (C === DOMMatrix) mutableGeometry.add(o);
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

  function parseTransform(text) {
    if (natives.worker) throw new TypeErrorC('CSS transform strings require a Window.');
    if (text === '') return { m: IDENTITY.slice(), is2D: true };
    const parsed = natives.parseTransform(text);
    if (!parsed) throw domError('SyntaxError', `Failed to parse '${text}'.`);
    return { m: parsed[0], is2D: parsed[1] };
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
    constructor(init = undefined) {
      const s = initToState(init);
      matrixState.set(this, s);
    }
    static fromMatrix(other = {}) { const s = fixup(other, false); return make(DOMMatrixReadOnly, s.m, s.is2D); }
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
    translate(tx = 0, ty, tz) { return copy(this).translateSelf(tx, ty, tz); }
    scale(sx = 1, sy, sz, ox, oy, oz) { return copy(this).scaleSelf(sx, sy, sz, ox, oy, oz); }
    scaleNonUniform(sx = 1, sy = 1) { return copy(this).scaleSelf(sx, sy, 1, 0, 0, 0); }
    scale3d(scale = 1, ox, oy, oz) { return copy(this).scale3dSelf(scale, ox, oy, oz); }
    rotate(rx = 0, ry, rz) { return copy(this).rotateSelf(rx, ry, rz); }
    rotateFromVector(x = 0, y) { return copy(this).rotateFromVectorSelf(x, y); }
    rotateAxisAngle(x = 0, y, z, angle) { return copy(this).rotateAxisAngleSelf(x, y, z, angle); }
    skewX(sx = 0) { return copy(this).skewXSelf(sx); }
    skewY(sy = 0) { return copy(this).skewYSelf(sy); }
    multiply(other = {}) { return copy(this).multiplySelf(other); }
    flipX() { const s = st(this); return make(DOMMatrix, multiply(s.m, scaling(-1, 1, 1)), s.is2D); }
    flipY() { const s = st(this); return make(DOMMatrix, multiply(s.m, scaling(1, -1, 1)), s.is2D); }
    inverse() { return copy(this).invertSelf(); }
    transformPoint(point = {}) {
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
    constructor(...args) { super(...args); mutableGeometry.add(this); }
    static fromMatrix(other = {}) { const s = fixup(other, false); return make(DOMMatrix, s.m, s.is2D); }
    static fromFloat32Array(array) { return fromArray(DOMMatrix, array, Float32ArrayC); }
    static fromFloat64Array(array) { return fromArray(DOMMatrix, array, Float64ArrayC); }
    multiplySelf(other = {}) {
      const s = st(this);
      const o = matrixState.has(other) ? matrixState.get(other) : fixup(other, false);
      s.m = multiply(s.m, o.m);
      if (!o.is2D) s.is2D = false;
      return this;
    }
    preMultiplySelf(other = {}) {
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
  if (natives.worker) {
    delete DOMMatrix.prototype.setMatrixValue;
    delete DOMMatrixReadOnly.prototype.toString;
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
  webidl(DOMRectList, 'DOMRectList');
  webidl(DOMRectReadOnly, 'DOMRectReadOnly');
  webidl(DOMRect, 'DOMRect');
  webidl(DOMQuad, 'DOMQuad');
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
  // Copies `src` onto the end of `dst` without spreading it as call
  // arguments: a long path (a chart's thousands of points) exceeds the
  // engine's argument limit.
  function append(dst, src) {
    let n = dst.length;
    for (let i = 0; i < src.length; i++) dst[n++] = src[i];
  }
  function record(self, op, args) {
    const o = ops(self);
    if (!allFinite(args)) return;
    o[o.length] = op;
    append(o, args);
  }
  class Path2D {
    constructor(path) {
      const o = [];
      paths.set(this, o);
      if (path === undefined) return;
      if (paths.has(path)) append(o, paths.get(path));
      else append(o, [12, String(path)]);
    }
    addPath(path, transform) {
      const own = ops(this);
      const other = paths.get(path);
      if (!other) throw new TypeErrorC("Failed to execute 'addPath' on 'Path2D': parameter 1 is not of type 'Path2D'.");
      const m = fixup(transform, true).m;
      const t = [m[0], m[1], m[4], m[5], m[12], m[13]];
      if (!allFinite(t)) return;
      // `other` may be this path: copy it before it grows.
      const copy = other === own ? other.slice() : other;
      own[own.length] = 10;
      append(own, t);
      append(own, copy);
      own[own.length] = 11;
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
  const COLOR_SPACES = ['srgb', 'srgb-linear', 'display-p3', 'display-p3-linear'];
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
    // pyro: only 8-bit RGBA data is made.
    get pixelFormat() { img(this); return 'rgba-unorm8'; }
  }
  webidl(ImageData, 'ImageData');

  // ---- CanvasGradient / CanvasPattern / TextMetrics ----------------------
  // The records canvas.rs reads to paint (`paintRecord`): `version` changes
  // with every change that affects painting.
  const gradients = new WeakMap();
  class CanvasGradient {
    constructor(token, kind, args) {
      if (token !== secret) throw illegal();
      gradients.set(this, { kind, args, stops: [], version: 0 });
    }
    addColorStop(offset, color) {
      const g = gradients.get(this);
      if (!g) throw new TypeErrorC('Illegal invocation');
      if (arguments.length < 2) throw new TypeErrorC(`Failed to execute 'addColorStop' on 'CanvasGradient': 2 arguments required, but only ${arguments.length} present.`);
      const o = finite(offset, "The provided double value");
      if (o < 0 || o > 1) throw domError('IndexSizeError', `The provided value (${o}) is outside the range (0.0, 1.0).`);
      const parsed = parseColor(String(color));
      if (parsed === null) throw domError('SyntaxError', `The value provided ('${color}') could not be parsed as a color.`);
      g.stops.push(o, parsed);
      g.version++;
    }
  }
  webidl(CanvasGradient, 'CanvasGradient');
  // `image` is the native handle of the pixels captured at creation.
  const patterns = new WeakMap();
  class CanvasPattern {
    constructor(token, repetition, image) {
      if (token !== secret) throw illegal();
      patterns.set(this, { repetition, image, transform: [1, 0, 0, 1, 0, 0], version: 0, stops: null });
    }
    setTransform(transform) {
      const p = patterns.get(this);
      if (!p) throw new TypeErrorC('Illegal invocation');
      const m = fixup(transform, true).m;
      p.transform = [m[0], m[1], m[4], m[5], m[12], m[13]];
      p.version++;
    }
  }
  webidl(CanvasPattern, 'CanvasPattern');

  // ---- ImageBitmap / createImageBitmap ---------------------------------
  // `image` is the native handle of the pixels (canvas.rs); `close()` drops
  // it, so the pixels go with the next collection.
  const bitmaps = new WeakMap();
  function bm(self) {
    const b = bitmaps.get(self);
    if (!b) throw new TypeErrorC('Illegal invocation');
    return b;
  }
  class ImageBitmap {
    constructor(token, image, width, height) {
      if (token !== secret) throw illegal();
      bitmaps.set(this, { image, width, height });
    }
    get width() { const b = bm(this); return b.image ? b.width : 0; }
    get height() { const b = bm(this); return b.image ? b.height : 0; }
    close() { bm(this).image = null; }
  }
  webidl(ImageBitmap, 'ImageBitmap');
  function newBitmap(made) { return new ImageBitmap(secret, made[0], made[1], made[2]); }
  const PromiseC = Promise;
  // HTML: createImageBitmap snapshots its source now and resolves (or, for
  // a Blob that does not decode, rejects) in a later task.
  const setTimeoutC = globalThis.setTimeout;
  const queueTask = (f) => {
    try { setTimeoutC(f, 0); } catch (e) { /* the global is closing */ }
  };
  function createImageBitmap(image, sx, sy, sw, sh, options) {
    try {
      if (arguments.length < 1) throw new TypeErrorC(`Failed to execute 'createImageBitmap' on 'Window': 1 argument required, but only 0 present.`);
      let crop = null;
      if (arguments.length >= 5) {
        crop = [+sx, +sy, +sw, +sh];
        if (crop[2] === 0 || crop[3] === 0) throw new RangeErrorC(`The crop rect ${crop[2] === 0 ? 'width' : 'height'} is 0.`);
        if (!isFinite(crop[0]) || !isFinite(crop[1]) || !isFinite(crop[2]) || !isFinite(crop[3])) {
          throw new TypeErrorC(`Failed to execute 'createImageBitmap' on 'Window': The provided crop rect is not finite.`);
        }
      } else if (arguments.length > 2) {
        throw new TypeErrorC(`Failed to execute 'createImageBitmap' on 'Window': Overload resolution failed.`);
      } else {
        options = sx;
      }
      const o = dict(options);
      const rw = o.resizeWidth === undefined ? 0 : +o.resizeWidth >>> 0;
      const rh = o.resizeHeight === undefined ? 0 : +o.resizeHeight >>> 0;
      if ((o.resizeWidth !== undefined && rw === 0) || (o.resizeHeight !== undefined && rh === 0)) {
        throw domError('InvalidStateError', 'The resize width or height is 0.');
      }
      const flip = o.imageOrientation === 'flipY';
      const smooth = o.resizeQuality !== 'pixelated';
      let made;
      try {
        made = natives.makeBitmap(image, crop, rw, rh, flip, smooth);
      } catch (e) {
        if (!natives.isBlob(image)) throw e;
        return new PromiseC((resolve, reject) => queueTask(() => reject(e)));
      }
      return new PromiseC((resolve) => queueTask(() => resolve(newBitmap(made))));
    } catch (e) {
      return PromiseC.reject(e);
    }
  }
  // An image argument that is not an element as canvas.rs takes it, or null:
  // [0, handle] for an ImageBitmap, [1, context or null, width, height] for
  // an OffscreenCanvas, [2, data, width, height, colorSpace] for an ImageData (only
  // where `imageData` allows it), [3, blob] for a Blob.
  function sourceOf(image, imageData) {
    const b = bitmaps.get(image);
    if (b) {
      if (!b.image) throw domError('InvalidStateError', 'The image source is detached.');
      return [0, b.image];
    }
    const o = offscreens.get(image);
    if (o) {
      if (o.detached) throw domError('InvalidStateError', 'The image source is detached.');
      return [1, o.context, o.width, o.height];
    }
    // pyro: a CSSImageValue (Typed OM) draws the image its url() names, once loaded.
    const CSSImageValueC = globalThis.CSSImageValue;
    if (typeof CSSImageValueC === 'function' && image instanceof CSSImageValueC) return [4, String(image)];
    if (imageData) {
      const d = images.get(image);
      if (d) return [2, d.data, d.width, d.height, d.colorSpace];
      if (natives.isBlob(image)) return [3, image];
    }
    return null;
  }

  // ---- OffscreenCanvas -------------------------------------------------
  // Its native 2D context (canvas.rs) draws into the offscreen bitmap.
  // `placeholder` is the native handle of the `<canvas>` it shows on (made
  // by `transferControlToOffscreen`), or null. A transferred OffscreenCanvas
  // is detached: its size reads 0 and using it throws InvalidStateError.
  const offscreens = new WeakMap();
  function oc(self) {
    const o = offscreens.get(self);
    if (!o) throw new TypeErrorC('Illegal invocation');
    return o;
  }
  function live(self) {
    const o = oc(self);
    if (o.detached) throw domError('InvalidStateError', 'OffscreenCanvas object is detached.');
    return o;
  }
  function offscreenSize(value, what) {
    // [EnforceRange] unsigned long long.
    const n = MathC.trunc(+value);
    if (!isFinite(n) || n < 0 || n > NumberC.MAX_SAFE_INTEGER) {
      throw new TypeErrorC(`Value is outside the 'unsigned long long' value range (${what}).`);
    }
    return n;
  }
  class OffscreenCanvas {
    constructor(width, height) {
      if (arguments.length < 2) throw new TypeErrorC(`Failed to construct 'OffscreenCanvas': 2 arguments required, but only ${arguments.length} present.`);
      offscreens.set(this, { width: offscreenSize(width, 'width'), height: offscreenSize(height, 'height'), context: null, detached: false, placeholder: null });
    }
    get width() { return oc(this).width; }
    set width(v) { const o = live(this); o.width = offscreenSize(v, 'width'); if (o.context) natives.resizeContext(o.context, o.width, o.height); }
    get height() { return oc(this).height; }
    set height(v) { const o = live(this); o.height = offscreenSize(v, 'height'); if (o.context) natives.resizeContext(o.context, o.width, o.height); }
    getContext(type, options) {
      const o = oc(this);
      if (arguments.length < 1) throw new TypeErrorC(`Failed to execute 'getContext' on 'OffscreenCanvas': 1 argument required, but only 0 present.`);
      type = String(type);
      if (!['2d', 'webgl', 'webgl2', 'bitmaprenderer', 'webgpu'].includes(type)) {
        throw new TypeErrorC(`Failed to execute 'getContext' on 'OffscreenCanvas': The provided value '${type}' is not a valid enum value of type OffscreenRenderingContextType.`);
      }
      if (o.detached) throw domError('InvalidStateError', 'OffscreenCanvas object is detached.');
      if (type !== '2d') return null;
      if (!o.context) {
        // CanvasRenderingContext2DSettings, converted member by member in
        // dictionary order when the context is made (a cached context
        // ignores its argument).
        const d = dict(options);
        void !!d.alpha;
        const raw = d.colorSpace;
        const space = raw === undefined ? 'srgb' : String(raw);
        if (!COLOR_SPACES.includes(space)) throw new TypeErrorC(`The provided value '${space}' is not a valid enum value of type PredefinedColorSpace.`);
        void !!d.desynchronized;
        void !!d.willReadFrequently;
        o.context = natives.offscreenContext(this, o.width, o.height, o.placeholder, space);
      }
      return o.context;
    }
    transferToImageBitmap() {
      const o = oc(this);
      if (o.detached) throw domError('InvalidStateError', 'Cannot transfer an ImageBitmap from a detached OffscreenCanvas');
      if (!o.context) throw domError('InvalidStateError', 'Cannot transfer an ImageBitmap from an OffscreenCanvas with no context');
      return newBitmap(natives.transferContext(o.context));
    }
    convertToBlob(options) {
      try {
        const o = oc(this);
        if (o.detached) throw domError('InvalidStateError', 'OffscreenCanvas object is detached.');
        if (o.width === 0 || o.height === 0) throw domError('IndexSizeError', 'The canvas has no pixels.');
        const d = dict(options);
        const type = d.type === undefined ? 'image/png' : String(d.type);
        const quality = d.quality === undefined ? undefined : +d.quality;
        return PromiseC.resolve(natives.encodeContext(o.context, o.width, o.height, type, quality));
      } catch (e) {
        return PromiseC.reject(e);
      }
    }
  }
  webidl(OffscreenCanvas, 'OffscreenCanvas');
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
    DOMRectList, DOMRectReadOnly, DOMRect, SVGRect: DOMRect, DOMQuad, DOMPointReadOnly, DOMPoint, DOMMatrixReadOnly, DOMMatrix, WebKitCSSMatrix: DOMMatrix,
    geometryParts(value) {
      const mutable = mutableGeometry.has(value);
      const p = pointState.get(value);
      if (p) return [mutable ? 'DOMPoint' : 'DOMPointReadOnly', [p.x, p.y, p.z, p.w]];
      const rect = rectParts(value);
      if (rect) return [mutable ? 'DOMRect' : 'DOMRectReadOnly', rect];
      const points = quadPoints(value);
      if (points) return ['DOMQuad', [points[0], points[1], points[2], points[3]]];
      const matrix = matrixState.get(value);
      if (!matrix) return undefined;
      const m = matrix.m;
      return [mutable ? 'DOMMatrix' : 'DOMMatrixReadOnly', matrix.is2D ? [m[0], m[1], m[4], m[5], m[12], m[13]] : m.slice()];
    },
    geometryFromParts(name, v) {
      switch (name) {
        case 'DOMPoint': return new DOMPoint(v[0], v[1], v[2], v[3]);
        case 'DOMPointReadOnly': return new DOMPointReadOnly(v[0], v[1], v[2], v[3]);
        case 'DOMRect': return new DOMRect(v[0], v[1], v[2], v[3]);
        case 'DOMRectReadOnly': return new DOMRectReadOnly(v[0], v[1], v[2], v[3]);
        case 'DOMQuad': return quadFromPoints(v);
        case 'DOMMatrix': case 'DOMMatrixReadOnly':
          return make(name === 'DOMMatrix' ? DOMMatrix : DOMMatrixReadOnly,
            v.length === 6 ? from2D(v[0],v[1],v[2],v[3],v[4],v[5]) : v, v.length === 6);
        default: throw domError('DataCloneError', 'Unknown geometry value');
      }
    },
    rectList(rects) { return new DOMRectList(secret, rects); },
    rectValue(x,y,width,height,readonly) { return readonly ? new DOMRectReadOnly(x,y,width,height) : new DOMRect(x,y,width,height); },
    Path2D, ImageData, CanvasGradient, CanvasPattern, TextMetrics,
    ImageBitmap, OffscreenCanvas, createImageBitmap,
    // Helpers for canvas.rs.
    gradient(kind, args) { return new CanvasGradient(secret, kind, args); },
    pattern(repetition, image) { return new CanvasPattern(secret, repetition, image); },
    textMetrics(values) { return new TextMetrics(secret, values); },
    imageData(width, height, settings) { return new ImageData(width, height, settings); },
    // An ImageData over `data` (a Uint8ClampedArray the native side filled).
    imageDataOf(data, width, height, space) {
      const o = Object.create(ImageData.prototype);
      images.set(o, { width, height, data, colorSpace: space });
      return o;
    },
    // Structured serialization and transfer (canvas.rs `canvas_object_of`):
    // [0, handle] for an ImageBitmap, [1, width, height, placeholder] for an
    // OffscreenCanvas that may be transferred (`transfer`), undefined for
    // anything else; DataCloneError for a detached source or an
    // OffscreenCanvas with a context.
    describeTransferable(value, transfer) {
      const b = bitmaps.get(value);
      if (b) {
        if (!b.image) throw domError('DataCloneError', 'An ImageBitmap is detached and could not be cloned.');
        return [0, b.image];
      }
      const o = offscreens.get(value);
      if (!o || !transfer) return undefined;
      if (o.detached) throw domError('DataCloneError', 'An OffscreenCanvas is detached and could not be transferred.');
      if (o.context) throw domError('InvalidStateError', 'An OffscreenCanvas could not be transferred because it had a rendering context.');
      return [1, o.width, o.height, o.placeholder];
    },
    // A transferred ImageBitmap or OffscreenCanvas: detached here.
    detachTransferable(value) {
      const b = bitmaps.get(value);
      if (b) { b.image = null; return true; }
      const o = offscreens.get(value);
      if (!o) return false;
      o.detached = true;
      o.width = 0;
      o.height = 0;
      o.placeholder = null;
      return true;
    },
    // The ImageBitmap of `made` ([handle, width, height]).
    bitmapOf(made) { return newBitmap(made); },
    // An OffscreenCanvas showing on `placeholder` (a native handle or null).
    offscreenOf(width, height, placeholder) {
      const canvas = new OffscreenCanvas(width, height);
      oc(canvas).placeholder = placeholder;
      return canvas;
    },
    // [width, height, data, colorSpace] of an ImageData, or null.
    imageInfo(value) { const s = images.get(value); return s ? [s.width, s.height, s.data, s.colorSpace] : null; },
    // The gradient or pattern record of a fill/stroke style.
    paintRecord(value) { return gradients.get(value) || patterns.get(value) || null; },
    frameCanvasSource(image) { return bitmaps.has(image) || offscreens.has(image); },
    sourceOf,
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
