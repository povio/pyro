// ES module support for the source-built JavaScriptCore (scripts/jsc/build.sh).
//
// Same C interface as shim/modules.m, implemented with the C module-loader SPI
// that webkit/patches/0001-c-api-module-loader.patch adds to JSAPIGlobalObject.
// Declarations are local so this file needs no JavaScriptCore headers.

#include <stdbool.h>
#include <stddef.h>
#include <stdlib.h>

typedef const struct OpaqueJSContext *JSContextRef;
typedef const struct OpaqueJSContextGroup *JSContextGroupRef;
typedef struct OpaqueJSContext *JSGlobalContextRef;
typedef struct OpaqueJSString *JSStringRef;
typedef const struct OpaqueJSValue *JSValueRef;
typedef struct OpaqueJSValue *JSObjectRef;

JSStringRef JSStringCreateWithUTF8CString(const char *string);
JSStringRef JSValueToStringCopy(JSContextRef ctx, JSValueRef value, JSValueRef *exception);
void JSStringRelease(JSStringRef string);

// JSModuleType (patch 0020-module-import-attributes): 0 JavaScript, 1 JSON,
// 2 CSS, 3 WebAssembly, 4 text.
typedef void (*JSModuleLoaderTypedFetchCallback)(JSGlobalContextRef ctx, JSStringRef identifier, int type,
                                                 JSObjectRef resolve, JSObjectRef reject, void *info);
bool JSGlobalContextSetTypedModuleLoader(JSGlobalContextRef ctx, JSModuleLoaderTypedFetchCallback callback, void *info);
JSValueRef JSModuleSourceCreateWithType(JSContextRef ctx, JSStringRef source, JSStringRef url, int type,
                                        JSValueRef *exception);
JSObjectRef JSLoadAndEvaluateModule(JSContextRef ctx, JSStringRef url, JSValueRef *exception);
#ifdef PYRO_JSC_MODULE_PHASES
// Patch 0037-module-graph-phases (build.rs defines this when the archive has it).
JSObjectRef JSLoadModuleGraph(JSContextRef ctx, JSStringRef url, JSValueRef *exception);
JSObjectRef JSLinkAndEvaluateModule(JSContextRef ctx, JSStringRef url, JSValueRef *exception);
#endif
bool JSGlobalContextAddImportMap(JSGlobalContextRef ctx, JSStringRef source, JSStringRef baseURL, JSStringRef *messages);
#ifdef PYRO_JSC_INLINE_MODULE_BASES
// Patch 0038-inline-module-bases (build.rs defines this when the archive has it).
bool JSGlobalContextSetInlineModuleBase(JSGlobalContextRef ctx, JSStringRef identifier, JSStringRef baseURL);
#endif
#ifdef PYRO_JSC_MODULE_FETCH_REFERRER
// Patch 0039-module-fetch-referrer (build.rs defines this when the archive has it).
JSStringRef JSGlobalContextCopyModuleFetchReferrer(JSContextRef ctx);
#endif
typedef JSStringRef (*pyro_module_base_url_cb)(JSGlobalContextRef ctx);
bool JSGlobalContextSetModuleBaseURLCallback(JSGlobalContextRef ctx, pyro_module_base_url_cb callback);

// Patch 0027-precompiled-modules. The struct is JSModulePrecompileStats.
struct pyro_module_precompile_stats {
    size_t modules;
    size_t functions;
    size_t characters;
    size_t record_hits;
    size_t record_misses;
    size_t code_hits;
    size_t function_compiles;
};
bool JSModuleSourcePrecompile(JSContextRef ctx, JSStringRef source, JSStringRef url, bool functions);
void JSContextGroupGetModulePrecompileStats(JSContextGroupRef group, struct pyro_module_precompile_stats *stats);

typedef void (*pyro_module_fetch_cb)(void *userdata, JSGlobalContextRef ctx, JSStringRef identifier, int module_type,
                                   JSObjectRef resolve, JSObjectRef reject);

struct pyro_module_loader {
    JSGlobalContextRef ctx;
    pyro_module_fetch_cb callback;
    void *userdata;
};

int pyro_jsc_modules_available(void) { return 1; }
int pyro_jsc_import_maps_available(void) { return 1; }
// The fetch callback sees each request's `type` import attribute, and JSON and
// CSS module scripts load.
int pyro_jsc_typed_modules_available(void) { return 1; }

