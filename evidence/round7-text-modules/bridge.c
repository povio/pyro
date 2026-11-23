#include <JavaScriptCore/JavaScript.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef void (*TypedFetch)(JSGlobalContextRef, JSStringRef, int, JSObjectRef, JSObjectRef, void *);
extern bool JSGlobalContextSetTypedModuleLoader(JSGlobalContextRef, TypedFetch, void *);
extern JSObjectRef JSLoadAndEvaluateModule(JSContextRef, JSStringRef, JSValueRef *);
extern JSValueRef JSModuleSourceCreateWithType(JSContextRef, JSStringRef, JSStringRef, int, JSValueRef *);

static int text_count, js_count;
static const char *entry =
    "import value from 'https://app.test/same' with {type:'text'};"
    "globalThis.staticText = value;"
    "import('https://app.test/same',{with:{type:'text'}}).then(ns => {"
    "globalThis.dynamicText=ns.default; globalThis.keys=Object.keys(ns);"
    "return import('https://app.test/same'); }).then(ns=>globalThis.jsValue=ns.default)"
    ".catch(e=>globalThis.failure=String(e));";
#include "literal.h"

static void fetchModule(JSGlobalContextRef ctx, JSStringRef key, int type, JSObjectRef resolve, JSObjectRef reject, void *unused) {
    (void)unused;
    char url[512]; JSStringGetUTF8CString(key,url,sizeof(url));
    const char *code;
    if (strcmp(url, "https://app.test/entry")==0 && type==0) code=entry;
    else if (strcmp(url,"https://app.test/same")==0 && type==4) { text_count++; code=text_module; }
    else if (strcmp(url,"https://app.test/same")==0 && type==0) { js_count++; code="globalThis.executions++;export default 7;"; }
    else { JSValueRef err=JSValueMakeNumber(ctx,type); JSObjectCallAsFunction(ctx,reject,NULL,1,&err,NULL);return; }
    JSStringRef src=JSStringCreateWithUTF8CString(code);
    JSValueRef exception=NULL, arg=JSModuleSourceCreateWithType(ctx,src,key,0,&exception);
    JSStringRelease(src);
    if (exception) JSObjectCallAsFunction(ctx,reject,NULL,1,&exception,NULL);
    else JSObjectCallAsFunction(ctx,resolve,NULL,1,&arg,NULL);
}
static JSValueRef eval(JSGlobalContextRef ctx,const char *text) {
    JSStringRef src=JSStringCreateWithUTF8CString(text); JSValueRef exc=NULL;
    JSValueRef result=JSEvaluateScript(ctx,src,NULL,NULL,1,&exc);JSStringRelease(src);
    return exc?exc:result;
}
int main(void) {
    JSGlobalContextRef ctx=JSGlobalContextCreate(NULL);
    if (!JSGlobalContextSetTypedModuleLoader(ctx,fetchModule,NULL)) return 2;
    eval(ctx,"globalThis.executions=0;");
    JSStringRef url=JSStringCreateWithUTF8CString("https://app.test/entry");JSValueRef exc=NULL;
    JSObjectRef p=JSLoadAndEvaluateModule(ctx,url,&exc);JSStringRelease(url);
    if (!p || exc) return 3;
    JSStringRef k=JSStringCreateWithUTF8CString("resultPromise");JSObjectSetProperty(ctx,JSContextGetGlobalObject(ctx),k,p,0,NULL);JSStringRelease(k);
    eval(ctx,"resultPromise.catch(e=>globalThis.failure=String(e));");
    eval(ctx,"void 0");JSValueRef result=eval(ctx,"JSON.stringify({staticText:globalThis.staticText,dynamicText:globalThis.dynamicText,keys:globalThis.keys,jsValue:globalThis.jsValue,executions:globalThis.executions,failure:globalThis.failure})");
    JSStringRef s=JSValueToStringCopy(ctx,result,NULL);size_t max=JSStringGetMaximumUTF8CStringSize(s);char *out=malloc(max);JSStringGetUTF8CString(s,out,max);printf("%s\ntext_fetches=%d js_fetches=%d\n",out,text_count,js_count);
    free(out);JSStringRelease(s);JSGlobalContextRelease(ctx);return 0;
}
