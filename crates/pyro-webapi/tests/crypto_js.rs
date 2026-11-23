//! SubtleCrypto with CryptoKey (symmetric and asymmetric algorithms, spec
//! error names, structured clone), CompressionStream/DecompressionStream,
//! navigator.storage, and the streams, FileReader and CryptoKey globals of
//! dedicated workers.
use crate::support;
use pyro_webapi::join_stopped;
use std::time::Duration;
use support::network::*;

/// Runs `body` (an async function body that returns a string) and returns
/// what it resolved to, or `ERROR <name>: <message>`.
fn run(env: &Env, body: &str) -> String {
    env.eval(&format!(
        r#"globalThis.state='pending';
        globalThis.hex=(b)=>[...new Uint8Array(b)].map(x=>x.toString(16).padStart(2,'0')).join('');
        globalThis.unhex=(h)=>new Uint8Array(h.match(/../g).map(x=>parseInt(x,16)));
        globalThis.fails=async(p)=>{{try{{await p;return 'resolved'}}catch(e){{return e.name}}}};
        (async()=>{{ {body} }})().then(v=>state=String(v),e=>state='ERROR '+e.name+': '+e.message);"#
    ));
    env.until("state!=='pending'");
    env.eval("state")
}

#[test]
fn crypto_key_is_a_lazy_global_with_cached_slots() {
    let server = Server::new();
    let env = Env::new(&server.url);
    assert_eq!(env.eval("(d => typeof (d.get || d.value) === 'function' && !d.enumerable)(Object.getOwnPropertyDescriptor(globalThis,'CryptoKey'))"), "true");
    let out = run(
        &env,
        r#"const out=[typeof CryptoKey, 'value' in Object.getOwnPropertyDescriptor(globalThis,'CryptoKey')];
        try { new CryptoKey(); } catch (e) { out.push(e.name); }
        const key=await crypto.subtle.generateKey({name:'AES-CTR',length:256},true,['encrypt']);
        out.push(key instanceof CryptoKey, Object.prototype.toString.call(key), key.type, key.extractable,
          key.algorithm===key.algorithm, key.usages===key.usages, JSON.stringify(key.algorithm), key.usages.join());
        out.push(Object.keys(CryptoKey.prototype).join());
        out.push(crypto.subtle.importKey.length, crypto.subtle.unwrapKey.length, crypto.subtle.deriveBits.length);
        out.push(CryptoKey.length);
        return out.join('|');"#,
    );
    assert_eq!(
        out,
        "function|true|TypeError|true|[object CryptoKey]|secret|true|true|true|{\"name\":\"AES-CTR\",\"length\":256}|encrypt|type,extractable,algorithm,usages|5|7|2|0"
    );
}

