//! Credential-free, native HTTP peer. Works through Windows .exe/.cmd launchers.
use base64::Engine as _;
use serde_json::json;
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::Duration,
};

fn main() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(30));
        std::process::exit(99);
    });
    let exe = std::env::current_exe().unwrap();
    let name = exe.file_name().unwrap().to_string_lossy();
    let v2 = !name.contains("v1");
    let version = if v2 { "2.0.22" } else { "1.18.34" };
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--version") {
        if name.contains("slow") {
            std::thread::sleep(Duration::from_secs(3));
        }
        if !name.contains("unknown") {
            println!("{version}");
        }
        return;
    }
    if name.contains("crash") {
        eprintln!("fixture plugin initialization failed");
        std::process::exit(7);
    }
    let port = args.windows(2).find(|pair| pair[0] == "--port").unwrap()[1]
        .parse::<u16>()
        .unwrap();
    let password = std::env::var("OPENCODE_SERVER_PASSWORD").unwrap();
    let username = std::env::var("OPENCODE_SERVER_USERNAME").unwrap();
    let record = json!({
        "cwd":std::env::current_dir().unwrap(), "username":username,
        "passwordsMatch":std::env::var("OPENCODE_PASSWORD").unwrap() == password,
        "config":std::env::var("OPENCODE_CONFIG_CONTENT").ok(),
    });
    std::fs::write("fixture-launch.json", record.to_string()).unwrap();
    let expected = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))
    );
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    for socket in listener.incoming() {
        let mut socket = socket.unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0; 4096];
        while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
            let Ok(count) = socket.read(&mut chunk) else {
                break;
            };
            if count == 0 || bytes.len() > 1024 * 1024 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
        }
        let request = String::from_utf8_lossy(&bytes);
        let authorized = request.lines().any(|line| {
            line.split_once(':').is_some_and(|(key, value)| {
                key.eq_ignore_ascii_case("authorization") && value.trim() == expected
            })
        });
        let path = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("")
            .split('?')
            .next()
            .unwrap();
        let response = if !authorized {
            (401, json!({"error":"auth"}))
        } else if (v2 && path == "/api/info") || (!v2 && path == "/global/health") {
            (200, json!({"version":version}))
        } else {
            match (v2, path) {
                (true, "/api/model") => (
                    200,
                    json!({"data":[{"providerID":"test","id":"model","name":"Native fixture","enabled":true}]}),
                ),
                (false, "/provider") => (
                    200,
                    json!({"all":[{"id":"test","models":{"model":{"name":"Native fixture"}}}],"connected":["test"]}),
                ),
                (true, "/api/agent") => (200, json!({"data":[]})),
                (true, "/api/command") => (200, json!({"data":[]})),
                (false, "/command") => (200, json!([])),
                _ => (404, json!({})),
            }
        };
        let body = response.1.to_string();
        let _ = write!(
            socket,
            "HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.0,
            body.len(),
            body
        );
    }
}
