//! Typed ticket rows in the per-user registry. Relations are separate rows so
//! concurrent links never race through a last-writer-wins array field.

use std::collections::{BTreeMap, HashSet};

use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use zeron_proto::{
    BoardSpaceLink, TaskBoard, Ticket, TicketChatLink, TicketComment, TicketSnapshot,
};

use super::{
    KIND_BOARD_SPACE_LINKS, KIND_TASK_BOARDS, KIND_TICKET_CHAT_LINKS, KIND_TICKET_COMMENTS,
    KIND_TICKETS, OpKind, RegistryDoc, RowOp, encode_hlc, row_to,
};
use crate::schema::DocError;

fn row_fields<T: Serialize>(value: &T) -> Result<BTreeMap<String, Value>, DocError> {
    match serde_json::to_value(value)? {
        Value::Object(fields) => Ok(fields.into_iter().collect()),
        _ => Err(DocError::Schema("ticket row must be an object".into())),
    }
}

/// Collision-free for any pair of IDs accepted by the registry. Length is
/// checked by the engine before a link row is written.
pub fn relation_id(left: &str, right: &str) -> String {
    format!("{}:{left}:{right}", left.len())
}

impl RegistryDoc {
    fn write_ticket_row(
        &mut self,
        kind: &str,
        id: &str,
        op: OpKind,
        set: BTreeMap<String, Value>,
    ) -> Result<(), DocError> {
        // The edge checks JSON.stringify(op).length <= 16 KiB. UTF-8 bytes are
        // a conservative measure of that length and prevent a rejected pending
        // op from getting stuck in the offline queue.
        let shape = RowOp {
            kind: kind.to_string(),
            id: id.to_string(),
            op,
            set: Some(set.clone()),
            hlc: encode_hlc(0, 0, self.device_id()),
            clocks: None,
        };
        if serde_json::to_vec(&shape)?.len() > 16 * 1024 {
            return Err(DocError::Schema(
                "ticket record exceeds the registry operation limit".into(),
            ));
        }
        self.write(kind, id, op, set);
        Ok(())
    }

    pub fn read_tickets(&self) -> Result<TicketSnapshot, DocError> {
        let mut snapshot = TicketSnapshot {
            boards: self.read_kind(KIND_TASK_BOARDS),
            board_space_links: self.read_kind(KIND_BOARD_SPACE_LINKS),
            tickets: self.read_kind(KIND_TICKETS),
            chat_links: self.read_kind(KIND_TICKET_CHAT_LINKS),
            comments: self.read_kind(KIND_TICKET_COMMENTS),
        };
        snapshot.boards.sort_by(|a, b| a.id.cmp(&b.id));
        snapshot.board_space_links.sort_by(|a, b| a.id.cmp(&b.id));
        snapshot.tickets.sort_by(|a, b| a.id.cmp(&b.id));
        snapshot.chat_links.sort_by(|a, b| a.id.cmp(&b.id));
        snapshot.comments.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(snapshot)
    }

    pub fn task_board(&self, id: &str) -> Option<TaskBoard> {
        self.overlay_row(KIND_TASK_BOARDS, id)
            .and_then(|row| row_to(&row))
    }

    pub fn ticket(&self, id: &str) -> Option<Ticket> {
        self.overlay_row(KIND_TICKETS, id)
            .and_then(|row| row_to(&row))
    }

    pub fn ticket_comment(&self, id: &str) -> Option<TicketComment> {
        self.overlay_row(KIND_TICKET_COMMENTS, id)
            .and_then(|row| row_to(&row))
    }

    pub fn board_space_link(&self, id: &str) -> Option<BoardSpaceLink> {
        self.overlay_row(KIND_BOARD_SPACE_LINKS, id)
            .and_then(|row| row_to(&row))
    }

    pub fn ticket_chat_link(&self, id: &str) -> Option<TicketChatLink> {
        self.overlay_row(KIND_TICKET_CHAT_LINKS, id)
            .and_then(|row| row_to(&row))
    }

    pub fn upsert_task_board(&mut self, board: &TaskBoard) -> Result<(), DocError> {
        self.write_ticket_row(
            KIND_TASK_BOARDS,
            &board.id,
            OpKind::Upsert,
            row_fields(board)?,
        )
    }

    pub fn patch_task_board(
        &mut self,
        board_id: &str,
        set: BTreeMap<String, Value>,
    ) -> Result<bool, DocError> {
        if !self.row_exists(KIND_TASK_BOARDS, board_id) || set.is_empty() {
            return Ok(false);
        }
        self.write_ticket_row(KIND_TASK_BOARDS, board_id, OpKind::Update, set)?;
        Ok(true)
    }

    pub fn delete_task_board(&mut self, board_id: &str) -> Result<bool, DocError> {
        if !self.row_exists(KIND_TASK_BOARDS, board_id) {
            return Ok(false);
        }
        let snapshot = self.read_tickets()?;
        let ticket_ids: HashSet<String> = snapshot
            .tickets
            .iter()
            .filter(|ticket| ticket.board_id == board_id)
            .map(|ticket| ticket.id.clone())
            .collect();
        let mut keys: Vec<(&str, &str)> = vec![(KIND_TASK_BOARDS, board_id)];
        keys.extend(
            snapshot
                .board_space_links
                .iter()
                .filter(|link| link.board_id == board_id)
                .map(|link| (KIND_BOARD_SPACE_LINKS, link.id.as_str())),
        );
        keys.extend(
            snapshot
                .tickets
                .iter()
                .filter(|ticket| ticket_ids.contains(&ticket.id))
                .map(|ticket| (KIND_TICKETS, ticket.id.as_str())),
        );
        keys.extend(
            snapshot
                .chat_links
                .iter()
                .filter(|link| ticket_ids.contains(&link.ticket_id))
                .map(|link| (KIND_TICKET_CHAT_LINKS, link.id.as_str())),
        );
        keys.extend(
            snapshot
                .comments
                .iter()
                .filter(|comment| ticket_ids.contains(&comment.ticket_id))
                .map(|comment| (KIND_TICKET_COMMENTS, comment.id.as_str())),
        );
        self.delete_row_ops(&keys);
        Ok(true)
    }