#[test]
fn hmac_aes_and_key_derivation_match_reference_vectors() {
    let server = Server::new();
    let env = Env::new(&server.url);
    let out = run(
        &env,
        r#"const s=crypto.subtle, te=new TextEncoder(), out=[];
        // RFC 4231 test case 2.
        const hk=await s.importKey('raw',te.encode('Jefe'),{name:'HMAC',hash:'SHA-256'},true,['sign','verify']);
        const mac=await s.sign('HMAC',hk,te.encode('what do ya want for nothing?'));
        out.push(hex(mac), await s.verify('HMAC',hk,mac,te.encode('what do ya want for nothing?')),
          await s.verify('HMAC',hk,mac,te.encode('x')), hk.algorithm.length, hk.algorithm.hash.name);
        // RFC 3394 4.1 (AES-KW wrapping a raw AES key).
        const kek=await s.importKey('raw',unhex('000102030405060708090a0b0c0d0e0f'),'AES-KW',false,['wrapKey','unwrapKey']);
        const inner=await s.importKey('raw',unhex('00112233445566778899aabbccddeeff'),'AES-CBC',true,['encrypt']);
        const wrapped=await s.wrapKey('raw',inner,kek,'AES-KW');
        out.push(hex(wrapped));
        const back=await s.unwrapKey('raw',wrapped,kek,'AES-KW','AES-CBC',true,['decrypt']);
        out.push(hex(await s.exportKey('raw',back)), back.usages.join());
        // AES-GCM (GCM spec test case 3), CBC and CTR round trips.
        const gk=await s.importKey('raw',unhex('feffe9928665731c6d6a8f9467308308'),'AES-GCM',true,['encrypt','decrypt']);
        const iv=unhex('cafebabefacedbaddecaf888');
        const pt=unhex('d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b391aafd255');
        const ct=await s.encrypt({name:'AES-GCM',iv},gk,pt);
        out.push(hex(ct).slice(-32), hex(await s.decrypt({name:'AES-GCM',iv},gk,ct))===hex(pt));
        out.push(await fails(s.decrypt({name:'AES-GCM',iv,additionalData:new Uint8Array([1])},gk,ct)));
        out.push(new Uint8Array(await s.encrypt({name:'AES-GCM',iv,tagLength:96},gk,pt)).length-pt.length);
        const cbc=await s.generateKey({name:'AES-CBC',length:128},false,['encrypt','decrypt']);
        const civ=new Uint8Array(16);
        const cc=await s.encrypt({name:'AES-CBC',iv:civ},cbc,te.encode('sixteen bytes!!!'));
        out.push(cc.byteLength, new TextDecoder().decode(await s.decrypt({name:'AES-CBC',iv:civ},cbc,cc)));
        const ctr=await s.generateKey({name:'AES-CTR',length:192},false,['encrypt','decrypt']);
        const counter=new Uint8Array(16);
        const xc=await s.encrypt({name:'AES-CTR',counter,length:64},ctr,te.encode('ctr'));
        out.push(new TextDecoder().decode(await s.decrypt({name:'AES-CTR',counter,length:64},ctr,xc)));
        // RFC 6070 #2 and RFC 5869 #1.
        const pw=await s.importKey('raw',te.encode('password'),'PBKDF2',false,['deriveBits']);
        out.push(hex(await s.deriveBits({name:'PBKDF2',hash:'SHA-1',salt:te.encode('salt'),iterations:2},pw,160)));
        const ikm=await s.importKey('raw',unhex('0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b'),'HKDF',false,['deriveBits','deriveKey']);
        const hkdf={name:'HKDF',hash:'SHA-256',salt:unhex('000102030405060708090a0b0c'),info:unhex('f0f1f2f3f4f5f6f7f8f9')};
        out.push(hex(await s.deriveBits(hkdf,ikm,336)));
        // MSAL's cache key: HKDF -> AES-GCM, then encrypt/decrypt.
        const derived=await s.deriveKey(hkdf,ikm,{name:'AES-GCM',length:256},false,['encrypt','decrypt']);
        const enc=await s.encrypt({name:'AES-GCM',iv},derived,te.encode('cache'));
        out.push(JSON.stringify(derived.algorithm), new TextDecoder().decode(await s.decrypt({name:'AES-GCM',iv},derived,enc)));
        return out.join('|');"#,
    );
    let expected = [
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
        "true",
        "false",
        "32",
        "SHA-256",
        "1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5",
        "00112233445566778899aabbccddeeff",
        "decrypt",
        "4d5c2af327cd64a62cf35abd2ba6fab4",
        "true",
        "OperationError",
        "12",
        "32",
        "sixteen bytes!!!",
        "ctr",
        "ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957",
        "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865",
        "{\"name\":\"AES-GCM\",\"length\":256}",
        "cache",
    ];
    assert_eq!(out, expected.join("|"));
}

