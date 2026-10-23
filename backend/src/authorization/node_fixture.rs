//! Optional socket owner for machines that block inbound Rust test executables.
//! The production reqwest client still crosses real HTTP. Every response is
//! produced by the original Axum test router; Node has no authorization logic.

use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::Duration,
};

use axum::{
    body::{to_bytes, Body},
    http::Request,
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

pub(super) struct Server {
    pub(super) origin: String,
    child: Child,
    reader: Option<thread::JoinHandle<()>>,
}

impl Server {
    pub(super) async fn start(router: Router) -> Self {
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || Self::start_blocking(router, runtime))
            .await
            .expect("start owned Node HTTP fixture worker")
    }

    fn start_blocking(router: Router, runtime: tokio::runtime::Handle) -> Self {
        let executable = std::env::var_os("WORK_ORDERS_TEST_NODE")
            .expect("WORK_ORDERS_TEST_NODE must name an installed Node executable");
        let mut command = Command::new(executable);
        command
            .arg("-e")
            .arg(SCRIPT)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().expect("start owned Node HTTP fixture");
        let input = Arc::new(Mutex::new(child.stdin.take().expect("fixture input pipe")));
        let output = child.stdout.take().expect("fixture output pipe");
        let (ready, startup) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut ready = Some(ready);
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                let Ok(event) = serde_json::from_str::<Value>(&line) else {
                    break;
                };
                if event["type"] == "ready" {
                    if let Some(sender) = ready.take() {
                        let _ =
                            sender.send(event["origin"].as_str().unwrap_or_default().to_owned());
                    }
                    continue;
                }
                if event["type"] != "request" {
                    break;
                }
                let router = router.clone();
                let input = input.clone();
                runtime.spawn(async move {
                    let id = event["id"].as_u64().expect("fixture request id");
                    let mut request = Request::builder()
                        .method(event["method"].as_str().expect("fixture method"))
                        .uri(event["uri"].as_str().expect("fixture URI"));
                    for header in event["headers"].as_array().expect("fixture headers") {
                        request = request.header(header[0].as_str().expect("fixture header name"),
                            header[1].as_str().expect("fixture header value"));
                    }
                    let bytes: Vec<u8> = serde_json::from_value(event["body"].clone()).expect("fixture body bytes");
                    let response = router.oneshot(request.body(Body::from(bytes)).unwrap()).await.unwrap();
                    let (parts, body) = response.into_parts();
                    let headers: Vec<_> = parts.headers.iter().map(|(name, value)|
                        (name.as_str(), value.to_str().expect("fixture response header"))).collect();
                    let bytes = to_bytes(body, 1024 * 1024).await.expect("bounded fixture response");
                    let result = json!({"id": id, "status": parts.status.as_u16(), "headers": headers, "body": bytes.to_vec()});
                    let mut pipe = input.lock().expect("fixture response input lock");
                    // A request can time out and close before its delayed reply.
                    if serde_json::to_writer(&mut *pipe, &result).is_ok() {
                        let _ = pipe.write_all(b"\n");
                        let _ = pipe.flush();
                    }
                });
            }
        });
        let mut server = Self {
            origin: String::new(),
            child,
            reader: Some(reader),
        };
        server.origin = startup
            .recv_timeout(Duration::from_secs(10))
            .expect("owned Node HTTP fixture readiness");
        let url = url::Url::parse(&server.origin).expect("fixture origin URL");
        assert_eq!(url.scheme(), "http");
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert!(url.port().is_some());
        assert_eq!(server.origin, url.origin().ascii_serialization());
        server
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

const SCRIPT: &str = r#"
'use strict';
const http = require('node:http');
const readline = require('node:readline');
const pending = new Map();
let next = 1;
const emit = value => process.stdout.write(JSON.stringify(value) + '\n');
const server = http.createServer((request, response) => {
  const chunks = []; let length = 0;
  request.on('data', chunk => { length += chunk.length; if (length > 65536) request.destroy(); else chunks.push(chunk); });
  request.on('end', () => {
    const id = next++;
    pending.set(id, response);
    response.once('close', () => pending.delete(id));
    const headers = [];
    for (let i = 0; i < request.rawHeaders.length; i += 2) headers.push(request.rawHeaders.slice(i, i + 2));
    emit({type:'request', id, method:request.method, uri:request.url, headers, body:Array.from(Buffer.concat(chunks))});
  });
});
const input = readline.createInterface({input:process.stdin});
input.on('line', line => {
  let message; try { message = JSON.parse(line); } catch { process.exit(2); }
  const response = pending.get(message.id); pending.delete(message.id);
  if (!response || response.destroyed) return;
  response.writeHead(message.status, message.headers.flat());
  response.end(Buffer.from(message.body));
});
input.on('close', () => process.exit(0));
server.on('error', () => process.exit(1));
server.listen(0, '127.0.0.1', () => emit({type:'ready', origin:'http://127.0.0.1:' + server.address().port}));
"#;
