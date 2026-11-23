// ES module support for the C API via JavaScriptCore's Objective-C SPI.
//
// The public JavaScriptCore C API has no module loader. The system framework
// does ship (unexported-in-headers but present and stable since macOS 10.15)
// `JSScript` with type module and `-[JSContext setModuleLoaderDelegate:]`,
// which hook into JSAPIGlobalObject's loader. This shim exposes exactly that to
// Rust as plain C: fetching is delegated back to Rust, which resolves or rejects
// asynchronously (after network I/O) using ordinary C API calls.
//
// Only contexts created with JSGlobalContextCreateInGroup(group, NULL) use
// JSAPIGlobalObject and therefore support this loader.

#import <Foundation/Foundation.h>
#import <JavaScriptCore/JavaScriptCore.h>

@interface LPJSScript : NSObject
+ (instancetype)scriptOfType:(int)type
                  withSource:(NSString *)source
                andSourceURL:(NSURL *)sourceURL
            andBytecodeCache:(NSURL *)cachePath
            inVirtualMachine:(JSVirtualMachine *)vm
                       error:(out NSError **)error;
@end

@interface JSContext (LPModuleSPI)
@property(nonatomic, weak) id moduleLoaderDelegate;
- (JSValue *)evaluateJSScript:(LPJSScript *)script;
@end

enum { kLPScriptTypeProgram = 0, kLPScriptTypeModule = 1 };

// `module_type` is always 0 (JavaScript): the system framework's delegate does
// not see import attributes (pyro_jsc_typed_modules_available).
typedef void (*pyro_module_fetch_cb)(void *userdata, JSGlobalContextRef ctx, JSStringRef identifier, int module_type,
                                   JSObjectRef resolve, JSObjectRef reject);

@interface LPModuleLoader : NSObject
@property(nonatomic, strong) JSContext *context;
@property(nonatomic) pyro_module_fetch_cb callback;
@property(nonatomic) void *userdata;
@end

@implementation LPModuleLoader
- (void)context:(JSContext *)context
    fetchModuleForIdentifier:(JSValue *)identifier
         withResolveHandler:(JSValue *)resolve
           andRejectHandler:(JSValue *)reject {
    JSGlobalContextRef g = context.JSGlobalContextRef;
    JSStringRef ident = JSValueToStringCopy(g, identifier.JSValueRef, NULL);
    if (!ident) return;
    self.callback(self.userdata, g, ident, 0, (JSObjectRef)resolve.JSValueRef, (JSObjectRef)reject.JSValueRef);
    JSStringRelease(ident);
}
@end

// Capability probe: 1 when this JavaScriptCore has the module SPI the shim uses.
int pyro_jsc_modules_available(void) {
    @autoreleasepool {
        Class script = NSClassFromString(@"JSScript");
        if (!script) return 0;
        if (![script respondsToSelector:@selector(scriptOfType:withSource:andSourceURL:andBytecodeCache:inVirtualMachine:error:)])
            return 0;
        if (![JSContext instancesRespondToSelector:@selector(setModuleLoaderDelegate:)]) return 0;
        if (![JSContext instancesRespondToSelector:@selector(evaluateJSScript:)]) return 0;
        return 1;
    }
}

// Import maps need the resolve hook of the source-built JavaScriptCore; the system
// framework's loader rejects bare specifiers before asking the delegate.
int pyro_jsc_import_maps_available(void) { return 0; }

// The system framework passes no import attributes to the delegate, so JSON
// and CSS module scripts cannot load: every request looks like a JavaScript
// one, and the embedder rejects JSON and CSS responses to those.
int pyro_jsc_typed_modules_available(void) { return 0; }

// Precompiled modules are the source-built JavaScriptCore's (patch 0027).
struct pyro_module_precompile_stats {
    size_t modules;
    size_t functions;
    size_t characters;
    size_t record_hits;
    size_t record_misses;
    size_t code_hits;
    size_t function_compiles;
};

int pyro_jsc_module_precompile_available(void) { return 0; }

int pyro_jsc_module_precompile(JSGlobalContextRef ctx, const char *src, size_t src_len, const char *url, size_t url_len,
                             int functions) {
    (void)ctx;
    (void)src;
    (void)src_len;
    (void)url;
    (void)url_len;
    (void)functions;
    return 0;
}

void pyro_jsc_module_precompile_stats(JSContextGroupRef group, struct pyro_module_precompile_stats *stats) {
    (void)group;
    *stats = (struct pyro_module_precompile_stats){0};
}

int pyro_jsc_import_map_add(JSGlobalContextRef ctx, const char *src, size_t src_len, const char *base, size_t base_len,
                          JSStringRef *messages) {
    (void)ctx;
    (void)src;
    (void)src_len;
    (void)base;
    (void)base_len;
    *messages = NULL;
    return 0;
}

static NSString *pyro_nsstring(const char *utf8, size_t len) {
    return [[NSString alloc] initWithBytes:utf8 length:len encoding:NSUTF8StringEncoding];
}