static void pyro_fetch(JSGlobalContextRef ctx, JSStringRef identifier, int type, JSObjectRef resolve, JSObjectRef reject,
                     void *info) {
    struct pyro_module_loader *loader = info;
    loader->callback(loader->userdata, ctx, identifier, type, resolve, reject);
}

void *pyro_jsc_module_loader_install(JSGlobalContextRef ctx, pyro_module_fetch_cb cb, pyro_module_base_url_cb base_url,
                                   void *userdata) {
    struct pyro_module_loader *loader = malloc(sizeof *loader);
    loader->ctx = ctx;
    loader->callback = cb;
    loader->userdata = userdata;
    JSGlobalContextSetTypedModuleLoader(ctx, pyro_fetch, loader);
    JSGlobalContextSetModuleBaseURLCallback(ctx, base_url);
    return loader;
}

void pyro_jsc_module_loader_release(void *handle) {
    struct pyro_module_loader *loader = handle;
    JSGlobalContextSetTypedModuleLoader(loader->ctx, NULL, NULL);
    JSGlobalContextSetModuleBaseURLCallback(loader->ctx, NULL);
    free(loader);
}

// JSStringCreateWithUTF8CString needs NUL termination; the Rust callers pass slices.
static JSStringRef pyro_string(const char *utf8, size_t len) {
    char *copy = malloc(len + 1);
    for (size_t i = 0; i < len; i++) copy[i] = utf8[i];
    copy[len] = 0;
    JSStringRef s = JSStringCreateWithUTF8CString(copy);
    free(copy);
    return s;
}

static JSStringRef pyro_error(JSGlobalContextRef ctx, JSValueRef exception, const char *fallback) {
    JSStringRef s = exception ? JSValueToStringCopy(ctx, exception, NULL) : NULL;
    return s ? s : JSStringCreateWithUTF8CString(fallback);
}

// Returns 1 when the map was merged. `*messages` is an owned newline-separated
// list of warnings or the error, or NULL.
int pyro_jsc_import_map_add(JSGlobalContextRef ctx, const char *src, size_t src_len, const char *base, size_t base_len,
                          JSStringRef *messages) {
    JSStringRef source = pyro_string(src, src_len);
    JSStringRef baseURL = pyro_string(base, base_len);
    bool merged = JSGlobalContextAddImportMap(ctx, source, baseURL, messages);
    JSStringRelease(source);
    JSStringRelease(baseURL);
    return merged;
}

// Registers `id` as an inline module script's registry identity resolving
// against `base` (patch 0038). Returns 1 when registered, 0 without the patch.
int pyro_jsc_inline_module_base_set(JSGlobalContextRef ctx, const char *id, size_t id_len, const char *base,
                                    size_t base_len) {
#ifdef PYRO_JSC_INLINE_MODULE_BASES
    JSStringRef identifier = pyro_string(id, id_len);
    JSStringRef baseURL = pyro_string(base, base_len);
    bool set = JSGlobalContextSetInlineModuleBase(ctx, identifier, baseURL);
    JSStringRelease(identifier);
    JSStringRelease(baseURL);
    return set;
#else
    (void)ctx;
    (void)id;
    (void)id_len;
    (void)base;
    (void)base_len;
    return 0;
#endif
}

// The referrer of the module fetch whose callback is running (patch 0039): the
// referring module's key or the import() caller's source URL, as an owned
// string; NULL for a fetch without a referring script, and without the patch.
JSStringRef pyro_jsc_module_fetch_referrer(JSContextRef ctx) {
#ifdef PYRO_JSC_MODULE_FETCH_REFERRER
    return JSGlobalContextCopyModuleFetchReferrer(ctx);
#else
    (void)ctx;
    return NULL;
#endif
}

// 1 when pyro_jsc_module_fetch_referrer reports referrers (patch 0039).
int pyro_jsc_module_fetch_referrer_available(void) {
#ifdef PYRO_JSC_MODULE_FETCH_REFERRER
    return 1;
#else
    return 0;
#endif
}