#[test]
fn subtle_reports_spec_error_names_in_spec_order() {
    let server = Server::new();
    let env = Env::new(&server.url);
    let out = run(
        &env,
        r#"const s=crypto.subtle, out=[];
        const raw=new Uint8Array(16);
        // Names match ASCII case-insensitively only (U+212A KELVIN SIGN is not K).
        out.push(await fails(s.generateKey({name:'AES-KW',length:256},false,['wrapKey'])));
        out.push((await s.generateKey({name:'aes-kw',length:128},false,['wrapKey'])).algorithm.name);
        out.push(await fails(s.generateKey({},false,['sign'])));
        out.push(await fails(s.generateKey({name:'HMAC',hash:'MD5'},false,['sign'])));
        out.push(await fails(s.generateKey({name:'AES-GCM',length:64},false,['encrypt'])));
        out.push(await fails(s.generateKey({name:'AES-GCM',length:2**32+128},false,['encrypt'])));
        out.push(await fails(s.generateKey({name:'AES-GCM',length:128},false,['sign'])));
        out.push(await fails(s.generateKey({name:'AES-GCM',length:128},false,[])));
        out.push(await fails(s.generateKey({name:'ECDSA',namedCurve:'P-512'},false,['sign'])));
        out.push(await fails(s.importKey('raw',raw,'AES-GCM',false,['encrypt','bogus'])));
        out.push(await fails(s.importKey('raw',new Uint8Array(15),'AES-GCM',false,['encrypt'])));
        out.push(await fails(s.importKey('spki',raw,'AES-GCM',false,['encrypt'])));
        out.push(await fails(s.importKey('jwk',raw,'AES-GCM',false,['encrypt'])));
        out.push(await fails(s.importKey('jwk',{kty:'oct',k:'AAAAAAAAAAAAAAAAAAAAAA',alg:'A256GCM'},'AES-GCM',false,['encrypt'])));
        out.push(await fails(s.importKey('raw',raw,'PBKDF2',true,['deriveBits'])));
        const hkdf=await s.importKey('raw',new Uint8Array(),'HKDF',false,['deriveBits']);
        out.push(await fails(s.exportKey('raw',hkdf)));
        const aes=await s.importKey('raw',raw,'AES-CBC',false,['encrypt']);
        out.push(await fails(s.exportKey('raw',aes)));
        out.push(await fails(s.encrypt({name:'AES-GCM',iv:raw},aes,raw)));
        out.push(await fails(s.decrypt({name:'AES-CBC',iv:raw},aes,raw)));
        out.push(await fails(s.encrypt({name:'AES-CBC',iv:new Uint8Array(8)},aes,raw)));
        out.push(await fails(s.encrypt({name:'AES-CBC'},aes,raw)));
        out.push(await fails(s.encrypt({name:'AES-CBC',iv:raw},{},raw)));
        out.push(await fails(s.deriveBits({name:'HKDF',hash:'SHA-256',salt:raw,info:raw},hkdf)));
        out.push(await fails(s.deriveBits({name:'HKDF',hash:'SHA-256',salt:raw,info:raw},hkdf,NaN)));
        out.push((await s.deriveBits({name:'HKDF',hash:'SHA-256',salt:raw,info:raw},hkdf,0)).byteLength);
        out.push(await fails(s.importKey('raw',raw,'AES-GCM',false)));
        // The hash is normalized after the dictionary: a missing salt comes first.
        out.push(await fails(s.deriveBits({name:'HKDF',hash:'SHA-999',info:raw},hkdf,256)));
        out.push(await fails(s.deriveBits({name:'HKDF',hash:'SHA-999',info:raw,salt:raw},hkdf,256)));
        // Enumerable Object.prototype members are not algorithm names.
        Object.prototype.FAKE='';
        out.push(await fails(s.generateKey({name:'HMAC',hash:'FAKE'},false,['sign'])), await fails(s.generateKey('FAKE',false,['sign'])));
        delete Object.prototype.FAKE;
        // Key data is copied after the algorithm is normalized.
        const data=new Uint8Array(16); data[0]=9;
        const key=await s.importKey('raw',data,{get name(){data[0]=1;return 'AES-GCM'}},true,['encrypt']);
        out.push(new Uint8Array(await s.exportKey('raw',key))[0]);
        return out.join('|');"#,
    );
    let expected = [
        "NotSupportedError",
        "AES-KW",
        "TypeError",
        "NotSupportedError",
        "OperationError",
        "TypeError",
        "SyntaxError",
        "SyntaxError",
        "NotSupportedError",
        "TypeError",
        "DataError",
        "NotSupportedError",
        "TypeError",
        "DataError",
        "SyntaxError",
        "NotSupportedError",
        "InvalidAccessError",
        "InvalidAccessError",
        "InvalidAccessError",
        "OperationError",
        "TypeError",
        "TypeError",
        "OperationError",
        "TypeError",
        "0",
        "TypeError",
        "TypeError",
        "NotSupportedError",
        "NotSupportedError",
        "NotSupportedError",
        "1",
    ];
    assert_eq!(out, expected.join("|"));
}

