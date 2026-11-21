/* Thin adapter for the OpenSSL 3.6+ default provider. Key generation,
 * key validation, RFC 9881 encoding, randomness, and KEM operations are
 * provided by OpenSSL. The isolated library context selects seed-only DER
 * encoding without modifying other providers or the browser TLS stack. */
#include <limits.h>
#include <openssl/core_names.h>
#include <openssl/crypto.h>
#include <openssl/err.h>
#include <openssl/evp.h>
#include <openssl/provider.h>
#include <openssl/x509.h>
#include <string.h>

enum { RAW_PUBLIC = 1, RAW_SEED, SPKI, PKCS8 };
typedef struct {
    OSSL_LIB_CTX *lib;
    OSSL_PROVIDER *provider;
    const char *name;
    size_t public_len, ciphertext_len;
} Adapter;

static int open_adapter(Adapter *a, int level) {
    switch (level) {
    case 512:
        a->name = "ML-KEM-512";
        a->public_len = 800;
        a->ciphertext_len = 768;
        break;
    case 768:
        a->name = "ML-KEM-768";
        a->public_len = 1184;
        a->ciphertext_len = 1088;
        break;
    case 1024:
        a->name = "ML-KEM-1024";
        a->public_len = 1568;
        a->ciphertext_len = 1568;
        break;
    default:
        return 0;
    }
    char seed_only[] = "seed-only";
    OSSL_PARAM params[] = {
        OSSL_PARAM_construct_utf8_string(OSSL_PKEY_PARAM_ML_KEM_OUTPUT_FORMATS, seed_only, 0),
        OSSL_PARAM_construct_utf8_string(OSSL_PKEY_PARAM_ML_KEM_INPUT_FORMATS, seed_only, 0),
        OSSL_PARAM_construct_end()};
    a->lib = OSSL_LIB_CTX_new();
    if (!a->lib)
        return 0;
    a->provider = OSSL_PROVIDER_load_ex(a->lib, "default", params);
    return a->provider != NULL;
}
static void close_adapter(Adapter *a) {
    if (a->provider)
        OSSL_PROVIDER_unload(a->provider);
    OSSL_LIB_CTX_free(a->lib);
}

/* ML-KEM PKCS8 has a CHOICE of seed, expanded key, or both. The bridge
 * supports only seed-only imports. Distinguish a well-formed unsupported
 * choice from a malformed encoding using OpenSSL's ASN.1 decoder. */
static int unsupported_private_format(const Adapter *a, const unsigned char *input, size_t len) {
    if (!input || len == 0 || len > 65536)
        return 0;
    const unsigned char *end = input, *inner = NULL;
    PKCS8_PRIV_KEY_INFO *info = d2i_PKCS8_PRIV_KEY_INFO(NULL, &end, (long)len);
    const X509_ALGOR *algorithm = NULL;
    const ASN1_OBJECT *oid = NULL;
    int inner_len = 0, unsupported = 0, parameter_type = V_ASN1_UNDEF;
    if (!info || end != input + len || !PKCS8_pkey_get0(&oid, &inner, &inner_len, &algorithm, info))
        goto finish;
    X509_ALGOR_get0(NULL, &parameter_type, NULL, algorithm);
    const int expected_nid = a->public_len == 800    ? NID_ML_KEM_512
                             : a->public_len == 1184 ? NID_ML_KEM_768
                                                     : NID_ML_KEM_1024;
    const int expanded_len = a->public_len == 800 ? 1632 : a->public_len == 1184 ? 2400 : 3168;
    if (OBJ_obj2nid(oid) != expected_nid || parameter_type != V_ASN1_UNDEF || inner_len <= 0)
        goto finish;
    end = inner;
    if (*inner == V_ASN1_OCTET_STRING) {
        ASN1_OCTET_STRING *expanded = d2i_ASN1_OCTET_STRING(NULL, &end, inner_len);
        unsupported = expanded && end == inner + inner_len && ASN1_STRING_length(expanded) == expanded_len;
        if (expanded)
            OPENSSL_cleanse(ASN1_STRING_data(expanded), (size_t)ASN1_STRING_length(expanded));
        ASN1_OCTET_STRING_free(expanded);
    } else if (*inner == (V_ASN1_SEQUENCE | V_ASN1_CONSTRUCTED)) {
        STACK_OF(ASN1_TYPE) *items = d2i_ASN1_SEQUENCE_ANY(NULL, &end, inner_len);
        if (items && end == inner + inner_len && sk_ASN1_TYPE_num(items) == 2) {
            const ASN1_TYPE *seed = sk_ASN1_TYPE_value(items, 0), *expanded = sk_ASN1_TYPE_value(items, 1);
            unsupported = seed->type == V_ASN1_OCTET_STRING && expanded->type == V_ASN1_OCTET_STRING &&
                          ASN1_STRING_length(seed->value.octet_string) == 64 &&
                          ASN1_STRING_length(expanded->value.octet_string) == expanded_len;
        }
        if (items) {
            for (int i = 0; i < sk_ASN1_TYPE_num(items); i++) {
                ASN1_TYPE *item = sk_ASN1_TYPE_value(items, i);
                if (item->type == V_ASN1_OCTET_STRING)
                    OPENSSL_cleanse(ASN1_STRING_data(item->value.octet_string),
                                    (size_t)ASN1_STRING_length(item->value.octet_string));
            }
        }
        sk_ASN1_TYPE_pop_free(items, ASN1_TYPE_free);
    }
finish:
    PKCS8_PRIV_KEY_INFO_free(info);
    return unsupported;
}