// Installs the loader. Returns an owned handle (release with pyro_jsc_module_loader_release)
// that keeps both the delegate and the ObjC JSContext wrapper alive: the context
// holds its delegate weakly and the wrapper is otherwise only weakly cached.
// `base_url` is unused: the system framework has no resolve hook (see
// pyro_jsc_import_maps_available).
void *pyro_jsc_module_loader_install(JSGlobalContextRef ctx, pyro_module_fetch_cb cb, JSStringRef (*base_url)(JSGlobalContextRef),
                                   void *userdata) {
    (void)base_url;
    @autoreleasepool {
        LPModuleLoader *loader = [LPModuleLoader new];
        loader.context = [JSContext contextWithJSGlobalContextRef:ctx];
        loader.callback = cb;
        loader.userdata = userdata;
        loader.context.moduleLoaderDelegate = loader;
        return (__bridge_retained void *)loader;
    }
}

void pyro_jsc_module_loader_release(void *handle) {
    @autoreleasepool {
        LPModuleLoader *loader = (__bridge_transfer LPModuleLoader *)handle;
        loader.context.moduleLoaderDelegate = nil;
        loader.context = nil;
        (void)loader;
    }
}

// Creates a module record source for `resolve`. Returns NULL (and an owned error
// JSStringRef in *error) when the URL or source cannot be converted.
JSValueRef pyro_jsc_module_script_create(JSGlobalContextRef ctx, const char *src, size_t src_len,
                                       const char *url, size_t url_len, int type, JSStringRef *error) {
    if (type != 0) {
        *error = JSStringCreateWithUTF8CString("JSON module scripts need the source-built JavaScriptCore");
        return NULL;
    }
    @autoreleasepool {
        JSContext *context = [JSContext contextWithJSGlobalContextRef:ctx];
        NSString *source = pyro_nsstring(src, src_len);
        NSURL *sourceURL = [NSURL URLWithString:pyro_nsstring(url, url_len)];
        if (!source || !sourceURL) {
            *error = JSStringCreateWithUTF8CString(source ? "invalid module URL" : "module source is not UTF-8");
            return NULL;
        }
        NSError *err = nil;
        LPJSScript *script = [(Class)NSClassFromString(@"JSScript") scriptOfType:kLPScriptTypeModule
                                       withSource:source
                                     andSourceURL:sourceURL
                                 andBytecodeCache:nil
                                 inVirtualMachine:context.virtualMachine
                                            error:&err];
        if (!script) {
            *error = JSStringCreateWithUTF8CString(err ? err.localizedDescription.UTF8String : "module creation failed");
            return NULL;
        }
        return [JSValue valueWithObject:script inContext:context].JSValueRef;
    }
}

// Evaluates a top-level module script. Returns the evaluation promise.
JSValueRef pyro_jsc_module_evaluate(JSGlobalContextRef ctx, const char *src, size_t src_len, const char *url,
                                  size_t url_len, JSStringRef *error) {
    @autoreleasepool {
        JSContext *context = [JSContext contextWithJSGlobalContextRef:ctx];
        NSString *source = pyro_nsstring(src, src_len);
        NSURL *sourceURL = [NSURL URLWithString:pyro_nsstring(url, url_len)];
        if (!source || !sourceURL) {
            *error = JSStringCreateWithUTF8CString(source ? "invalid module URL" : "module source is not UTF-8");
            return NULL;
        }
        NSError *err = nil;
        LPJSScript *script = [(Class)NSClassFromString(@"JSScript") scriptOfType:kLPScriptTypeModule
                                       withSource:source
                                     andSourceURL:sourceURL
                                 andBytecodeCache:nil
                                 inVirtualMachine:context.virtualMachine
                                            error:&err];
        if (!script) {
            *error = JSStringCreateWithUTF8CString(err ? err.localizedDescription.UTF8String : "module creation failed");
            return NULL;
        }
        JSValue *result = [context evaluateJSScript:script];
        return result.JSValueRef;
    }
}

// Inline module base registration is the source-built JavaScriptCore's (patch 0038).
int pyro_jsc_inline_module_base_set(JSGlobalContextRef ctx, const char *id, size_t id_len, const char *base,
                                    size_t base_len) {
    (void)ctx;
    (void)id;
    (void)id_len;
    (void)base;
    (void)base_len;
    return 0;
}

// A module fetch's referrer is the source-built JavaScriptCore's (patch 0039).
JSStringRef pyro_jsc_module_fetch_referrer(JSContextRef ctx) {
    (void)ctx;
    return NULL;
}
int pyro_jsc_module_fetch_referrer_available(void) { return 0; }

// The module script phases are the source-built JavaScriptCore's (patch 0037).
int pyro_jsc_module_phases_available(void) { return 0; }

JSValueRef pyro_jsc_module_load(JSGlobalContextRef ctx, const char *url, size_t url_len, JSStringRef *error) {
    (void)ctx;
    (void)url;
    (void)url_len;
    *error = JSStringCreateWithUTF8CString("module graph phases need the source-built JavaScriptCore");
    return NULL;
}

JSValueRef pyro_jsc_module_link_evaluate(JSGlobalContextRef ctx, const char *url, size_t url_len, JSValueRef *exception) {
    (void)ctx;
    (void)url;
    (void)url_len;
    *exception = NULL;
    return NULL;
}