#[test]
fn asymmetric_algorithms_sign_derive_and_round_trip_keys() {
    let server = Server::new();
    let env = Env::new(&server.url);
    let out = run(
        &env,
        r#"const s=crypto.subtle, te=new TextEncoder(), out=[], msg=te.encode('message');
        for (const namedCurve of ['P-256','P-384','P-521']) {
          const pair=await s.generateKey({name:'ECDSA',namedCurve},true,['sign','verify']);
          const sig=await s.sign({name:'ECDSA',hash:'SHA-384'},pair.privateKey,msg);
          const jwk=await s.exportKey('jwk',pair.publicKey);
          const pub=await s.importKey('jwk',jwk,{name:'ECDSA',namedCurve},false,['verify']);
          const spki=await s.importKey('spki',await s.exportKey('spki',pair.publicKey),{name:'ECDSA',namedCurve},false,['verify']);
          out.push(namedCurve+':'+sig.byteLength+':'+(await s.verify({name:'ECDSA',hash:'SHA-384'},pub,sig,msg))
            +':'+(await s.verify({name:'ECDSA',hash:'SHA-384'},spki,sig,te.encode('other')))+':'+jwk.kty+jwk.crv+Object.keys(jwk).join(','));
        }
        const a=await s.generateKey({name:'ECDH',namedCurve:'P-256'},false,['deriveBits']);
        const b=await s.generateKey({name:'ECDH',namedCurve:'P-256'},false,['deriveBits']);
        const ab=await s.deriveBits({name:'ECDH',public:b.publicKey},a.privateKey,256);
        const ba=await s.deriveBits({name:'ECDH',public:a.publicKey},b.privateKey,null);
        out.push(hex(ab)===hex(ba), new Uint8Array(await s.deriveBits({name:'ECDH',public:b.publicKey},a.privateKey,230)).length,
          a.publicKey.usages.length, await fails(s.deriveBits({name:'ECDH',public:b.publicKey},a.privateKey,512)));
        // RFC 8032 7.1 test 1.
        const seed='9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60';
        const edPriv=await s.importKey('pkcs8',unhex('302e020100300506032b657004220420'+seed),'Ed25519',true,['sign']);
        const edSig=await s.sign('Ed25519',edPriv,new Uint8Array());
        const edJwk=await s.exportKey('jwk',edPriv);
        const edPub=await s.importKey('raw',unhex('d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a'),'Ed25519',true,['verify']);
        out.push(hex(edSig), await s.verify('Ed25519',edPub,edSig,new Uint8Array()), edJwk.alg+edJwk.crv+edJwk.kty,
          hex(await s.exportKey('pkcs8',edPriv)).endsWith(seed));
        // RFC 7748 6.1.
        const xPriv=await s.importKey('pkcs8',unhex('302e020100300506032b656e04220420'+'77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a'),'X25519',false,['deriveBits']);
        const xPub=await s.importKey('raw',unhex('de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f'),'X25519',true,[]);
        out.push(hex(await s.deriveBits({name:'X25519',public:xPub},xPriv,256)));
        out.push(await fails(s.importKey('raw',new Uint8Array(32),'X25519',true,['deriveBits'])));
        // RSA: generation, signatures, OAEP and JWK round trips.
        const rsa={modulusLength:1024,publicExponent:new Uint8Array([1,0,1]),hash:'SHA-256'};
        const pss=await s.generateKey({name:'RSA-PSS',...rsa},true,['sign','verify']);
        const psig=await s.sign({name:'RSA-PSS',saltLength:32},pss.privateKey,msg);
        out.push(pss.publicKey.algorithm.modulusLength, hex(pss.publicKey.algorithm.publicExponent), pss.privateKey.algorithm.hash.name,
          await s.verify({name:'RSA-PSS',saltLength:32},pss.publicKey,psig,msg));
        const pkcs=await s.generateKey({name:'RSASSA-PKCS1-v1_5',...rsa},true,['sign','verify']);
        const ksig=await s.sign('RSASSA-PKCS1-v1_5',pkcs.privateKey,msg);
        const pjwk=await s.exportKey('jwk',pkcs.privateKey);
        const reimported=await s.importKey('jwk',pjwk,{name:'RSASSA-PKCS1-v1_5',hash:'SHA-256'},true,['sign']);
        const ksig2=await s.sign('RSASSA-PKCS1-v1_5',reimported,msg);
        const pubFromSpki=await s.importKey('spki',await s.exportKey('spki',pkcs.publicKey),{name:'RSASSA-PKCS1-v1_5',hash:'SHA-256'},true,['verify']);
        out.push(pjwk.alg, hex(ksig)===hex(ksig2), await s.verify('RSASSA-PKCS1-v1_5',pubFromSpki,ksig,msg));
        out.push(await fails(s.sign('RSASSA-PKCS1-v1_5',pkcs.publicKey,msg)));
        const oaep=await s.generateKey({name:'RSA-OAEP',...rsa},false,['encrypt','decrypt']);
        const oc=await s.encrypt({name:'RSA-OAEP',label:te.encode('l')},oaep.publicKey,te.encode('secret'));
        out.push(new TextDecoder().decode(await s.decrypt({name:'RSA-OAEP',label:te.encode('l')},oaep.privateKey,oc)),
          oaep.publicKey.usages.join(), oaep.privateKey.usages.join(), oaep.publicKey.extractable);
        out.push(await fails(s.generateKey({name:'RSA-OAEP',...rsa,publicExponent:new Uint8Array([1,0,0])},false,['encrypt'])));
        return out.join('|');"#,
    );
    let expected = [
        "P-256:64:true:false:ECP-256crv,ext,key_ops,kty,x,y",
        "P-384:96:true:false:ECP-384crv,ext,key_ops,kty,x,y",
        "P-521:132:true:false:ECP-521crv,ext,key_ops,kty,x,y",
        "true",
        "29",
        "0",
        "OperationError",
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
        "true",
        "Ed25519Ed25519OKP",
        "true",
        "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742",
        "SyntaxError",
        "1024",
        "010001",
        "SHA-256",
        "true",
        "RS256",
        "true",
        "true",
        "InvalidAccessError",
        "secret",
        "encrypt",
        "decrypt",
        "true",
        "OperationError",
    ];
    assert_eq!(out, expected.join("|"));
}

