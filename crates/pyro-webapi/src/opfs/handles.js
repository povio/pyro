// Origin-private file system interfaces. Each entry is a native host object:
// its locator cannot be manufactured by changing a JavaScript prototype and
// can be used as an argument from another realm in the same context.
(function (native, streams, dedicated) {
  'use strict';
  const {defineProperty, getOwnPropertyDescriptor, getOwnPropertyNames, getPrototypeOf, setPrototypeOf, create} = Object;
  const TypeErrorC = TypeError;
  const toWellFormed = Function.call.bind(String.prototype.toWellFormed);
  const string = value => toWellFormed(`${value}`);
  const entryOf = value => (value !== null && (typeof value === 'object' || typeof value === 'function'))
    ? native.entry(value) : null;

  function handle(value, kind) {
    const entry = entryOf(value);
    if (!entry || (kind !== undefined && native.kind(entry) !== kind)) {
      throw new TypeErrorC('Expected a file system handle of the requested kind.');
    }
    return entry;
  }
  function required(length) {
    if (length === 0) throw new TypeErrorC('A required argument is missing.');
  }
  function dictionary(value) {
    if (value != null && typeof value !== 'object' && typeof value !== 'function') {
      throw new TypeErrorC('Expected an options dictionary.');
    }
    return value;
  }
  function permission(options) {
    const mode = dictionary(options)?.mode;
    if (mode !== undefined) {
      const modeString = `${mode}`;
      if (modeString !== 'read' && modeString !== 'readwrite') {
        throw new TypeErrorC('Invalid file system permission mode.');
      }
    }
    // Access to an entry in the origin-private file system is always granted.
    return 'granted';
  }
  function wrap(entry) {
    const prototype = native.kind(entry) === 'file'
      ? FileSystemFileHandle.prototype : FileSystemDirectoryHandle.prototype;
    return setPrototypeOf(entry, prototype);
  }

  class FileSystemHandle {
    constructor() {
      throw new TypeErrorC('Illegal constructor');
    }
    get kind() { return native.kind(handle(this)); }
    get name() { return native.name(handle(this)); }
    async isSameEntry(other) {
      const self = handle(this);
      required(arguments.length);
      const target = handle(other);
      await undefined;
      return native.same(self, target);
    }
    async queryPermission(options = undefined) {
      handle(this);
      return permission(options);
    }
    async requestPermission(options = undefined) {
      handle(this);
      return permission(options);
    }
    async remove(options = undefined) {
      const self = handle(this);
      const recursive = !!dictionary(options)?.recursive;
      await undefined;
      return native.remove(self, recursive);
    }
    async move(destination, ...rest) {
      const self = handle(this);
      required(arguments.length);
      const candidate = entryOf(destination);
      let dir = null;
      let name = null;
      if (rest.length > 0) {
        dir = handle(destination, 'directory');
        name = string(rest[0]);
      } else if (candidate && native.kind(candidate) === 'directory') {
        dir = candidate;
      } else {
        name = string(destination);
      }
      await undefined;
      return native.move(self, dir, name);
    }
    async getUniqueId() {
      const self = handle(this);
      await undefined;
      return native.id(self);
    }
  }

  class FileSystemFileHandle extends FileSystemHandle {
    async getFile() {
      const self = handle(this, 'file');
      await undefined;
      return native.snapshot(self);
    }
    async createWritable(options = undefined) {
      return streams.createWritable(handle(this, 'file'), options);
    }
    async createSyncAccessHandle(options = undefined) {
      return streams.createSync(handle(this, 'file'), options);
    }
  }
  if (!dedicated) delete FileSystemFileHandle.prototype.createSyncAccessHandle;

  // The native directory cursor seeks the next live entry by name. It neither
  // repeats names nor retains snapshots of all file handles in a directory.
  const iterators = new WeakMap();
  const getIterator = iterators.get.bind(iterators);
  const setIterator = iterators.set.bind(iterators);
  const asyncIteratorPrototype = getPrototypeOf(getPrototypeOf((async function* () {}).prototype));
  const iteratorPrototype = create(asyncIteratorPrototype);
  const next = {
    async next() {
      const cursor = getIterator(this);
      if (!cursor) throw new TypeErrorC('Illegal invocation');
      await undefined;
      if (cursor.done) return {value: undefined, done: true};
      try {
        const pair = native.next(cursor.dir, cursor.after);
        if (pair !== null) {
          const name = pair[0];
          cursor.after = name;
          const value = cursor.kind === 'keys' ? name
            : cursor.kind === 'values' ? wrap(pair[1]) : [name, wrap(pair[1])];
          return {value, done: false};
        }
      } catch (e) {
        cursor.done = true;
        throw e;
      }
      cursor.done = true;
      return {value: undefined, done: true};
    }
  }.next;
  defineProperty(iteratorPrototype, 'next', {
    value: next, enumerable: true, writable: true, configurable: true,
  });
  defineProperty(iteratorPrototype, Symbol.toStringTag, {
    value: 'FileSystemDirectoryHandleAsyncIterator', configurable: true,
  });
  function iterate(dir, kind) {
    const result = create(iteratorPrototype);
    setIterator(result, {dir, kind, after: null, done: false});
    return result;
  }

  class FileSystemDirectoryHandle extends FileSystemHandle {
    async getDirectoryHandle(name, options = undefined) {
      const dir = handle(this, 'directory');
      required(arguments.length);
      const key = string(name);
      const createEntry = !!dictionary(options)?.create;
      await undefined;
      return wrap(native.find(dir, key, 'directory', createEntry));
    }
    async getFileHandle(name, options = undefined) {
      const dir = handle(this, 'directory');
      required(arguments.length);
      const key = string(name);
      const createEntry = !!dictionary(options)?.create;
      await undefined;
      return wrap(native.find(dir, key, 'file', createEntry));
    }
    async resolve(possibleDescendant) {
      const dir = handle(this, 'directory');
      required(arguments.length);
      const child = handle(possibleDescendant);
      await undefined;
      return native.resolve(dir, child);
    }
    async removeEntry(name, options = undefined) {
      const dir = handle(this, 'directory');
      required(arguments.length);
      const key = string(name);
      const recursive = !!dictionary(options)?.recursive;
      await undefined;
      return native.removeEntry(dir, key, recursive);
    }
    entries() { return iterate(handle(this, 'directory'), 'entries'); }
    keys() { return iterate(handle(this, 'directory'), 'keys'); }
    values() { return iterate(handle(this, 'directory'), 'values'); }
  }
  defineProperty(FileSystemDirectoryHandle.prototype, Symbol.asyncIterator, {
    value: FileSystemDirectoryHandle.prototype.entries, configurable: true, writable: true,
  });

  for (const C of [FileSystemHandle, FileSystemFileHandle, FileSystemDirectoryHandle]) {
    for (const name of getOwnPropertyNames(C.prototype)) {
      if (name === 'constructor') continue;
      const desc = getOwnPropertyDescriptor(C.prototype, name);
      desc.enumerable = true;
      defineProperty(C.prototype, name, desc);
    }
    defineProperty(C.prototype, Symbol.toStringTag, {value: C.name, configurable: true});
  }

  async function getDirectory() {
    await undefined;
    return wrap(native.root());
  }
  return {
    getDirectory, FileSystemHandle, FileSystemDirectoryHandle, FileSystemFileHandle,
    FileSystemWritableFileStream: streams.FileSystemWritableFileStream,
    FileSystemSyncAccessHandle: streams.FileSystemSyncAccessHandle,
  };
})
