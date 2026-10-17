use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn fetch(body: &'static str, args: &[&str]) -> std::process::Output {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                            break;
                        }
                    }
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("fixture failed: {e}"),
            }
        }
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_lightningpanda"))
        .arg("fetch")
        .args(args)
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            server.join().unwrap();
            panic!("fetch failed to enforce its JavaScript timeout");
        }
        thread::sleep(Duration::from_millis(10));
    }
    server.join().unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn fetch_deadline_interrupts_parser_script_and_timer_script() {
    for body in ["<script>while (true) {}</script>", "<script>setTimeout(() => { while (true) {} }, 0)</script>"] {
        let output = fetch(body, &["--timeout-ms", "200", "--wait-ms", "1000"]);
        assert_eq!(output.status.code(), Some(124), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.stdout.is_empty(), "timeout should not dump an incomplete document");
    }
}

#[test]
fn fetch_dumps_script_modified_document() {
    let output = fetch("<body><script>document.body.setAttribute('data-loaded', 'yes')</script></body>", &[]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("data-loaded=\"yes\""));
}