/// The WPT RSA vectors (one 2048-bit key) that rsa.rs's unit tests use.
#[path = "../src/crypto_subtle/rsa/kat.rs"]
#[allow(dead_code)]
mod rsa_kat;

#[test]
fn rsa_matches_known_answers_through_subtle() {
    let server = Server::new();
    let env = Env::new(&server.url);
    let body = format!(
        r#"const s=crypto.subtle, out=[];
        const pkcs8=unhex('{pkcs8}'), spki=unhex('{spki}'), msg=unhex('{signed}');
        const pkcsAlg={{name:'RSASSA-PKCS1-v1_5',hash:'SHA-256'}}, pssAlg={{name:'RSA-PSS',hash:'SHA-256'}};
        const pkcsKey=await s.importKey('pkcs8',pkcs8,pkcsAlg,true,['sign']);
        const pkcsSig=await s.sign('RSASSA-PKCS1-v1_5',pkcsKey,msg);
        const pkcsPub=await s.importKey('spki',spki,pkcsAlg,true,['verify']);
        out.push(hex(pkcsSig)==='{pkcs1}', await s.verify('RSASSA-PKCS1-v1_5',pkcsPub,unhex('{pkcs1}'),msg),
          pkcsKey.algorithm.modulusLength, hex(await s.exportKey('pkcs8',pkcsKey))===hex(pkcs8));
        // The JWK round trip keeps every member, so it signs identically.
        const jwk=await s.exportKey('jwk',pkcsKey);
        const fromJwk=await s.importKey('jwk',jwk,pkcsAlg,true,['sign']);
        out.push(hex(await s.sign('RSASSA-PKCS1-v1_5',fromJwk,msg))==='{pkcs1}', hex(await s.exportKey('pkcs8',fromJwk))===hex(pkcs8));
        const badJwk={{...jwk,dp:jwk.dq}};
        out.push(await fails(s.importKey('jwk',badJwk,pkcsAlg,true,['sign'])));
        out.push(await fails(s.importKey('pkcs8',pkcs8.slice(0,-1),pkcsAlg,true,['sign'])));
        // saltLength 0 makes RSA-PSS deterministic.
        const pssKey=await s.importKey('pkcs8',pkcs8,pssAlg,false,['sign']);
        const pssPub=await s.importKey('spki',spki,pssAlg,false,['verify']);
        out.push(hex(await s.sign({{name:'RSA-PSS',saltLength:0}},pssKey,msg))==='{pss0}',
          await s.verify({{name:'RSA-PSS',saltLength:32}},pssPub,unhex('{pss32}'),msg),
          await s.verify({{name:'RSA-PSS',saltLength:31}},pssPub,unhex('{pss32}'),msg),
          await fails(s.sign({{name:'RSA-PSS',saltLength:223}},pssKey,msg)));
        const oaepAlg={{name:'RSA-OAEP',hash:'SHA-256'}};
        const oaepKey=await s.importKey('pkcs8',pkcs8,oaepAlg,false,['decrypt']);
        const oaepPub=await s.importKey('spki',spki,oaepAlg,false,['encrypt']);
        out.push(hex(await s.decrypt({{name:'RSA-OAEP',label:unhex('{label}')}},oaepKey,unhex('{oaep}')))==='{plain}',
          await fails(s.decrypt({{name:'RSA-OAEP'}},oaepKey,unhex('{oaep}'))));
        // Labels need not be UTF-8.
        const binary=new Uint8Array([0xff,0x00,0xfe]);
        const c=await s.encrypt({{name:'RSA-OAEP',label:binary}},oaepPub,unhex('{plain}'));
        out.push(c.byteLength, hex(await s.decrypt({{name:'RSA-OAEP',label:binary}},oaepKey,c))==='{plain}',
          await fails(s.encrypt({{name:'RSA-OAEP'}},oaepPub,unhex('{plain}00'))));
        return out.join('|');"#,
        pkcs8 = rsa_kat::PKCS8,
        spki = rsa_kat::SPKI,
        signed = rsa_kat::SIGNED,
        pkcs1 = rsa_kat::PKCS1_SIGNATURES[1],
        pss0 = rsa_kat::PSS_UNSALTED_SIGNATURES[1],
        pss32 = rsa_kat::PSS_SALTED_SIGNATURES[1],
        label = rsa_kat::OAEP_LABEL,
        oaep = rsa_kat::OAEP_LABELED_CIPHERTEXTS[1],
        plain = rsa_kat::OAEP_PLAINTEXTS[1],
    );
    let out = run(&env, &body);
    assert_eq!(
        out,
        "true|true|2048|true|true|true|DataError|DataError|true|true|false|OperationError|true|OperationError|256|true|OperationError"
    );
}