// `type` is 0 (JavaScript) or 1 (JSON).
JSValueRef pyro_jsc_module_script_create(JSGlobalContextRef ctx, const char *src, size_t src_len, const char *url,
                                       size_t url_len, int type, JSStringRef *error) {
    JSStringRef source = pyro_string(src, src_len);
    JSStringRef sourceURL = pyro_string(url, url_len);
    JSValueRef exception = NULL;
    JSValueRef script = JSModuleSourceCreateWithType(ctx, source, sourceURL, type, &exception);
    JSStringRelease(source);
    JSStringRelease(sourceURL);
    if (!script) *error = pyro_error(ctx, exception, "module creation failed");
    return script;
}

// Module sources can be parsed and compiled ahead of their first load.
int pyro_jsc_module_precompile_available(void) { return 1; }

// Precompiles the JavaScript module source that pyro_jsc_module_script_create
// makes for the same `src` and `url`: the two build their strings alike, and
// a load is served from the precompiled module when its text is the same.
// Returns 1 when the module is kept, 0 for a source that does not parse.
int pyro_jsc_module_precompile(JSGlobalContextRef ctx, const char *src, size_t src_len, const char *url, size_t url_len,
                             int functions) {
    JSStringRef source = pyro_string(src, src_len);
    JSStringRef sourceURL = pyro_string(url, url_len);
    bool kept = JSModuleSourcePrecompile(ctx, source, sourceURL, functions != 0);
    JSStringRelease(source);
    JSStringRelease(sourceURL);
    return kept;
}

void pyro_jsc_module_precompile_stats(JSContextGroupRef group, struct pyro_module_precompile_stats *stats) {
    JSContextGroupGetModulePrecompileStats(group, stats);
}

// Like the ObjC shim, the top-level module is fetched through the loader by URL;
// the caller registers `src` for that URL before calling.
JSValueRef pyro_jsc_module_evaluate(JSGlobalContextRef ctx, const char *src, size_t src_len, const char *url,
                                  size_t url_len, JSStringRef *error) {
    (void)src;
    (void)src_len;
    JSStringRef sourceURL = pyro_string(url, url_len);
    JSValueRef exception = NULL;
    JSObjectRef promise = JSLoadAndEvaluateModule(ctx, sourceURL, &exception);
    JSStringRelease(sourceURL);
    if (!promise) *error = pyro_error(ctx, exception, "module evaluation failed");
    return promise;
}

// A module script's two steps apart (patch 0037): 1 when this JavaScriptCore has them.
int pyro_jsc_module_phases_available(void) {
#ifdef PYRO_JSC_MODULE_PHASES
    return 1;
#else
    return 0;
#endif
}

// Fetches the graph of the top-level module `url` (registered like
// pyro_jsc_module_evaluate's) and returns a promise that fulfills once it is
// fetched and parsed, or rejects with the failure. NULL (and an owned error
// string) without patch 0037 or when the context cannot load modules.
JSValueRef pyro_jsc_module_load(JSGlobalContextRef ctx, const char *url, size_t url_len, JSStringRef *error) {
#ifdef PYRO_JSC_MODULE_PHASES
    JSStringRef sourceURL = pyro_string(url, url_len);
    JSValueRef exception = NULL;
    JSObjectRef promise = JSLoadModuleGraph(ctx, sourceURL, &exception);
    JSStringRelease(sourceURL);
    if (!promise) *error = pyro_error(ctx, exception, "module loading failed");
    return promise;
#else
    (void)ctx;
    (void)url;
    (void)url_len;
    *error = JSStringCreateWithUTF8CString("module graph phases need patch 0037");
    return NULL;
#endif
}

// Links and evaluates the graph pyro_jsc_module_load fetched for `url`: the
// evaluation promise, or NULL with *exception set to the link error (or
// another failure).
JSValueRef pyro_jsc_module_link_evaluate(JSGlobalContextRef ctx, const char *url, size_t url_len, JSValueRef *exception) {
#ifdef PYRO_JSC_MODULE_PHASES
    JSStringRef sourceURL = pyro_string(url, url_len);
    JSObjectRef promise = JSLinkAndEvaluateModule(ctx, sourceURL, exception);
    JSStringRelease(sourceURL);
    return promise;
#else
    (void)ctx;
    (void)url;
    (void)url_len;
    *exception = NULL;
    return NULL;
#endif
}
