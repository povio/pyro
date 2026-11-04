// ES module support for the source-built JavaScriptCore (scripts/jsc/build.sh).
//
// Same C interface as shim/modules.m, implemented with the C module-loader SPI
// that webkit/patches/0001-c-api-module-loader.patch adds to JSAPIGlobalObject.
// Declarations are local so this file needs no JavaScriptCore headers.

#include <stdbool.h>
#include <stddef.h>
#include <stdlib.h>

typedef const struct OpaqueJSContext *JSContextRef;
typedef struct OpaqueJSContext *JSGlobalContextRef;
typedef struct OpaqueJSString *JSStringRef;
typedef const struct OpaqueJSValue *JSValueRef;
typedef struct OpaqueJSValue *JSObjectRef;

JSStringRef JSStringCreateWithUTF8CString(const char *string);
JSStringRef JSValueToStringCopy(JSContextRef ctx, JSValueRef value, JSValueRef *exception);
void JSStringRelease(JSStringRef string);

typedef void (*JSModuleLoaderFetchCallback)(JSGlobalContextRef ctx, JSStringRef identifier, JSObjectRef resolve,
                                            JSObjectRef reject, void *info);
bool JSGlobalContextSetModuleLoader(JSGlobalContextRef ctx, JSModuleLoaderFetchCallback callback, void *info);
JSValueRef JSModuleSourceCreate(JSContextRef ctx, JSStringRef source, JSStringRef url, JSValueRef *exception);
JSObjectRef JSLoadAndEvaluateModule(JSContextRef ctx, JSStringRef url, JSValueRef *exception);
bool JSGlobalContextAddImportMap(JSGlobalContextRef ctx, JSStringRef source, JSStringRef baseURL, JSStringRef *messages);

typedef void (*lp_module_fetch_cb)(void *userdata, JSGlobalContextRef ctx, JSStringRef identifier, JSObjectRef resolve,
                                   JSObjectRef reject);

struct lp_module_loader {
    JSGlobalContextRef ctx;
    lp_module_fetch_cb callback;
    void *userdata;
};

int lp_jsc_modules_available(void) { return 1; }
int lp_jsc_import_maps_available(void) { return 1; }

static void lp_fetch(JSGlobalContextRef ctx, JSStringRef identifier, JSObjectRef resolve, JSObjectRef reject,
                     void *info) {
    struct lp_module_loader *loader = info;
    loader->callback(loader->userdata, ctx, identifier, resolve, reject);
}

void *lp_jsc_module_loader_install(JSGlobalContextRef ctx, lp_module_fetch_cb cb, void *userdata) {
    struct lp_module_loader *loader = malloc(sizeof *loader);
    loader->ctx = ctx;
    loader->callback = cb;
    loader->userdata = userdata;
    JSGlobalContextSetModuleLoader(ctx, lp_fetch, loader);
    return loader;
}

void lp_jsc_module_loader_release(void *handle) {
    struct lp_module_loader *loader = handle;
    JSGlobalContextSetModuleLoader(loader->ctx, NULL, NULL);
    free(loader);
}

// JSStringCreateWithUTF8CString needs NUL termination; the Rust callers pass slices.
static JSStringRef lp_string(const char *utf8, size_t len) {
    char *copy = malloc(len + 1);
    for (size_t i = 0; i < len; i++) copy[i] = utf8[i];
    copy[len] = 0;
    JSStringRef s = JSStringCreateWithUTF8CString(copy);
    free(copy);
    return s;
}

static JSStringRef lp_error(JSGlobalContextRef ctx, JSValueRef exception, const char *fallback) {
    JSStringRef s = exception ? JSValueToStringCopy(ctx, exception, NULL) : NULL;
    return s ? s : JSStringCreateWithUTF8CString(fallback);
}

// Returns 1 when the map was merged. `*messages` is an owned newline-separated
// list of warnings or the error, or NULL.
int lp_jsc_import_map_add(JSGlobalContextRef ctx, const char *src, size_t src_len, const char *base, size_t base_len,
                          JSStringRef *messages) {
    JSStringRef source = lp_string(src, src_len);
    JSStringRef baseURL = lp_string(base, base_len);
    bool merged = JSGlobalContextAddImportMap(ctx, source, baseURL, messages);
    JSStringRelease(source);
    JSStringRelease(baseURL);
    return merged;
}

JSValueRef lp_jsc_module_script_create(JSGlobalContextRef ctx, const char *src, size_t src_len, const char *url,
                                       size_t url_len, JSStringRef *error) {
    JSStringRef source = lp_string(src, src_len);
    JSStringRef sourceURL = lp_string(url, url_len);
    JSValueRef exception = NULL;
    JSValueRef script = JSModuleSourceCreate(ctx, source, sourceURL, &exception);
    JSStringRelease(source);
    JSStringRelease(sourceURL);
    if (!script) *error = lp_error(ctx, exception, "module creation failed");
    return script;
}

// Like the ObjC shim, the top-level module is fetched through the loader by URL;
// the caller registers `src` for that URL before calling.
JSValueRef lp_jsc_module_evaluate(JSGlobalContextRef ctx, const char *src, size_t src_len, const char *url,
                                  size_t url_len, JSStringRef *error) {
    (void)src;
    (void)src_len;
    JSStringRef sourceURL = lp_string(url, url_len);
    JSValueRef exception = NULL;
    JSObjectRef promise = JSLoadAndEvaluateModule(ctx, sourceURL, &exception);
    JSStringRelease(sourceURL);
    if (!promise) *error = lp_error(ctx, exception, "module evaluation failed");
    return promise;
}