#[test]
fn crypto_keys_are_structured_cloneable() {
    let server = Server::new();
    let env = Env::new(&server.url);
    let out = run(
        &env,
        r#"const s=crypto.subtle, out=[];
        const hk=await s.generateKey({name:'HMAC',hash:'SHA-512'},true,['sign']);
        const pair=await s.generateKey({name:'ECDSA',namedCurve:'P-256'},false,['sign','verify']);
        const {key, nested:[priv, again]}=structuredClone({key:hk, nested:[pair.privateKey, pair.privateKey]});
        out.push(key instanceof CryptoKey, key!==hk, priv===again, priv.extractable, priv.usages.join(), JSON.stringify(key.algorithm));
        out.push(hex(await s.exportKey('raw',key))===hex(await s.exportKey('raw',hk)));
        const sig=await s.sign({name:'ECDSA',hash:'SHA-256'},priv,new Uint8Array([1]));
        out.push(await s.verify({name:'ECDSA',hash:'SHA-256'},pair.publicKey,sig,new Uint8Array([1])));
        // A forged tag is not a key.
        try { structuredClone({[Symbol.toStringTag]:'CryptoKey'}); out.push('cloned'); } catch (e) { out.push(e.name); }
        return out.join('|');"#,
    );
    assert_eq!(out, "true|true|true|false|sign|{\"name\":\"HMAC\",\"length\":1024,\"hash\":{\"name\":\"SHA-512\"}}|true|true|NotSupportedError");
}

