// The parts of `bun:test` the Playwright runner uses (Bun provides the module at run time).
declare module 'bun:test' {
  type Fn = (...args: any[]) => any;
  type Test = ((name: string, fn: Fn, timeout?: number | object) => void) & {
    only: (name: string, fn: Fn, timeout?: number | object) => void;
    skip: (name: string, fn: Fn, timeout?: number | object) => void;
    concurrent: (name: string, fn: Fn, timeout?: number | object) => void;
  };
  export const test: Test;
  export const describe: ((name: string, fn: () => void) => void) & {
    only: (name: string, fn: () => void) => void;
    skip: (name: string, fn: () => void) => void;
  };
  export const beforeAll: (fn: Fn, timeout?: number) => void;
  export const afterAll: (fn: Fn, timeout?: number) => void;
  export const expect: ((value: unknown) => any) & Record<string, any>;
}
