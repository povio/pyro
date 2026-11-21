#include <openssl/core_names.h>
#include <openssl/crypto.h>
#include <openssl/evp.h>
#include <openssl/x509.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static void require(int ok, const char *what) {
    if (!ok) { fprintf(stderr, "Provider operation failed: %s\n", what); exit(1); }
}
static uint64_t nanos(void) {
    struct timespec t;
    require(clock_gettime(CLOCK_MONOTONIC, &t) == 0, "monotonic clock");
    return (uint64_t)t.tv_sec * 1000000000 + t.tv_nsec;
}
static int compare(const void *l, const void *r) {
    uint64_t a = *(const uint64_t *)l, b = *(const uint64_t *)r;
    return (a > b) - (a < b);
}
static void measure(const char *name, size_t pub_expected, size_t ct_expected) {
    uint64_t gen[31], enc[31], dec[31];
    int spki_len = 0, pkcs8_len = 0;
    for (int i = -1; i < 31; i++) {
        uint8_t public_bytes[1568], ciphertext[1568], shared[32], recovered[32];
        size_t pub_len = sizeof public_bytes, ct_len = sizeof ciphertext, ss_len = sizeof shared, recovered_len = sizeof recovered, seed_len = 0;
        uint64_t start = nanos();
        EVP_PKEY *private = EVP_PKEY_Q_keygen(NULL, NULL, name);
        require(private != NULL, "generate");
        uint64_t generated = nanos();
        require(EVP_PKEY_get_raw_public_key(private, public_bytes, &pub_len) == 1 && pub_len == pub_expected, "public key export");
        require(EVP_PKEY_get_octet_string_param(private, OSSL_PKEY_PARAM_ML_KEM_SEED, NULL, 0, &seed_len) == 1 && seed_len == 64, "seed availability");
        spki_len = i2d_PUBKEY(private, NULL);
        PKCS8_PRIV_KEY_INFO *info = EVP_PKEY2PKCS8(private);
        require(info != NULL, "PKCS8 encoding");
        pkcs8_len = i2d_PKCS8_PRIV_KEY_INFO(info,NULL);
        PKCS8_PRIV_KEY_INFO_free(info);
        require(spki_len > 0 && pkcs8_len > 0, "encoded sizes");
        EVP_PKEY *public = EVP_PKEY_new_raw_public_key_ex(NULL,name,NULL,public_bytes,pub_len);
        require(public != NULL,"separate public key import");
        uint64_t before_enc = nanos();
        EVP_PKEY_CTX *e = EVP_PKEY_CTX_new_from_pkey(NULL,public,NULL);
        require(e && EVP_PKEY_encapsulate_init(e,NULL) == 1 && EVP_PKEY_encapsulate(e,ciphertext,&ct_len,shared,&ss_len) == 1,"encapsulate");
        EVP_PKEY_CTX_free(e);
        uint64_t before_dec = nanos();
        EVP_PKEY_CTX *d = EVP_PKEY_CTX_new_from_pkey(NULL,private,NULL);
        require(d && EVP_PKEY_decapsulate_init(d,NULL) == 1 && EVP_PKEY_decapsulate(d,recovered,&recovered_len,ciphertext,ct_len) == 1,"decapsulate");
        EVP_PKEY_CTX_free(d);
        uint64_t end = nanos();
        require(ct_len == ct_expected && ss_len == 32 && recovered_len == ss_len && CRYPTO_memcmp(shared,recovered,ss_len) == 0,"shared-secret equality");
        OPENSSL_cleanse(shared,sizeof shared); OPENSSL_cleanse(recovered,sizeof recovered);
        EVP_PKEY_free(public); EVP_PKEY_free(private);
        if (i>=0) { gen[i]=generated-start; enc[i]=before_dec-before_enc; dec[i]=end-before_dec; }
    }
    qsort(gen,31,sizeof gen[0],compare);qsort(enc,31,sizeof enc[0],compare);qsort(dec,31,sizeof dec[0],compare);
    printf("{\"algorithm\":\"%s\",\"roundTrips\":31,\"publicBytes\":%zu,\"ciphertextBytes\":%zu,\"sharedBytes\":32,\"seedBytes\":64,\"spkiBytes\":%d,\"pkcs8Bytes\":%d,\"medianUs\":{\"keygen\":%.3f,\"encapsulate\":%.3f,\"decapsulate\":%.3f}}\n",name,pub_expected,ct_expected,spki_len,pkcs8_len,gen[15]/1000.,enc[15]/1000.,dec[15]/1000.);
}
int main(void) {
    measure("ML-KEM-512",800,768);measure("ML-KEM-768",1184,1088);measure("ML-KEM-1024",1568,1568);
}