#[test]
fn compression_streams_round_trip_every_format() {
    let server = Server::new();
    let env = Env::new(&server.url);
    assert_eq!(env.eval("(d => typeof (d.get || d.value) === 'function' && !d.enumerable)(Object.getOwnPropertyDescriptor(globalThis,'CompressionStream'))"), "true");
    let out = run(
        &env,
        r#"const out=[], te=new TextEncoder();
        const text='hello compression '.repeat(200);
        const pipe=async(stream,chunks)=>{
          const w=stream.writable.getWriter(); const r=stream.readable.getReader(); const parts=[];
          const reading=(async()=>{for(;;){const {value,done}=await r.read(); if(done) break; parts.push(value);}})();
          for (const c of chunks) w.write(c); w.close();
          await reading;
          const total=parts.reduce((n,p)=>n+p.length,0), all=new Uint8Array(total); let at=0;
          for (const p of parts) { if(!(p instanceof Uint8Array)) throw new Error('not bytes'); all.set(p,at); at+=p.length; }
          return all;
        };
        for (const format of ['gzip','deflate','deflate-raw']) {
          const packed=await pipe(new CompressionStream(format),[te.encode(text.slice(0,100)),te.encode(text.slice(100)).buffer]);
          // Decompress one byte at a time.
          const unpacked=await pipe(new DecompressionStream(format),[...packed].map(b=>new Uint8Array([b])));
          out.push(format+':'+(packed.length<text.length)+':'+(new TextDecoder().decode(unpacked)===text)+':'+(format==='deflate-raw'?'':hex(packed.slice(0,2))));
        }
        const viaBlob=await new Response(new Blob([await pipe(new CompressionStream('gzip'),[te.encode('blob')])]).stream()
          .pipeThrough(new DecompressionStream('gzip'))).text();
        out.push(viaBlob);
        try { new CompressionStream('brotli'); } catch (e) { out.push(e.name); }
        try { new DecompressionStream(); } catch (e) { out.push(e.name); }
        out.push(await fails(pipe(new DecompressionStream('deflate'),[new Uint8Array([1,2,3,4])])));
        out.push(await fails(pipe(new CompressionStream('deflate'),['text'])));
        const good=await pipe(new CompressionStream('deflate'),[te.encode('abc')]);
        out.push(await fails(pipe(new DecompressionStream('deflate'),[good.slice(0,-1)])));
        out.push(await fails(pipe(new DecompressionStream('deflate'),[good,new Uint8Array([0])])));
        out.push(Object.prototype.toString.call(new CompressionStream('gzip')));
        return out.join('|');"#,
    );
    assert_eq!(
        out,
        "gzip:true:true:1f8b|deflate:true:true:789c|deflate-raw:true:true:|blob|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|[object CompressionStream]"
    );
}

#[test]
fn navigator_storage_reports_usage_and_no_persistence() {
    let server = Server::new();
    let env = Env::new(&server.url);
    let out = run(
        &env,
        r#"const out=[navigator.storage===navigator.storage, navigator.storage instanceof StorageManager];
        localStorage.setItem('ab','cd');
        const e=await navigator.storage.estimate();
        out.push(Object.keys(e).join(), e.usage, e.quota>1e9, await navigator.storage.persisted(), await navigator.storage.persist());
        out.push(await fails(StorageManager.prototype.estimate.call({})));
        try { new StorageManager(); } catch (err) { out.push(err.name); }
        out.push(Object.keys(StorageManager.prototype).join(), StorageManager.length);
        return out.join('|');"#,
    );
    assert_eq!(
        out,
        "true|true|quota,usage,usageDetails|8|true|false|false|TypeError|TypeError|estimate,persisted,getDirectory,persist|0"
    );
}

