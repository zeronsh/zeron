//! rigclient call <port> <data_dir> <method> <json>
//! rigclient watch <port> <data_dir> <method> <json> <seconds>
//! rigclient applywatch <port> <data_dir> <harness> <seconds>   (watch while applying)
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() {
    let a: Vec<String> = std::env::args().collect();
    let port: u16 = a[2].parse().unwrap();
    let dir = std::path::PathBuf::from(&a[3]);
    let deadline = Instant::now() + Duration::from_secs(60);
    let client = loop {
        match zeron_rpc::connect_ipc(port, Some(&dir)).await {
            Ok(c) => break std::sync::Arc::new(c),
            Err(e) if Instant::now() > deadline => panic!("connect: {e}"),
            Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    };
    let start = Instant::now();
    match a[1].as_str() {
        "call" => {
            let params: serde_json::Value = serde_json::from_str(&a[5]).unwrap();
            match client.call(&a[4], params).await {
                Ok(v) => println!("{}", serde_json::to_string(&v).unwrap()),
                Err(e) => {
                    println!("{}", serde_json::json!({"error": e.to_string()}));
                }
            }
        }
        "watch" | "applywatch" => {
            let (method, params, secs, harness) = if a[1] == "watch" {
                (a[4].clone(), serde_json::from_str(&a[5]).unwrap(), a[6].parse::<u64>().unwrap(), a.get(7).cloned())
            } else {
                ("WatchHarnessUpdates".to_string(), serde_json::json!({}), a[5].parse::<u64>().unwrap(), Some(a[4].clone()))
            };
            let mut rx = client.subscribe(&method, params).await.unwrap();
            let want = harness.clone();
            let filter = move |v: &serde_json::Value| -> serde_json::Value {
                match &want {
                    Some(h) => v.as_array().into_iter().flatten().find(|s| s["harness"] == h.as_str()).cloned().unwrap_or_default(),
                    None => v.clone(),
                }
            };
            if let Some(h) = harness.filter(|_| a[1] == "applywatch") {
                let c2 = client.clone();
                tokio::spawn(async move {
                    let t = Instant::now();
                    let r = c2.call("ApplyHarnessUpdate", serde_json::json!({"harness": h})).await;
                    println!("[{:7.2}s] APPLY RESULT {}", t.elapsed().as_secs_f64(), match r { Ok(v) => v.to_string(), Err(e) => format!("error: {e}") });
                });
            }
            let mut last = String::new();
            let end = tokio::time::sleep(Duration::from_secs(secs));
            tokio::pin!(end);
            loop {
                tokio::select! {
                    _ = &mut end => break,
                    item = rx.recv() => {
                        let Some(item) = item else { break };
                        let s = filter(&item);
                        let line = format!("phase={} installed={} latest={} canApply={} error={} wait={}",
                            s["phase"], s["installedVersion"], s["latestVersion"], s["canApply"],
                            s["error"]["message"], s["waitReason"]);
                        if line != last {
                            println!("[{:7.2}s] {line}", start.elapsed().as_secs_f64());
                            last = line;
                        }
                    }
                }
            }
        }
        _ => panic!("usage"),
    }
}
