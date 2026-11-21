#include <openssl/core_names.h>
#include <openssl/crypto.h>
#include <openssl/evp.h>
#include <openssl/provider.h>
#include <openssl/x509.h>
#include <stdio.h>
#include <stdlib.h>
int pyro_mlkem_convert(int,int,int,const unsigned char*,size_t,unsigned char*,size_t,size_t*);
static void exercise(const char *name, int level, char *format) {
    OSSL_LIB_CTX *lib=OSSL_LIB_CTX_new();
    OSSL_PARAM params[]={OSSL_PARAM_construct_utf8_string(OSSL_PKEY_PARAM_ML_KEM_OUTPUT_FORMATS,format,0), OSSL_PARAM_construct_end()};
    OSSL_PROVIDER *p=OSSL_PROVIDER_load_ex(lib,"default",params);
    EVP_PKEY *key=p?EVP_PKEY_Q_keygen(lib,NULL,name):NULL;
    PKCS8_PRIV_KEY_INFO *info=key?EVP_PKEY2PKCS8(key):NULL;
    unsigned char *der=NULL, seed[64];
    const int len=info?i2d_PKCS8_PRIV_KEY_INFO(info,&der):0;
    size_t n=0;
    const int result=len>0?pyro_mlkem_convert(level,4,2,der,(size_t)len,seed,sizeof seed,&n):0;
    if(result != -1) {fprintf(stderr,"unexpected provider result %d for %s %s\n",result,name,format);exit(1);}
    OPENSSL_cleanse(seed,sizeof seed);OPENSSL_clear_free(der,len);
    PKCS8_PRIV_KEY_INFO_free(info);EVP_PKEY_free(key);OSSL_PROVIDER_unload(p);OSSL_LIB_CTX_free(lib);
    printf("{\"algorithm\":\"%s\",\"pkcs8Format\":\"%s\",\"bytes\":%d,\"adapter\":\"unsupported\"}\n",name,format,len);
}
int main(void) {
    for(int i=0;i<2;i++){
        char *fmt=i?"priv-only":"seed-priv";
        exercise("ML-KEM-512",512,fmt);exercise("ML-KEM-768",768,fmt);exercise("ML-KEM-1024",1024,fmt);
    }
}
