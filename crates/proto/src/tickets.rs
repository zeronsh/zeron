//! Native planning records. A board is independent of a device/folder Space;
//! links attach execution context without making chats or Spaces own tickets.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskBoard {
    pub id: String,
    pub name: String,
    pub description: String,
    pub archived: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardSpaceLink {
    pub id: String,
    pub board_id: String,
    pub space_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TicketKind {
    Epic,
    Issue,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TicketStatus {
    #[default]
    Backlog,
    Todo,
    InProgress,
    InReview,
    Done,
    Canceled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TicketPriority {
    #[default]
    None,
    Urgent,
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ticket {
    pub id: String,
    pub board_id: String,
    pub kind: TicketKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_ticket_id: Option<String>,
    pub title: String,
    pub description: String,
    pub status: TicketStatus,
    pub priority: TicketPriority,
    pub archived: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketChatLink {
    pub id: String,
    pub ticket_id: String,
    pub chat_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketComment {
    pub id: String,
    pub ticket_id: String,
    pub body: String,
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketSnapshot {
    pub boards: Vec<TaskBoard>,
    pub board_space_links: Vec<BoardSpaceLink>,
    pub tickets: Vec<Ticket>,
    pub chat_links: Vec<TicketChatLink>,
    pub comments: Vec<TicketComment>,
}

/// The same mutation contract is used by the native UI and the agent-facing
/// control surface. Callers mint IDs before creation so retries are idempotent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum TicketMutation {
    #[serde(rename_all = "camelCase")]
    CreateBoard {
        board_id: String,
        name: String,
        #[serde(default)]
        description: String,
    },
    #[serde(rename_all = "camelCase")]
    UpdateBoard {
        board_id: String,
        name: Option<String>,
        description: Option<String>,
        archived: Option<bool>,
    },
    #[serde(rename_all = "camelCase")]
    DeleteBoard { board_id: String },
    #[serde(rename_all = "camelCase")]
    LinkBoardSpace { board_id: String, space_id: String },
    #[serde(rename_all = "camelCase")]
    UnlinkBoardSpace { board_id: String, space_id: String },
    #[serde(rename_all = "camelCase")]
    CreateTicket {
        ticket_id: String,
        board_id: String,
        kind: TicketKind,
        #[serde(default)]
        parent_ticket_id: Option<String>,
        title: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        status: TicketStatus,
        #[serde(default)]
        priority: TicketPriority,
    },
    #[serde(rename_all = "camelCase")]
    UpdateTicket {
        ticket_id: String,
        title: Option<String>,
        description: Option<String>,
        kind: Option<TicketKind>,
        status: Option<TicketStatus>,
        priority: Option<TicketPriority>,
        archived: Option<bool>,
    },
    #[serde(rename_all = "camelCase")]
    SetTicketParent {
        ticket_id: String,
        #[serde(default)]
        parent_ticket_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    DeleteTicket { ticket_id: String },
    #[serde(rename_all = "camelCase")]
    LinkTicketChat { ticket_id: String, chat_id: String },
    #[serde(rename_all = "camelCase")]
    UnlinkTicketChat { ticket_id: String, chat_id: String },
    #[serde(rename_all = "camelCase")]
    CreateComment {
        comment_id: String,
        ticket_id: String,
        body: String,
        #[serde(default)]
        author: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    UpdateComment { comment_id: String, body: String },
    #[serde(rename_all = "camelCase")]
    DeleteComment { comment_id: String },
}
