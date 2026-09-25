//! Validated, profile-wide ticket mutations over the offline-first registry.

use std::collections::{BTreeMap, HashSet};

use chrono::Utc;
use serde_json::{Value, json};
use zeron_doc::{RegistryDoc, relation_id};
use zeron_proto::{
    BoardSpaceLink, TaskBoard, Ticket, TicketChatLink, TicketComment, TicketKind, TicketMutation,
    TicketSnapshot,
};

use super::WorkspaceHost;
use crate::EngineError;

const MAX_BOARD_NAME_BYTES: usize = 160;
const MAX_TICKET_TITLE_BYTES: usize = 240;
const MAX_DESCRIPTION_BYTES: usize = 12 * 1024;
const MAX_COMMENT_BYTES: usize = 12 * 1024;
const MAX_AUTHOR_BYTES: usize = 120;

fn invalid(message: impl Into<String>) -> EngineError {
    EngineError::Other(message.into())
}

fn valid_id(id: &str) -> Result<(), EngineError> {
    if id.is_empty()
        || id.len() > 256
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.:@/-".contains(&byte))
    {
        return Err(invalid("ticket IDs must use registry-safe characters"));
    }
    Ok(())
}

fn validated_title(text: &str, label: &str, max: usize) -> Result<String, EngineError> {
    let text = text.trim();
    if text.is_empty() || text.len() > max {
        return Err(invalid(format!("{label} must be 1–{max} bytes")));
    }
    Ok(text.to_string())
}

fn validated_text(text: &str, label: &str, max: usize) -> Result<(), EngineError> {
    if text.len() > max {
        return Err(invalid(format!("{label} exceeds {max} bytes")));
    }
    Ok(())
}

fn validated_body(text: &str) -> Result<(), EngineError> {
    validated_text(text, "Comment", MAX_COMMENT_BYTES)?;
    if text.trim().is_empty() {
        return Err(invalid("Comment cannot be empty"));
    }
    Ok(())
}

fn validated_relation_id(left: &str, right: &str) -> Result<String, EngineError> {
    valid_id(left)?;
    valid_id(right)?;
    let id = relation_id(left, right);
    valid_id(&id)?;
    Ok(id)
}

fn require_board(doc: &RegistryDoc, board_id: &str) -> Result<TaskBoard, EngineError> {
    doc.task_board(board_id)
        .ok_or_else(|| invalid(format!("No board with id {board_id}")))
}

fn require_ticket(doc: &RegistryDoc, ticket_id: &str) -> Result<Ticket, EngineError> {
    doc.ticket(ticket_id)
        .ok_or_else(|| invalid(format!("No ticket with id {ticket_id}")))
}

fn validate_parent(
    doc: &RegistryDoc,
    ticket_id: &str,
    board_id: &str,
    kind: TicketKind,
    parent_ticket_id: Option<&str>,
) -> Result<(), EngineError> {
    if kind == TicketKind::Epic && parent_ticket_id.is_some() {
        return Err(invalid("Epics must be root tickets"));
    }
    let mut seen = HashSet::new();
    let mut cursor = parent_ticket_id.map(str::to_owned);
    while let Some(parent_id) = cursor {
        if parent_id == ticket_id || !seen.insert(parent_id.clone()) {
            return Err(invalid("Ticket parent would create a cycle"));
        }
        let parent = require_ticket(doc, &parent_id)?;
        if parent.board_id != board_id {
            return Err(invalid("Parent ticket must be on the same board"));
        }
        cursor = parent.parent_ticket_id;
    }
    Ok(())
}

impl WorkspaceHost {
    pub fn read_tickets(&self) -> Result<TicketSnapshot, EngineError> {
        Ok(self.read(|doc| doc.read_tickets())?)
    }

    pub fn mutate_ticket(&self, mutation: TicketMutation) -> Result<(), EngineError> {
        self.mutate(|doc| apply_mutation(doc, mutation))?;
        // MCP can read a fresh watch snapshot immediately after this RPC
        // returns, so publish the new rows before acknowledging the write.
        self.inner.publish();
        // A successful ticket mutation is durable before RPC acknowledges it.
        // The normal debounce still handles server merges and other rows.
        self.inner.persist_snapshot()?;
        Ok(())
    }
}

