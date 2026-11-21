fn main() {
    #[cfg(feature = "experimental-mlkem-openssl")]
    {
        use std::path::PathBuf;
        println!("cargo:rerun-if-env-changed=PYRO_MLKEM_OPENSSL_PREFIX");
        println!("cargo:rerun-if-changed=src/crypto_subtle/mlkem_bridge.c");
        let sdk = PathBuf::from(
            std::env::var_os("PYRO_MLKEM_OPENSSL_PREFIX")
                .expect("set PYRO_MLKEM_OPENSSL_PREFIX to an OpenSSL 3.6+ SDK"),
        );
        cc::Build::new()
            .file("src/crypto_subtle/mlkem_bridge.c")
            .include(sdk.join("include"))
            .compile("pyro_mlkem_bridge");
        println!("cargo:rustc-link-search=native={}", sdk.join("lib").display());
        println!("cargo:rustc-link-lib=dylib=crypto");
    }
}