    pub fn upsert_board_space_link(&mut self, link: &BoardSpaceLink) -> Result<(), DocError> {
        self.write_ticket_row(
            KIND_BOARD_SPACE_LINKS,
            &link.id,
            OpKind::Upsert,
            row_fields(link)?,
        )
    }

    pub fn delete_board_space_link(&mut self, board_id: &str, space_id: &str) -> bool {
        let id = relation_id(board_id, space_id);
        if !self.row_exists(KIND_BOARD_SPACE_LINKS, &id) {
            return false;
        }
        self.delete_row_ops(&[(KIND_BOARD_SPACE_LINKS, &id)]);
        true
    }

    pub fn delete_board_space_links_for_space(&mut self, space_id: &str) {
        let links: Vec<BoardSpaceLink> = self.read_kind(KIND_BOARD_SPACE_LINKS);
        let keys: Vec<_> = links
            .iter()
            .filter(|link| link.space_id == space_id)
            .map(|link| (KIND_BOARD_SPACE_LINKS, link.id.as_str()))
            .collect();
        if !keys.is_empty() {
            self.delete_row_ops(&keys);
        }
    }

    pub fn upsert_ticket(&mut self, ticket: &Ticket) -> Result<(), DocError> {
        self.write_ticket_row(
            KIND_TICKETS,
            &ticket.id,
            OpKind::Upsert,
            row_fields(ticket)?,
        )
    }

    pub fn patch_ticket(
        &mut self,
        ticket_id: &str,
        set: BTreeMap<String, Value>,
    ) -> Result<bool, DocError> {
        if !self.row_exists(KIND_TICKETS, ticket_id) || set.is_empty() {
            return Ok(false);
        }
        self.write_ticket_row(KIND_TICKETS, ticket_id, OpKind::Update, set)?;
        Ok(true)
    }

    pub fn delete_ticket_with_children(&mut self, ticket_id: &str) -> Result<bool, DocError> {
        if !self.row_exists(KIND_TICKETS, ticket_id) {
            return Ok(false);
        }
        let snapshot = self.read_tickets()?;
        let mut ticket_ids: HashSet<String> = HashSet::from([ticket_id.to_string()]);
        loop {
            let before = ticket_ids.len();
            for ticket in &snapshot.tickets {
                if ticket
                    .parent_ticket_id
                    .as_ref()
                    .is_some_and(|parent| ticket_ids.contains(parent))
                {
                    ticket_ids.insert(ticket.id.clone());
                }
            }
            if ticket_ids.len() == before {
                break;
            }
        }
        let mut keys: Vec<(&str, &str)> = vec![(KIND_TICKETS, ticket_id)];
        keys.extend(
            snapshot
                .tickets
                .iter()
                .filter(|ticket| ticket.id != ticket_id && ticket_ids.contains(&ticket.id))
                .map(|ticket| (KIND_TICKETS, ticket.id.as_str())),
        );
        keys.extend(
            snapshot
                .chat_links
                .iter()
                .filter(|link| ticket_ids.contains(&link.ticket_id))
                .map(|link| (KIND_TICKET_CHAT_LINKS, link.id.as_str())),
        );
        keys.extend(
            snapshot
                .comments
                .iter()
                .filter(|comment| ticket_ids.contains(&comment.ticket_id))
                .map(|comment| (KIND_TICKET_COMMENTS, comment.id.as_str())),
        );
        self.delete_row_ops(&keys);
        Ok(true)
    }

    pub fn upsert_ticket_chat_link(&mut self, link: &TicketChatLink) -> Result<(), DocError> {
        self.write_ticket_row(
            KIND_TICKET_CHAT_LINKS,
            &link.id,
            OpKind::Upsert,
            row_fields(link)?,
        )
    }

    pub fn delete_ticket_chat_link(&mut self, ticket_id: &str, chat_id: &str) -> bool {
        let id = relation_id(ticket_id, chat_id);
        if !self.row_exists(KIND_TICKET_CHAT_LINKS, &id) {
            return false;
        }
        self.delete_row_ops(&[(KIND_TICKET_CHAT_LINKS, &id)]);
        true
    }

    pub fn upsert_ticket_comment(&mut self, comment: &TicketComment) -> Result<(), DocError> {
        self.write_ticket_row(
            KIND_TICKET_COMMENTS,
            &comment.id,
            OpKind::Upsert,
            row_fields(comment)?,
        )
    }

    pub fn patch_ticket_comment(&mut self, comment_id: &str, body: &str) -> Result<bool, DocError> {
        if !self.row_exists(KIND_TICKET_COMMENTS, comment_id) {
            return Ok(false);
        }
        let mut set = BTreeMap::new();
        set.insert("body".into(), Value::String(body.to_string()));
        set.insert("updatedAt".into(), Value::String(Utc::now().to_rfc3339()));
        self.write_ticket_row(KIND_TICKET_COMMENTS, comment_id, OpKind::Update, set)?;
        Ok(true)
    }

    pub fn delete_ticket_comment(&mut self, comment_id: &str) -> bool {
        if !self.row_exists(KIND_TICKET_COMMENTS, comment_id) {
            return false;
        }
        self.delete_row_ops(&[(KIND_TICKET_COMMENTS, comment_id)]);
        true
    }
}
