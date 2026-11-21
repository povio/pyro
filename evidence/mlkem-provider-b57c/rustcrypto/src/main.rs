use ml_kem::{kem::{Decapsulate,Encapsulate},KemCore,MlKem512,MlKem768,MlKem1024};
use rand_core::OsRng;
use std::time::Instant;
fn sample<K:KemCore>(name:&str) {
 let mut kg=Vec::new(); let mut enc=Vec::new(); let mut dec=Vec::new();
 for i in 0..32 {
  let start=Instant::now();let (sk,pk)=K::generate(&mut OsRng);let t1=Instant::now();
  let (ct,shared)=pk.encapsulate(&mut OsRng).unwrap();let t2=Instant::now();
  let recovered=sk.decapsulate(&ct).unwrap();let t3=Instant::now();
  assert_eq!(shared.as_slice(),recovered.as_slice());assert_eq!(shared.len(),32);
  if i>0 {kg.push((t1-start).as_nanos());enc.push((t2-t1).as_nanos());dec.push((t3-t2).as_nanos());}
 }
 kg.sort_unstable();enc.sort_unstable();dec.sort_unstable();
 println!("{{\"algorithm\":\"{name}\",\"roundTrips\":31,\"medianNs\":{{\"keygen\":{},\"encapsulate\":{},\"decapsulate\":{}}}}}",kg[15],enc[15],dec[15]);
}
fn main(){sample::<MlKem512>("ML-KEM-512");sample::<MlKem768>("ML-KEM-768");sample::<MlKem1024>("ML-KEM-1024");}