#[test]
fn dedicated_worker_file_system_interfaces_are_own_data_properties() {
    let server = Server::new();
    let env = Env::new(&server.url);
    let script = r#"
        const names = ['FileSystemHandle', 'FileSystemDirectoryHandle', 'FileSystemFileHandle',
                       'FileSystemWritableFileStream', 'FileSystemSyncAccessHandle'];
        const valid = names.every(name => {
            const d = Object.getOwnPropertyDescriptor(self, name);
            return d && !('get' in d) && !('set' in d) && d.writable && !d.enumerable && d.configurable &&
                typeof d.value === 'function' && d.value === self[name] &&
                !Object.hasOwn(WorkerGlobalScope.prototype, name) &&
                !Object.hasOwn(DedicatedWorkerGlobalScope.prototype, name);
        });
        self.FileSystemHandle = 42;
        postMessage([valid, self.FileSystemHandle === 42,
                     delete self.FileSystemHandle, !('FileSystemHandle' in self)].join());
    "#;
    env.eval(&format!(
        "globalThis.answer = ''; const worker = new Worker(URL.createObjectURL(new Blob([{}], {{type:'text/javascript'}})));
         worker.onmessage = e => answer = e.data; worker.onerror = e => answer = e.message;",
        serde_json_string(script)
    ));
    env.until("answer.length > 0");
    assert_eq!(env.eval("answer"), "true,true,true,true");
}

#[test]
fn dedicated_workers_expose_streams_file_reader_crypto_keys_and_storage() {
    let server = Server::new();
    let env = Env::new(&server.url);
    let script = r#"(async () => {
        const out = [typeof ReadableStream, typeof TransformStream, typeof WritableStream, typeof CountQueuingStrategy,
          typeof TextEncoderStream, typeof CompressionStream, typeof FileReader, typeof CryptoKey, typeof StorageManager];
        const chunks = [];
        await new ReadableStream({start(c) { c.enqueue('a'); c.enqueue('b'); c.close(); }})
          .pipeThrough(new TransformStream({transform(ch, c) { c.enqueue(ch.toUpperCase()); }}))
          .pipeTo(new WritableStream({write(ch) { chunks.push(ch); }}));
        out.push(chunks.join(''));
        out.push(await new Response(new Blob(['zip']).stream().pipeThrough(new CompressionStream('gzip'))
          .pipeThrough(new DecompressionStream('gzip'))).text());
        out.push(await new Promise((resolve) => {
          const r = new FileReader(); r.onload = () => resolve(r.result); r.readAsText(new Blob(['read']));
        }));
        const key = await crypto.subtle.generateKey({name: 'HMAC', hash: 'SHA-256'}, false, ['sign']);
        out.push(key instanceof CryptoKey, (await crypto.subtle.sign('HMAC', key, new Uint8Array(1))).byteLength);
        out.push(navigator.storage instanceof StorageManager, await navigator.storage.persisted(), 'persist' in navigator.storage);
        postMessage(out.join());
      })().catch((e) => postMessage('error ' + e));"#;
    env.eval(&format!(
        "globalThis.msgs=[]; globalThis.w=new Worker(URL.createObjectURL(new Blob([{}],{{type:'text/javascript'}})));
         w.onmessage=e=>msgs.push(e.data); w.onerror=e=>msgs.push('error:'+e.message);",
        serde_json_string(script)
    ));
    env.until("msgs.length >= 1");
    assert_eq!(
        env.eval("msgs[0]"),
        "function,function,function,function,function,function,function,function,function,AB,zip,read,true,32,true,false,false"
    );
    env.eval("w.terminate()");
    assert!(join_stopped(Duration::from_secs(5)), "the worker thread ends after terminate()");
}

fn serde_json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[test]
fn file_stream_writes_observe_queued_bytes_and_abort_preserves_the_file() {
    let server = Server::new();
    let env = Env::new(&server.url);
    assert_eq!(run(&env, r#"
        const root = await navigator.storage.getDirectory();
        const entry = await root.getFileHandle('sequence.bin', {create:true});
        const bytes = async () => Array.from(new Uint8Array(await (await entry.getFile()).arrayBuffer())).join(',');
        const stream = await entry.createWritable();
        const writer = stream.getWriter(), view = Uint8Array.of(2);
        const first = writer.write(new Blob([Uint8Array.of(1)]));
        const second = writer.write(view);
        view[0] = 9;
        await Promise.all([first, second]);
        const beforeClose = await bytes();
        await writer.close();
        const discard = await entry.createWritable({keepExistingData:true});
        await discard.write(Uint8Array.of(7));
        await discard.abort();
        const final = await bytes();
        return [beforeClose, final, await root.isSameEntry(await navigator.storage.getDirectory())].join('|');
    "#), "|1,9|true");
}
