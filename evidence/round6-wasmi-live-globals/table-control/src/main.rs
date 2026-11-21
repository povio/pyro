use wasmi::{Config,Engine,Extern,Func,Instance,Module,Nullable,Ref,RefType,Store,Table,TableType};
fn main(){
 let mut cfg=Config::default();cfg.compilation_mode(wasmi::CompilationMode::Eager).consume_fuel(true);
 let engine=Engine::new(&cfg);let mut s=Store::new(&engine,());s.set_fuel(500_000_000).unwrap();
 let t=Table::new(&mut s,TableType::new(RefType::Func,1,Some(4)),Ref::null(RefType::Func)).unwrap();
 let m=Module::new(&engine,include_bytes!("../../fixtures/table.wasm")).unwrap();
 let a=Instance::new(&mut s,&m,&[Extern::Table(t)]).unwrap();let b=Instance::new(&mut s,&m,&[Extern::Table(t)]).unwrap();
 let a_run=a.get_typed_func::<i32,i32>(&s,"run").unwrap();let b_run=b.get_typed_func::<i32,i32>(&s,"run").unwrap();
 let one=Func::wrap(&mut s,|x:i32|x+1);
 t.set(&mut s,0,Ref::from(Nullable::Val(one))).unwrap();
 assert_eq!(a_run.call(&mut s,9).unwrap(),10);
 let alias=b.get_table(&s,"t").unwrap();let two=Func::wrap(&mut s,|x:i32|x*2);
 alias.set(&mut s,0,Ref::from(Nullable::Val(two))).unwrap();
 assert_eq!(a_run.call(&mut s,9).unwrap(),18);assert_eq!(b_run.call(&mut s,7).unwrap(),14);
 assert_eq!(t.grow(&mut s,1,Ref::null(RefType::Func)).unwrap(),1);assert_eq!(alias.size(&s),2);
 println!("Native Wasmi only: imported and re-exported funcref table shared across two instances; returns 10,18,14; grown length 2.");
}