static EVP_PKEY *load_key(const Adapter *a, int fmt, const unsigned char *in, size_t len) {
    if (!in || len == 0 || len > 65536)
        return NULL;
    EVP_PKEY *key = NULL;
    if (fmt == RAW_PUBLIC && len == a->public_len) {
        key = EVP_PKEY_new_raw_public_key_ex(a->lib, a->name, NULL, in, len);
    } else if (fmt == RAW_SEED && len == 64) {
        OSSL_PARAM params[] = {
            OSSL_PARAM_construct_octet_string(OSSL_PKEY_PARAM_ML_KEM_SEED, (void *)in, len),
            OSSL_PARAM_construct_end()};
        EVP_PKEY_CTX *ctx = EVP_PKEY_CTX_new_from_name(a->lib, a->name, NULL);
        if (ctx && EVP_PKEY_fromdata_init(ctx) == 1) {
            if (EVP_PKEY_fromdata(ctx, &key, EVP_PKEY_KEYPAIR, params) != 1) {
                EVP_PKEY_free(key);
                key = NULL;
            }
        }
        EVP_PKEY_CTX_free(ctx);
    } else if (fmt == SPKI || fmt == PKCS8) {
        const unsigned char *next = in;
        if (fmt == SPKI) {
            key = d2i_PUBKEY_ex(NULL, &next, (long)len, a->lib, NULL);
        } else {
            PKCS8_PRIV_KEY_INFO *info = d2i_PKCS8_PRIV_KEY_INFO(NULL, &next, (long)len);
            if (info)
                key = EVP_PKCS82PKEY_ex(info, a->lib, NULL);
            PKCS8_PRIV_KEY_INFO_free(info);
        }
        if (next != in + len || !key || !EVP_PKEY_is_a(key, a->name)) {
            EVP_PKEY_free(key);
            return NULL;
        }
    }
    if (key && (fmt == RAW_PUBLIC || fmt == SPKI)) {
        EVP_PKEY_CTX *ctx = EVP_PKEY_CTX_new_from_pkey(a->lib, key, NULL);
        int valid = ctx && EVP_PKEY_public_check(ctx) == 1;
        EVP_PKEY_CTX_free(ctx);
        if (!valid) {
            EVP_PKEY_free(key);
            key = NULL;
        }
    }
    return key;
}

static int store(EVP_PKEY *key, int fmt, unsigned char *out, size_t cap, size_t *written) {
    if (!key || !out || !written)
        return 0;
    if (fmt == RAW_PUBLIC) {
        *written = cap;
        return EVP_PKEY_get_raw_public_key(key, out, written) == 1;
    }
    if (fmt == RAW_SEED)
        return EVP_PKEY_get_octet_string_param(key, OSSL_PKEY_PARAM_ML_KEM_SEED, out, cap, written) == 1 &&
               *written == 64;
    PKCS8_PRIV_KEY_INFO *info = fmt == PKCS8 ? EVP_PKEY2PKCS8(key) : NULL;
    int n = fmt == SPKI ? i2d_PUBKEY(key, NULL) : (info ? i2d_PKCS8_PRIV_KEY_INFO(info, NULL) : 0);
    int ok = n > 0 && (size_t)n <= cap;
    if (ok) {
        unsigned char *next = out;
        ok = (fmt == SPKI ? i2d_PUBKEY(key, &next) : i2d_PKCS8_PRIV_KEY_INFO(info, &next)) == n;
        if (ok)
            *written = (size_t)n;
    }
    PKCS8_PRIV_KEY_INFO_free(info);
    return ok;
}

