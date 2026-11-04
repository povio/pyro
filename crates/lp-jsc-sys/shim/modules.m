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

typedef void (*lp_module_fetch_cb)(void *userdata, JSGlobalContextRef ctx, JSStringRef identifier,
                                   JSObjectRef resolve, JSObjectRef reject);

@interface LPModuleLoader : NSObject
@property(nonatomic, strong) JSContext *context;
@property(nonatomic) lp_module_fetch_cb callback;
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
    self.callback(self.userdata, g, ident, (JSObjectRef)resolve.JSValueRef, (JSObjectRef)reject.JSValueRef);
    JSStringRelease(ident);
}
@end

// Capability probe: 1 when this JavaScriptCore has the module SPI the shim uses.
int lp_jsc_modules_available(void) {
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
int lp_jsc_import_maps_available(void) { return 0; }

int lp_jsc_import_map_add(JSGlobalContextRef ctx, const char *src, size_t src_len, const char *base, size_t base_len,
                          JSStringRef *messages) {
    (void)ctx;
    (void)src;
    (void)src_len;
    (void)base;
    (void)base_len;
    *messages = NULL;
    return 0;
}

static NSString *lp_nsstring(const char *utf8, size_t len) {
    return [[NSString alloc] initWithBytes:utf8 length:len encoding:NSUTF8StringEncoding];
}

// Installs the loader. Returns an owned handle (release with lp_jsc_module_loader_release)
// that keeps both the delegate and the ObjC JSContext wrapper alive: the context
// holds its delegate weakly and the wrapper is otherwise only weakly cached.
void *lp_jsc_module_loader_install(JSGlobalContextRef ctx, lp_module_fetch_cb cb, void *userdata) {
    @autoreleasepool {
        LPModuleLoader *loader = [LPModuleLoader new];
        loader.context = [JSContext contextWithJSGlobalContextRef:ctx];
        loader.callback = cb;
        loader.userdata = userdata;
        loader.context.moduleLoaderDelegate = loader;
        return (__bridge_retained void *)loader;
    }
}

void lp_jsc_module_loader_release(void *handle) {
    @autoreleasepool {
        LPModuleLoader *loader = (__bridge_transfer LPModuleLoader *)handle;
        loader.context.moduleLoaderDelegate = nil;
        loader.context = nil;
        (void)loader;
    }
}

// Creates a module record source for `resolve`. Returns NULL (and an owned error
// JSStringRef in *error) when the URL or source cannot be converted.
JSValueRef lp_jsc_module_script_create(JSGlobalContextRef ctx, const char *src, size_t src_len,
                                       const char *url, size_t url_len, JSStringRef *error) {
    @autoreleasepool {
        JSContext *context = [JSContext contextWithJSGlobalContextRef:ctx];
        NSString *source = lp_nsstring(src, src_len);
        NSURL *sourceURL = [NSURL URLWithString:lp_nsstring(url, url_len)];
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
JSValueRef lp_jsc_module_evaluate(JSGlobalContextRef ctx, const char *src, size_t src_len, const char *url,
                                  size_t url_len, JSStringRef *error) {
    @autoreleasepool {
        JSContext *context = [JSContext contextWithJSGlobalContextRef:ctx];
        NSString *source = lp_nsstring(src, src_len);
        NSURL *sourceURL = [NSURL URLWithString:lp_nsstring(url, url_len)];
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