fn apply_mutation(doc: &mut RegistryDoc, mutation: TicketMutation) -> Result<(), EngineError> {
    match mutation {
        TicketMutation::CreateBoard {
            board_id,
            name,
            description,
        } => {
            valid_id(&board_id)?;
            let name = validated_title(&name, "Board name", MAX_BOARD_NAME_BYTES)?;
            validated_text(&description, "Board description", MAX_DESCRIPTION_BYTES)?;
            if let Some(existing) = doc.task_board(&board_id) {
                if existing.name == name && existing.description == description {
                    return Ok(());
                }
                return Err(invalid("Board ID already belongs to another board"));
            }
            let now = Utc::now();
            doc.upsert_task_board(&TaskBoard {
                id: board_id,
                name,
                description,
                archived: false,
                created_at: now,
                updated_at: now,
            })?;
        }
        TicketMutation::UpdateBoard {
            board_id,
            name,
            description,
            archived,
        } => {
            require_board(doc, &board_id)?;
            let mut set = BTreeMap::new();
            if let Some(name) = name {
                set.insert(
                    "name".into(),
                    Value::String(validated_title(&name, "Board name", MAX_BOARD_NAME_BYTES)?),
                );
            }
            if let Some(description) = description {
                validated_text(&description, "Board description", MAX_DESCRIPTION_BYTES)?;
                set.insert("description".into(), Value::String(description));
            }
            if let Some(archived) = archived {
                set.insert("archived".into(), json!(archived));
            }
            if !set.is_empty() {
                set.insert("updatedAt".into(), json!(Utc::now()));
                doc.patch_task_board(&board_id, set)?;
            }
        }
        TicketMutation::DeleteBoard { board_id } => {
            doc.delete_task_board(&board_id)?;
        }
        TicketMutation::LinkBoardSpace { board_id, space_id } => {
            require_board(doc, &board_id)?;
            if doc.space(&space_id)?.is_none() {
                return Err(invalid(format!("No Space with id {space_id}")));
            }
            let id = validated_relation_id(&board_id, &space_id)?;
            if doc.board_space_link(&id).is_none() {
                doc.upsert_board_space_link(&BoardSpaceLink {
                    id,
                    board_id,
                    space_id,
                    created_at: Utc::now(),
                })?;
            }
        }
        TicketMutation::UnlinkBoardSpace { board_id, space_id } => {
            validated_relation_id(&board_id, &space_id)?;
            doc.delete_board_space_link(&board_id, &space_id);
        }
        TicketMutation::CreateTicket {
            ticket_id,
            board_id,
            kind,
            parent_ticket_id,
            title,
            description,
            status,
            priority,
        } => {
            valid_id(&ticket_id)?;
            require_board(doc, &board_id)?;
            let title = validated_title(&title, "Ticket title", MAX_TICKET_TITLE_BYTES)?;
            validated_text(&description, "Ticket description", MAX_DESCRIPTION_BYTES)?;
            validate_parent(
                doc,
                &ticket_id,
                &board_id,
                kind,
                parent_ticket_id.as_deref(),
            )?;
            if let Some(existing) = doc.ticket(&ticket_id) {
                if existing.board_id == board_id
                    && existing.kind == kind
                    && existing.parent_ticket_id == parent_ticket_id
                    && existing.title == title
                    && existing.description == description
                    && existing.status == status
                    && existing.priority == priority
                {
                    return Ok(());
                }
                return Err(invalid("Ticket ID already belongs to another ticket"));
            }
            let now = Utc::now();
            doc.upsert_ticket(&Ticket {
                id: ticket_id,
                board_id,
                kind,
                parent_ticket_id,
                title,
                description,
                status,
                priority,
                archived: false,
                created_at: now,
                updated_at: now,
            })?;
        }
        TicketMutation::UpdateTicket {
            ticket_id,
            title,
            description,
            kind,
            status,
            priority,
            archived,
        } => {
            let ticket = require_ticket(doc, &ticket_id)?;
            if let Some(kind) = kind {
                validate_parent(
                    doc,
                    &ticket.id,
                    &ticket.board_id,
                    kind,
                    ticket.parent_ticket_id.as_deref(),
                )?;
            }
            let mut set = BTreeMap::new();
            if let Some(title) = title {
                set.insert(
                    "title".into(),
                    Value::String(validated_title(
                        &title,
                        "Ticket title",
                        MAX_TICKET_TITLE_BYTES,
                    )?),
                );
            }
            if let Some(description) = description {
                validated_text(&description, "Ticket description", MAX_DESCRIPTION_BYTES)?;
                set.insert("description".into(), Value::String(description));
            }
            if let Some(kind) = kind {
                set.insert("kind".into(), json!(kind));
            }
            if let Some(status) = status {
                set.insert("status".into(), json!(status));
            }
            if let Some(priority) = priority {
                set.insert("priority".into(), json!(priority));
            }
            if let Some(archived) = archived {
                set.insert("archived".into(), json!(archived));
            }
            if !set.is_empty() {
                set.insert("updatedAt".into(), json!(Utc::now()));
                doc.patch_ticket(&ticket_id, set)?;
            }
        }
        TicketMutation::SetTicketParent {
            ticket_id,
            parent_ticket_id,
        } => {
            let ticket = require_ticket(doc, &ticket_id)?;
            validate_parent(
                doc,
                &ticket.id,
                &ticket.board_id,
                ticket.kind,
                parent_ticket_id.as_deref(),
            )?;
            if ticket.parent_ticket_id != parent_ticket_id {
                let mut set = BTreeMap::new();
                set.insert("parentTicketId".into(), json!(parent_ticket_id));
                set.insert("updatedAt".into(), json!(Utc::now()));
                doc.patch_ticket(&ticket_id, set)?;
            }
        }
        TicketMutation::DeleteTicket { ticket_id } => {
            doc.delete_ticket_with_children(&ticket_id)?;
        }
        TicketMutation::LinkTicketChat { ticket_id, chat_id } => {
            require_ticket(doc, &ticket_id)?;
            if doc.chat(&chat_id)?.is_none() {
                return Err(invalid(format!("No chat with id {chat_id}")));
            }
            let id = validated_relation_id(&ticket_id, &chat_id)?;
            if doc.ticket_chat_link(&id).is_none() {
                doc.upsert_ticket_chat_link(&TicketChatLink {
                    id,
                    ticket_id,
                    chat_id,
                    created_at: Utc::now(),
                })?;
            }
        }
        TicketMutation::UnlinkTicketChat { ticket_id, chat_id } => {
            validated_relation_id(&ticket_id, &chat_id)?;
            doc.delete_ticket_chat_link(&ticket_id, &chat_id);
        }
        TicketMutation::CreateComment {
            comment_id,
            ticket_id,
            body,
            author,
        } => {
            valid_id(&comment_id)?;
            require_ticket(doc, &ticket_id)?;
            validated_body(&body)?;
            let author = validated_title(
                author.as_deref().unwrap_or("Unknown"),
                "Comment author",
                MAX_AUTHOR_BYTES,
            )?;
            if let Some(existing) = doc.ticket_comment(&comment_id) {
                if existing.ticket_id == ticket_id
                    && existing.body == body
                    && existing.author == author
                {
                    return Ok(());
                }
                return Err(invalid("Comment ID already belongs to another comment"));
            }
            let now = Utc::now();
            doc.upsert_ticket_comment(&TicketComment {
                id: comment_id,
                ticket_id,
                body,
                author,
                created_at: now,
                updated_at: now,
            })?;
        }
        TicketMutation::UpdateComment { comment_id, body } => {
            validated_body(&body)?;
            if doc.ticket_comment(&comment_id).is_none() {
                return Err(invalid(format!("No comment with id {comment_id}")));
            }
            doc.patch_ticket_comment(&comment_id, &body)?;
        }
        TicketMutation::DeleteComment { comment_id } => {
            doc.delete_ticket_comment(&comment_id);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::{Value, json};
    use zeron_doc::RegistryDoc;
    use zeron_proto::Space;

    use super::apply_mutation;
    use zeron_proto::TicketMutation;

    fn apply(doc: &mut RegistryDoc, value: Value) -> Result<(), crate::EngineError> {
        let mutation: TicketMutation = serde_json::from_value(value).unwrap();
        apply_mutation(doc, mutation)
    }

    #[test]
    fn epics_nested_issues_and_comments_are_durable_and_cascade() {
        let mut doc = RegistryDoc::new("dev-a");
        apply(
            &mut doc,
            json!({"op":"createBoard","boardId":"board-1","name":"Product"}),
        )
        .unwrap();
        apply(
            &mut doc,
            json!({"op":"createTicket","ticketId":"epic-1","boardId":"board-1","kind":"epic","title":"Release"}),
        )
        .unwrap();
        apply(
            &mut doc,
            json!({"op":"createTicket","ticketId":"issue-1","boardId":"board-1","kind":"issue","parentTicketId":"epic-1","title":"Desktop"}),
        )
        .unwrap();
        apply(
            &mut doc,
            json!({"op":"createTicket","ticketId":"issue-2","boardId":"board-1","kind":"issue","parentTicketId":"issue-1","title":"Tasks"}),
        )
        .unwrap();
        apply(
            &mut doc,
            json!({"op":"createComment","commentId":"comment-1","ticketId":"issue-2","body":"In progress","author":"Agent"}),
        )
        .unwrap();
        apply(
            &mut doc,
            json!({"op":"updateComment","commentId":"comment-1","body":"Ready for review"}),
        )
        .unwrap();
        assert_eq!(
            doc.ticket_comment("comment-1").unwrap().body,
            "Ready for review"
        );
        assert!(
            apply(
                &mut doc,
                json!({"op":"setTicketParent","ticketId":"issue-1","parentTicketId":"issue-2"}),
            )
            .is_err()
        );
        assert!(
            apply(
                &mut doc,
                json!({"op":"setTicketParent","ticketId":"epic-1","parentTicketId":"issue-1"}),
            )
            .is_err()
        );
        let saved = doc.to_bytes().unwrap();
        let mut restored = RegistryDoc::from_bytes(&saved, "dev-a").unwrap();
        assert_eq!(restored.read_tickets().unwrap().tickets.len(), 3);
        assert_eq!(restored.read_tickets().unwrap().comments.len(), 1);
        apply(
            &mut restored,
            json!({"op":"deleteTicket","ticketId":"epic-1"}),
        )
        .unwrap();
        assert!(restored.read_tickets().unwrap().tickets.is_empty());
        assert!(restored.read_tickets().unwrap().comments.is_empty());
    }

    #[test]
    fn invalid_references_and_oversized_text_do_not_write() {
        let mut doc = RegistryDoc::new("dev-a");
        apply(
            &mut doc,
            json!({"op":"createBoard","boardId":"board-1","name":"Product"}),
        )
        .unwrap();
        let before = doc.pending_len();
        assert!(
            apply(
                &mut doc,
                json!({"op":"createTicket","ticketId":"issue-1","boardId":"missing","kind":"issue","title":"No board"}),
            )
            .is_err()
        );
        assert!(
            apply(
                &mut doc,
                json!({"op":"createTicket","ticketId":"issue-2","boardId":"board-1","kind":"issue","title":"Huge","description":"x".repeat(13 * 1024)}),
            )
            .is_err()
        );
        assert!(
            apply(
                &mut doc,
                json!({"op":"createTicket","ticketId":"issue-3","boardId":"board-1","kind":"issue","title":"Escaped","description":"\n".repeat(10 * 1024)}),
            )
            .is_err(),
            "wire-escaped text must be rejected before it enters the pending queue"
        );
        assert_eq!(doc.pending_len(), before);
        assert!(doc.read_tickets().unwrap().tickets.is_empty());
    }

    #[test]
    fn deleting_space_unlinks_board_without_deleting_plan() {
        let mut doc = RegistryDoc::new("dev-a");
        doc.upsert_space(&Space {
            id: "space-1".into(),
            device_id: "dev-a".into(),
            path: "/repo".into(),
            name: None,
            git_detected: false,
            git_checked_at: None,
            checkout_id: None,
            created_at: Utc::now(),
        })
        .unwrap();
        apply(
            &mut doc,
            json!({"op":"createBoard","boardId":"board-1","name":"Product"}),
        )
        .unwrap();
        apply(
            &mut doc,
            json!({"op":"linkBoardSpace","boardId":"board-1","spaceId":"space-1"}),
        )
        .unwrap();
        assert_eq!(doc.read_tickets().unwrap().board_space_links.len(), 1);
        doc.delete_space("space-1").unwrap();
        let snapshot = doc.read_tickets().unwrap();
        assert_eq!(snapshot.boards.len(), 1);
        assert!(snapshot.board_space_links.is_empty());
    }
}
