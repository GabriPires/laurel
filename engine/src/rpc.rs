//! JSON-RPC 2.0 between the Laurel app and the engine (PoC-U1): one JSON object per
//! line, requests on stdin, responses and notifications on stdout. Only protocol
//! messages go to stdout; logs go to stderr. EOF on stdin means the app is gone, and
//! the engine shuts down. The lines are read on a thread of their own, so that the
//! health watch (health.rs) can stop the loop while it waits for the app.

use std::io::{BufRead, Write};
use std::sync::mpsc::{Receiver, Sender};

use serde_json::{json, Value};

pub enum Flow {
    Continue,
    Quit,
}

pub fn send(msg: Value) {
    let mut out = std::io::stdout().lock();
    // A closed stdout means the app is gone; the stdin EOF then ends the loop.
    let _ = writeln!(out, "{msg}").and_then(|_| out.flush());
}

pub fn notify(method: &str, params: Value) {
    send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
}

/// What the loop gets: a line from the app, the end of stdin, or a reason to stop
/// from elsewhere in the engine.
pub enum Event<T> {
    Line(String),
    Closed,
    Stop(T),
}

/// Reads the app's lines on a thread of its own and sends them to `tx`.
pub fn read_stdin<T: Send + 'static>(tx: Sender<Event<T>>) {
    let reader = std::thread::Builder::new().name("laurel: stdin".into()).spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(Event::Line(line)).is_err() {
                return;
            }
        }
        let _ = tx.send(Event::Closed);
    });
    if let Err(e) = reader {
        log!("[laurel] no thread to read the app's requests: {e}");
    }
}

/// Answers requests until stdin closes or `handle` asks to quit (None), or until a
/// `Stop` arrives (its reason). Requests without an id are treated as notifications
/// and get no response.
pub fn serve<T>(events: Receiver<Event<T>>, mut handle: impl FnMut(&str, &Value) -> (Result<Value, String>, Flow)) -> Option<T> {
    for event in events {
        let line = match event {
            Event::Line(line) => line,
            Event::Closed => return None,
            Event::Stop(reason) => return Some(reason),
        };
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                send(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": e.to_string() } }));
                continue;
            }
        };
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let method = req.get("method").and_then(Value::as_str).unwrap_or_default();
        let params = req.get("params").cloned().unwrap_or(Value::Null);
        let (result, flow) = handle(method, &params);
        if !id.is_null() {
            send(match result {
                Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
                Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": e } }),
            });
        }
        if let Flow::Quit = flow {
            return None;
        }
    }
    None
}
