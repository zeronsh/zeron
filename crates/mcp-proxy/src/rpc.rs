use serde_json::{Value, json};
use std::collections::HashMap;

pub fn ok(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0", "id":id,"result":result})
}
pub fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
pub fn request(id: impl Into<Value>, method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id.into(),"method":method,"params":params})
}
pub fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","method":method,"params":params})
}
pub fn failure(request: &Value, message: &str) -> Option<Value> {
    let id = request.get("id")?.clone();
    Some(if request["method"] == "tools/call" {
        ok(
            id,
            json!({"content":[{"type":"text","text":message}],"isError":true}),
        )
    } else {
        error(id, -32000, message)
    })
}

#[derive(Default)]
pub struct Remap {
    next: u64,
    pub pending: HashMap<String, Value>,
}
impl Remap {
    pub fn outgoing(&mut self, mut message: Value) -> Value {
        if message.get("id").is_some() && message.get("method").is_some() {
            self.next += 1;
            let id = format!("zc:{}", self.next);
            self.pending.insert(id.clone(), message.clone());
            message["id"] = json!(id);
        }
        message
    }
    pub fn response(&mut self, mut message: Value) -> Option<(Value, Value)> {
        let original = self.pending.remove(message["id"].as_str()?)?;
        message["id"] = original["id"].clone();
        Some((message, original))
    }
    pub fn cancel(&self, client_id: &Value) -> Option<String> {
        self.pending
            .iter()
            .find(|(_, v)| &v["id"] == client_id)
            .map(|(k, _)| k.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ids_round_trip_even_when_client_uses_proxy_namespace() {
        let mut map = Remap::default();
        for id in [json!(7), json!("zp:init"), json!("zc:1")] {
            let req = request(id.clone(), "tools/call", json!({}));
            let sent = map.outgoing(req);
            assert_ne!(sent["id"], "zp:init");
            assert_eq!(map.cancel(&id), sent["id"].as_str().map(str::to_owned));
            let (response, _) = map.response(ok(sent["id"].clone(), json!({}))).unwrap();
            assert_eq!(response["id"], id);
        }
        assert!(map.response(ok(json!("zp:1"), json!({}))).is_none());
        assert!(map.pending.is_empty());
    }
}
