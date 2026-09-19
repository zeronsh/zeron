use std::{collections::BTreeMap, path::Path};

use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zeron_doc::registry::{OpKind, RegistryRow, RowOp, apply_op};

use crate::{
    Node, NodeRole, PrivateConfig,
    config::{PairRequest, valid_id, valid_name},
    digest, now_ms,
};

pub(crate) struct Store {
    pub db: Connection,
}

#[derive(Deserialize)]
pub(crate) struct RegistryPush {
    pub batch: String,
    pub ops: Vec<RowOp>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatStats {
    pub head_seq: u64,
    pub seq_floor: u64,
    pub checkpoint_seq: u64,
    pub checkpoint_size: u64,
    pub checkpoint_at: i64,
    pub row_count: u64,
    pub row_bytes: u64,
}

pub(crate) struct ChatRow {
    pub seq: u64,
    pub device: String,
    pub batch: String,
    pub bytes: Vec<u8>,
}

impl Store {
    pub fn open(data_dir: &Path, config: &PrivateConfig) -> Result<Self> {
        let directory = data_dir.join("profiles/private").join(&config.workspace_id);
        std::fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        let db = Connection::open(directory.join("hub.sqlite"))?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS nodes (id TEXT PRIMARY KEY,name TEXT NOT NULL,role TEXT NOT NULL,paired_at INTEGER NOT NULL,token_hash TEXT NOT NULL UNIQUE,revoked INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS invitation (singleton INTEGER PRIMARY KEY CHECK(singleton=1),hash TEXT NOT NULL,role TEXT NOT NULL,expires_at INTEGER NOT NULL,attempts INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS registry (kind TEXT NOT NULL,id TEXT NOT NULL,seq INTEGER NOT NULL,json TEXT NOT NULL,PRIMARY KEY(kind,id));
            CREATE INDEX IF NOT EXISTS registry_seq ON registry(seq);
            CREATE TABLE IF NOT EXISTS chats (id TEXT PRIMARY KEY,head INTEGER NOT NULL DEFAULT 0,floor INTEGER NOT NULL DEFAULT 0,checkpoint BLOB,frontier BLOB,checkpoint_at INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS chat_rows (chat TEXT NOT NULL,seq INTEGER NOT NULL,device TEXT NOT NULL,batch TEXT NOT NULL,bytes BLOB NOT NULL,PRIMARY KEY(chat,seq),UNIQUE(chat,batch));
            CREATE TABLE IF NOT EXISTS chat_batches (chat TEXT NOT NULL,batch TEXT NOT NULL,seq INTEGER NOT NULL,PRIMARY KEY(chat,batch));
            CREATE TABLE IF NOT EXISTS blobs (key TEXT PRIMARY KEY,content_type TEXT NOT NULL,bytes BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS nudges (device TEXT NOT NULL,chat TEXT NOT NULL,queued_at INTEGER NOT NULL,PRIMARY KEY(device,chat));")?;
        let store = Self { db };
        let identity: Option<String> = store
            .db
            .query_row("SELECT value FROM meta WHERE key='workspace'", [], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(identity) = identity {
            ensure!(
                identity == config.workspace_id,
                "Hub database belongs to another workspace"
            );
        } else {
            store.db.execute(
                "INSERT INTO meta VALUES('workspace',?)",
                [&config.workspace_id],
            )?;
        }
        Ok(store)
    }

    pub fn enroll(&mut self, node: &Node, token: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO nodes(id,name,role,paired_at,token_hash) VALUES(?,?,?,?,?)",
            params![
                node.device_id,
                node.name,
                node.role.to_string(),
                node.paired_at,
                digest(token)
            ],
        )?;
        Ok(())
    }

    pub fn nodes(&self) -> Result<Vec<Node>> {
        let mut stmt = self.db.prepare(
            "SELECT id,name,role,paired_at FROM nodes WHERE revoked=0 ORDER BY paired_at,id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (device_id, name, role, paired_at) = row?;
            Ok(Node {
                device_id,
                name,
                role: parse_role(&role)?,
                paired_at,
            })
        })
        .collect()
    }

    pub fn authenticate(&self, token: &str) -> Result<Option<Node>> {
        let record: Option<(String, String, String, i64)> = self
            .db
            .query_row(
                "SELECT id,name,role,paired_at FROM nodes WHERE token_hash=? AND revoked=0",
                [digest(token)],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        record
            .map(|(device_id, name, role, paired_at)| {
                Ok(Node {
                    device_id,
                    name,
                    role: parse_role(&role)?,
                    paired_at,
                })
            })
            .transpose()
    }

    pub fn active(&self, device: &str, credential_hash: &str) -> bool {
        self.db
            .query_row(
                "SELECT 1 FROM nodes WHERE id=? AND token_hash=? AND revoked=0",
                params![device, credential_hash],
                |_| Ok(()),
            )
            .optional()
            .is_ok_and(|r| r.is_some())
    }

    pub fn node(&self, device: &str) -> Result<Option<Node>> {
        Ok(self
            .nodes()?
            .into_iter()
            .find(|node| node.device_id == device))
    }

    pub fn revoke(&self, device: &str) -> Result<()> {
        ensure!(
            self.db.execute(
                "UPDATE nodes SET revoked=1 WHERE id=? AND revoked=0",
                [device]
            )? == 1,
            "Node is not enrolled"
        );
        Ok(())
    }

    pub fn invite(&self, role: NodeRole) -> Result<(String, i64)> {
        let code = format!("{:06}", uuid::Uuid::new_v4().as_u128() % 1_000_000);
        let expires = now_ms() + 5 * 60 * 1000;
        self.db.execute("INSERT OR REPLACE INTO invitation(singleton,hash,role,expires_at,attempts) VALUES(1,?,?,?,0)", params![digest(&code),role.to_string(),expires])?;
        Ok((code, expires))
    }

    pub fn pair(&mut self, request: &PairRequest) -> Result<Option<String>> {
        let transaction = self.db.transaction()?;
        let invitation: Option<(String, String, i64, i64)> = transaction
            .query_row(
                "SELECT hash,role,expires_at,attempts FROM invitation WHERE singleton=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((hash, role, expires, attempts)) = invitation else {
            return Ok(None);
        };
        if expires < now_ms() || attempts >= 5 {
            return Ok(None);
        }
        if digest(&request.code) != hash
            || request.role.to_string() != role
            || !valid_id(&request.device_id)
            || valid_name(&request.name).is_err()
        {
            transaction.execute(
                "UPDATE invitation SET attempts=attempts+1 WHERE singleton=1",
                [],
            )?;
            transaction.commit()?;
            return Ok(None);
        }
        let token = crate::new_token();
        transaction.execute(
            "INSERT INTO nodes(id,name,role,paired_at,token_hash) VALUES(?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,role=excluded.role,paired_at=excluded.paired_at,token_hash=excluded.token_hash,revoked=0",
            params![
                request.device_id,
                request.name,
                role,
                now_ms(),
                digest(&token)
            ],
        )?;
        transaction.execute("DELETE FROM invitation", [])?;
        transaction.commit()?;
        Ok(Some(token))
    }

    pub fn registry_seq(&self) -> Result<u64> {
        Ok(self
            .db
            .query_row("SELECT COALESCE(MAX(seq),0) FROM registry", [], |r| {
                r.get(0)
            })?)
    }

    pub fn registry_rows(&self, cursor: Option<u64>) -> Result<Value> {
        let seq = self.registry_seq()?;
        let full = cursor.is_none_or(|c| c > seq);
        let mut stmt = self
            .db
            .prepare("SELECT json FROM registry WHERE seq>? ORDER BY seq,kind,id")?;
        let rows = stmt.query_map([if full { 0 } else { cursor.unwrap_or(0) }], |r| {
            r.get::<_, String>(0)
        })?;
        let values: Vec<Value> = rows
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect::<Result<_>>()?;
        Ok(json!({"seq":seq,"full":full,"gcFloor":0,"rows":values}))
    }

    pub fn registry_push(&mut self, push: RegistryPush) -> Result<(Value, Option<Value>)> {
        validate_registry_push(&push)?;
        let seq = self.registry_seq()?;
        let tx = self.db.transaction()?;
        let mut touched: BTreeMap<(String, String), RegistryRow> = BTreeMap::new();
        let mut applied = 0;
        for op in push.ops {
            let key = (op.kind.clone(), op.id.clone());
            let previous = if let Some(row) = touched.get(&key) {
                Some(row.clone())
            } else {
                let raw: Option<String> = tx
                    .query_row(
                        "SELECT json FROM registry WHERE kind=? AND id=?",
                        params![op.kind, op.id],
                        |r| r.get(0),
                    )
                    .optional()?;
                raw.map(|raw| serde_json::from_str(&raw)).transpose()?
            };
            let (row, changed) = apply_op(previous.as_ref(), &op);
            if changed && let Some(mut row) = row {
                row.seq = seq + 1;
                touched.insert(key, row);
                applied += 1;
            }
        }
        for row in touched.values() {
            tx.execute("INSERT INTO registry(kind,id,seq,json) VALUES(?,?,?,?) ON CONFLICT(kind,id) DO UPDATE SET seq=excluded.seq,json=excluded.json", params![row.kind,row.id,row.seq,serde_json::to_string(row)?])?;
        }
        tx.commit()?;
        let next = if applied > 0 { seq + 1 } else { seq };
        let broadcast = (applied > 0)
            .then(|| json!({"t":"rows","seq":next,"rows":touched.values().collect::<Vec<_>>()}));
        Ok((
            json!({"batch":push.batch,"seq":next,"applied":applied}),
            broadcast,
        ))
    }

    pub fn chat_stats(&self, chat: &str) -> Result<ChatStats> {
        self.db
            .execute("INSERT OR IGNORE INTO chats(id) VALUES(?)", [chat])?;
        let (head_seq, seq_floor, checkpoint_size, checkpoint_at) = self.db.query_row(
            "SELECT head,floor,COALESCE(LENGTH(checkpoint),0),checkpoint_at FROM chats WHERE id=?",
            [chat],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let (row_count, row_bytes) = self.db.query_row(
            "SELECT COUNT(*),COALESCE(SUM(LENGTH(bytes)),0) FROM chat_rows WHERE chat=?",
            [chat],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(ChatStats {
            head_seq,
            seq_floor,
            checkpoint_seq: seq_floor,
            checkpoint_size,
            checkpoint_at,
            row_count,
            row_bytes,
        })
    }

    pub fn frontier(&self, chat: &str) -> Result<Vec<u8>> {
        Ok(self
            .db
            .query_row("SELECT frontier FROM chats WHERE id=?", [chat], |r| {
                r.get::<_, Option<Vec<u8>>>(0)
            })
            .optional()?
            .flatten()
            .unwrap_or_default())
    }

    pub fn chat_rows(
        &self,
        chat: &str,
        after: u64,
        head: u64,
        exclude: Option<&str>,
    ) -> Result<(Vec<ChatRow>, bool)> {
        let mut stmt = self.db.prepare("SELECT seq,device,batch,bytes FROM chat_rows WHERE chat=? AND seq>? AND seq<=? AND (? IS NULL OR device!=?) ORDER BY seq")?;
        let rows = stmt.query_map(params![chat, after, head, exclude, exclude], |r| {
            Ok(ChatRow {
                seq: r.get(0)?,
                device: r.get(1)?,
                batch: r.get(2)?,
                bytes: r.get(3)?,
            })
        })?;
        let mut page = Vec::new();
        let mut bytes = 0;
        for row in rows {
            let row = row?;
            if bytes + row.bytes.len() > 4 * 1024 * 1024 {
                return Ok((page, true));
            }
            bytes += row.bytes.len();
            page.push(row);
        }
        Ok((page, false))
    }

    pub fn append_chat(
        &mut self,
        chat: &str,
        device: &str,
        batch: &str,
        bytes: &[u8],
    ) -> Result<(u64, bool)> {
        ensure!(
            !batch.is_empty()
                && batch.len() <= 128
                && !bytes.is_empty()
                && bytes.len() <= 1024 * 1024,
            "Invalid chat update"
        );
        let tx = self.db.transaction()?;
        let existing: Option<u64> = tx
            .query_row(
                "SELECT seq FROM chat_batches WHERE chat=? AND batch=?",
                params![chat, batch],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(seq) = existing {
            return Ok((seq, true));
        }
        tx.execute("INSERT OR IGNORE INTO chats(id) VALUES(?)", [chat])?;
        let head: u64 = tx.query_row("SELECT head FROM chats WHERE id=?", [chat], |r| r.get(0))?;
        let seq = head + 1;
        tx.execute(
            "INSERT INTO chat_rows(chat,seq,device,batch,bytes) VALUES(?,?,?,?,?)",
            params![chat, seq, device, batch, bytes],
        )?;
        tx.execute(
            "INSERT INTO chat_batches(chat,batch,seq) VALUES(?,?,?)",
            params![chat, batch, seq],
        )?;
        tx.execute("UPDATE chats SET head=? WHERE id=?", params![seq, chat])?;
        tx.commit()?;
        Ok((seq, false))
    }

    pub fn checkpoint(
        &mut self,
        chat: &str,
        covered: u64,
        frontier: &[u8],
        bytes: &[u8],
    ) -> Result<usize> {
        ensure!(
            !bytes.is_empty() && bytes.len() <= 64 * 1024 * 1024,
            "Invalid checkpoint size"
        );
        ensure!(
            covered == 0 || !frontier.is_empty(),
            "Checkpoint frontier is required"
        );
        let stats = self.chat_stats(chat)?;
        ensure!(
            covered >= stats.seq_floor && covered <= stats.head_seq,
            "Checkpoint sequence outside retained log"
        );
        let tx = self.db.transaction()?;
        tx.execute(
            "UPDATE chats SET floor=?,checkpoint=?,frontier=?,checkpoint_at=? WHERE id=?",
            params![covered, bytes, frontier, now_ms(), chat],
        )?;
        let pruned = tx.execute(
            "DELETE FROM chat_rows WHERE chat=? AND seq<=?",
            params![chat, covered],
        )?;
        tx.commit()?;
        Ok(pruned)
    }

    pub fn checkpoint_bytes(&self, chat: &str) -> Result<Option<(u64, Vec<u8>)>> {
        Ok(self
            .db
            .query_row(
                "SELECT floor,checkpoint FROM chats WHERE id=? AND checkpoint IS NOT NULL",
                [chat],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    pub fn put_blob(&self, key: &str, content_type: &str, bytes: &[u8]) -> Result<()> {
        self.db.execute("INSERT INTO blobs(key,content_type,bytes) VALUES(?,?,?) ON CONFLICT(key) DO UPDATE SET content_type=excluded.content_type,bytes=excluded.bytes", params![key,content_type,bytes])?;
        Ok(())
    }

    pub fn blob(&self, key: &str) -> Result<Option<(String, Vec<u8>)>> {
        Ok(self
            .db
            .query_row(
                "SELECT content_type,bytes FROM blobs WHERE key=?",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    pub fn queue_nudge(&self, device: &str, chat: &str) -> Result<()> {
        self.db.execute("INSERT INTO nudges(device,chat,queued_at) VALUES(?,?,?) ON CONFLICT(device,chat) DO UPDATE SET queued_at=excluded.queued_at",params![device,chat,now_ms()])?;
        self.db.execute("DELETE FROM nudges WHERE device=? AND chat NOT IN (SELECT chat FROM nudges WHERE device=? ORDER BY queued_at DESC LIMIT 256)",params![device,device])?;
        Ok(())
    }

    pub fn nudges(&self, device: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .db
            .prepare("SELECT chat FROM nudges WHERE device=? ORDER BY queued_at")?;
        Ok(stmt
            .query_map([device], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn remove_nudge(&self, device: &str, chat: &str) -> Result<()> {
        self.db.execute(
            "DELETE FROM nudges WHERE device=? AND chat=?",
            params![device, chat],
        )?;
        Ok(())
    }
}

fn parse_role(role: &str) -> Result<NodeRole> {
    match role {
        "client" => Ok(NodeRole::Client),
        "server" => Ok(NodeRole::Server),
        _ => anyhow::bail!("Invalid stored node role"),
    }
}

fn valid_hlc(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() > 21
        && bytes.len() <= 149
        && bytes[..13].iter().all(u8::is_ascii_digit)
        && bytes[13] == b'-'
        && bytes[14..20].iter().all(u8::is_ascii_digit)
        && bytes[20] == b'-'
        && valid_id(&value[21..])
}

fn valid_field(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_alphabetic()
        && value.bytes().all(|c| c.is_ascii_alphanumeric())
}

fn validate_op(op: &RowOp) -> Result<()> {
    ensure!(
        !op.kind.is_empty()
            && op.kind.len() <= 32
            && op.kind.as_bytes()[0].is_ascii_lowercase()
            && op.kind.bytes().all(|c| c.is_ascii_alphanumeric()),
        "Invalid row kind"
    );
    ensure!(
        !op.id.is_empty()
            && op.id.len() <= 256
            && op
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_.:@/-".contains(&c)),
        "Invalid row ID"
    );
    ensure!(valid_hlc(&op.hlc), "Invalid row clock");
    match op.op {
        OpKind::Delete => ensure!(op.set.is_none(), "Delete carries fields"),
        _ => ensure!(
            op.set
                .as_ref()
                .is_some_and(|set| set.keys().all(|key| valid_field(key))),
            "Invalid row fields"
        ),
    }
    if let Some(clocks) = &op.clocks {
        ensure!(
            clocks
                .iter()
                .all(|(field, clock)| valid_field(field) && valid_hlc(clock)),
            "Invalid field clocks"
        );
    }
    ensure!(
        serde_json::to_vec(op)?.len() <= 16 * 1024,
        "Registry operation is too large"
    );
    Ok(())
}

pub(crate) fn validate_registry_push(push: &RegistryPush) -> Result<()> {
    ensure!(
        !push.batch.is_empty()
            && push.batch.len() <= 128
            && !push.ops.is_empty()
            && push.ops.len() <= 500,
        "Invalid registry batch"
    );
    for op in &push.ops {
        validate_op(op)?;
    }
    Ok(())
}