int pyro_mlkem_generate(int level, unsigned char *seed, size_t seed_cap, unsigned char *pub,
                        size_t pub_cap) {
    Adapter a = {0};
    int ok = 0;
    size_t n = 0;
    if (open_adapter(&a, level) && seed_cap == 64 && pub_cap == a.public_len) {
        EVP_PKEY *key = EVP_PKEY_Q_keygen(a.lib, NULL, a.name);
        ok = store(key, RAW_SEED, seed, seed_cap, &n) && store(key, RAW_PUBLIC, pub, pub_cap, &n) &&
             n == pub_cap;
        EVP_PKEY_free(key);
    }
    close_adapter(&a);
    return ok;
}

int pyro_mlkem_convert(int level, int from, int to, const unsigned char *in, size_t len, unsigned char *out,
                       size_t cap, size_t *written) {
    Adapter a = {0};
    int ok = 0;
    if (open_adapter(&a, level)) {
        if (from == PKCS8 && unsupported_private_format(&a, in, len)) {
            ok = -1;
        } else {
            EVP_PKEY *key = load_key(&a, from, in, len);
            if (key && (from == SPKI || from == PKCS8)) {
                unsigned char canonical[2048];
                size_t actual = 0;
                /* Accept precisely the specified DER algorithm identifier
                 * (absent parameters) and canonical seed/public encoding. */
                if (!store(key, from, canonical, sizeof canonical, &actual) || actual != len ||
                    CRYPTO_memcmp(in, canonical, actual) != 0) {
                    EVP_PKEY_free(key);
                    key = NULL;
                }
                OPENSSL_cleanse(canonical, sizeof canonical);
            }
            ok = store(key, to, out, cap, written);
            EVP_PKEY_free(key);
        }
    }
    close_adapter(&a);
    return ok;
}

int pyro_mlkem_encapsulate(int level, const unsigned char *pub, size_t pub_len, unsigned char *out,
                           size_t cap, unsigned char *secret, size_t secret_len) {
    Adapter a = {0};
    int ok = 0;
    if (open_adapter(&a, level) && cap == a.ciphertext_len && secret_len == 32) {
        EVP_PKEY *key = load_key(&a, RAW_PUBLIC, pub, pub_len);
        EVP_PKEY_CTX *ctx = key ? EVP_PKEY_CTX_new_from_pkey(a.lib, key, NULL) : NULL;
        size_t n = cap, s = secret_len;
        ok = ctx && EVP_PKEY_encapsulate_init(ctx, NULL) == 1 &&
             EVP_PKEY_encapsulate(ctx, out, &n, secret, &s) == 1 && n == cap && s == secret_len;
        EVP_PKEY_CTX_free(ctx);
        EVP_PKEY_free(key);
    }
    close_adapter(&a);
    return ok;
}

int pyro_mlkem_decapsulate(int level, const unsigned char *seed, size_t seed_len, const unsigned char *ct,
                           size_t ct_len, unsigned char *secret, size_t secret_len) {
    Adapter a = {0};
    int ok = 0;
    if (open_adapter(&a, level) && ct_len == a.ciphertext_len && secret_len == 32) {
        EVP_PKEY *key = load_key(&a, RAW_SEED, seed, seed_len);
        EVP_PKEY_CTX *ctx = key ? EVP_PKEY_CTX_new_from_pkey(a.lib, key, NULL) : NULL;
        size_t n = secret_len;
        ok = ctx && EVP_PKEY_decapsulate_init(ctx, NULL) == 1 &&
             EVP_PKEY_decapsulate(ctx, secret, &n, ct, ct_len) == 1 && n == secret_len;
        EVP_PKEY_CTX_free(ctx);
        EVP_PKEY_free(key);
    }
    close_adapter(&a);
    return ok;
}
void pyro_mlkem_cleanse(unsigned char *p, size_t len) { OPENSSL_cleanse(p, len); }
