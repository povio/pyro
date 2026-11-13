//! `serve --parent-pid`: the server exits once the process it runs for has.

use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[test]
fn serve_exits_with_the_parent_pid_process() {
    let mut parent = Command::new("sleep").arg("2").spawn().unwrap();
    let port = free_port();
    let mut server = Command::new(env!("CARGO_BIN_EXE_pyro"))
        .args(["serve", "--port", &port.to_string(), "--parent-pid", &parent.id().to_string()])
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "server did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(server.try_wait().unwrap().is_none(), "serves while the process lives");
    parent.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = server.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = server.kill();
            panic!("server still runs after its parent pid exited");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status}");
}

#[test]
fn serve_exits_at_once_when_the_parent_pid_is_gone() {
    let mut exited = Command::new("true").spawn().unwrap();
    let pid = exited.id();
    exited.wait().unwrap();
    let mut server = Command::new(env!("CARGO_BIN_EXE_pyro"))
        .args(["serve", "--port", &free_port().to_string(), "--parent-pid", &pid.to_string()])
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while server.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = server.kill();
            panic!("server runs for a process that is gone");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        Command::new(env!("CARGO_BIN_EXE_pyro"))
            .args(["serve", "--parent-pid", "0"])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .code()
            == Some(2)
    );
}
