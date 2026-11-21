import Foundation
import CryptoKit
@available(macOS 26, *)
func run<Key: KEMPrivateKey>(_ keyType: Key.Type, _ name: String) throws {
    var kg:[UInt64]=[], enc:[UInt64]=[], dec:[UInt64]=[]
    var size=0
    for i in 0...31 {
        let start=DispatchTime.now().uptimeNanoseconds
        let key=try Key.generate()
        let pk=key.publicKey
        let a=DispatchTime.now().uptimeNanoseconds
        let box=try pk.encapsulate()
        let b=DispatchTime.now().uptimeNanoseconds
        let recovered=try key.decapsulate(box.encapsulated)
        let c=DispatchTime.now().uptimeNanoseconds
        precondition(box.sharedSecret == recovered && recovered.bitCount == 256)
        size=box.encapsulated.count
        if i>0 { kg.append(a-start); enc.append(b-a); dec.append(c-b) }
    }
    let result:[String:Any]=["algorithm":name,"roundTrips":31,"ciphertextBytes":size,"sharedBytes":32,
        "medianNs":["keygen":kg.sorted()[15],"encapsulate":enc.sorted()[15],"decapsulate":dec.sorted()[15]]]
    print(String(decoding:try JSONSerialization.data(withJSONObject:result,options:.sortedKeys),as:UTF8.self))
}
if #available(macOS 26, *) {
    try run(MLKEM768.PrivateKey.self,"ML-KEM-768")
    try run(MLKEM1024.PrivateKey.self,"ML-KEM-1024")
}
